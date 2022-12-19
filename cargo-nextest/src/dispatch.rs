// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputOpts, OutputWriter},
    reuse_build::{make_path_mapper, ArchiveFormatOpt, ReuseBuildOpts},
    ExpectedError, Result, ReuseBuildKind,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use guppy::graph::PackageGraph;
use itertools::Itertools;
use nextest_filtering::FilteringExpr;
use nextest_metadata::{BinaryListSummary, BuildPlatform};
use nextest_runner::{
    cargo_config::{CargoConfigs, EnvironmentMap, TargetTriple},
    config::{
        get_num_cpus, NextestConfig, NextestProfile, PreBuildPlatform, RetryPolicy, TestThreads,
        ToolConfigFile,
    },
    double_spawn::DoubleSpawnInfo,
    errors::WriteTestListError,
    list::{
        BinaryList, OutputFormat, RustTestArtifact, SerializableFormat, TestExecuteContext,
        TestList,
    },
    partition::PartitionerBuilder,
    platform::BuildPlatforms,
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay, TestReporterBuilder},
    reuse_build::{archive_to_file, ArchiveReporter, MetadataOrPath, PathMapper, ReuseBuildInfo},
    runner::{configure_handle_inheritance, TestRunnerBuilder},
    signal::SignalHandlerKind,
    target_runner::{PlatformRunner, TargetRunner},
    test_filter::{RunIgnored, TestFilterBuilder},
};
use once_cell::sync::OnceCell;
use owo_colors::{OwoColorize, Stream, Style};
use std::{
    fmt::Write as _,
    io::{Cursor, Write},
    sync::Arc,
};

/// A next-generation test runner for Rust.
///
/// This binary should typically be invoked as `cargo nextest` (in which case
/// this message will not be seen), not `cargo-nextest`.
#[derive(Debug, Parser)]
#[command(version, bin_name = "cargo")]
pub struct CargoNextestApp {
    #[clap(subcommand)]
    subcommand: NextestSubcommand,
}

impl CargoNextestApp {
    /// Executes the app.
    pub fn exec(self, output_writer: &mut OutputWriter) -> Result<i32> {
        #[cfg(feature = "experimental-tokio-console")]
        nextest_runner::console::init();

        match self.subcommand {
            NextestSubcommand::Nextest(app) => app.exec(output_writer),
            #[cfg(unix)]
            NextestSubcommand::DoubleSpawn(opts) => opts.exec(),
        }
    }
}

#[derive(Debug, Subcommand)]
enum NextestSubcommand {
    /// A next-generation test runner for Rust. <https://nexte.st>
    Nextest(Box<AppOpts>),
    /// Private command, used to double-spawn test processes.
    #[cfg(unix)]
    #[command(name = nextest_runner::double_spawn::DoubleSpawnInfo::SUBCOMMAND_NAME, hide = true)]
    DoubleSpawn(crate::double_spawn::DoubleSpawnOpts),
}

#[derive(Debug, Args)]
#[command(version)]
struct AppOpts {
    /// Path to Cargo.toml
    #[arg(long, global = true, value_name = "PATH")]
    manifest_path: Option<Utf8PathBuf>,

    #[clap(flatten)]
    output: OutputOpts,

    #[clap(flatten)]
    config_opts: ConfigOpts,

    #[clap(subcommand)]
    command: Command,
}

impl AppOpts {
    /// Execute the command.
    ///
    /// Returns the exit code.
    fn exec(self, output_writer: &mut OutputWriter) -> Result<i32> {
        fn build_filter_needs_deps(build_filter: &TestBuildFilter) -> bool {
            build_filter
                .filter_expr
                .iter()
                .any(|expr| FilteringExpr::needs_deps(expr))
        }

        match self.command {
            Command::List {
                cargo_options,
                build_filter,
                message_format,
                list_type,
                reuse_build,
                ..
            } => {
                let base = BaseApp::new(
                    self.output,
                    reuse_build,
                    cargo_options,
                    self.config_opts,
                    self.manifest_path,
                    build_filter_needs_deps(&build_filter),
                    output_writer,
                )?;
                let app = App::new(base, build_filter)?;
                app.exec_list(message_format, list_type, output_writer)?;
                Ok(0)
            }
            Command::Run {
                profile,
                no_capture,
                cargo_options,
                build_filter,
                runner_opts,
                reporter_opts,
                reuse_build,
                ..
            } => {
                let base = BaseApp::new(
                    self.output,
                    reuse_build,
                    cargo_options,
                    self.config_opts,
                    self.manifest_path,
                    build_filter_needs_deps(&build_filter),
                    output_writer,
                )?;
                let app = App::new(base, build_filter)?;
                app.exec_run(
                    profile.as_deref(),
                    no_capture,
                    &runner_opts,
                    &reporter_opts,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::Archive {
                cargo_options,
                archive_file,
                archive_format,
                zstd_level,
            } => {
                let app = BaseApp::new(
                    self.output,
                    ReuseBuildOpts::default(),
                    cargo_options,
                    self.config_opts,
                    self.manifest_path,
                    true,
                    output_writer,
                )?;
                app.exec_archive(&archive_file, archive_format, zstd_level, output_writer)?;
                Ok(0)
            }
            Command::Self_ { command } => command.exec(self.output),
        }
    }
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Config options")]
struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml]
    #[arg(long, global = true, value_name = "PATH")]
    pub config_file: Option<Utf8PathBuf>,

    /// Tool-specific config files
    ///
    /// Some tools on top of nextest may want to set up their own default configuration but
    /// prioritize user configuration on top. Use this argument to insert configuration
    /// that's lower than --config-file in priority but above the default config shipped with
    /// nextest.
    ///
    /// Arguments are specified in the format "tool:abs_path", for example
    /// "my-tool:/path/to/nextest.toml" (or "my-tool:C:\\path\\to\\nextest.toml" on Windows).
    /// Paths must be absolute.
    ///
    /// This argument may be specified multiple times. Files that come later are lower priority
    /// than those that come earlier.
    #[arg(long = "tool-config-file", global = true, value_name = "TOOL:ABS_PATH")]
    pub tool_config_files: Vec<ToolConfigFile>,
}

impl ConfigOpts {
    /// Creates a nextest config with the given options.
    pub fn make_config(
        &self,
        workspace_root: &Utf8Path,
        graph: &PackageGraph,
    ) -> Result<NextestConfig> {
        NextestConfig::from_sources(
            workspace_root,
            graph,
            self.config_file.as_deref(),
            &self.tool_config_files,
        )
        .map_err(ExpectedError::config_parse_error)
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List tests in workspace
    ///
    /// This command builds test binaries and queries them for the tests they contain.
    ///
    /// Use --verbose to get more information about tests, including test binary paths and skipped
    /// tests.
    ///
    /// Use --message-format json to get machine-readable output.
    ///
    /// For more information, see <https://nexte.st/book/listing>.
    List {
        #[clap(flatten)]
        cargo_options: CargoOptions,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        /// Output format
        #[arg(
            short = 'T',
            long,
            value_enum,
            default_value_t,
            help_heading = "Output options",
            value_name = "FMT"
        )]
        message_format: MessageFormatOpts,

        /// Type of listing
        #[arg(
            long,
            value_enum,
            default_value_t,
            help_heading = "Output options",
            value_name = "TYPE"
        )]
        list_type: ListType,

        #[clap(flatten)]
        reuse_build: ReuseBuildOpts,
    },
    /// Build and run tests
    ///
    /// This command builds test binaries and queries them for the tests they contain,
    /// then runs each test in parallel.
    ///
    /// For more information, see <https://nexte.st/book/running>.
    Run {
        /// Nextest profile to use
        #[arg(long, short = 'P', env = "NEXTEST_PROFILE")]
        profile: Option<String>,

        /// Run tests serially and do not capture output
        #[arg(
            long,
            name = "no-capture",
            alias = "nocapture",
            help_heading = "Runner options",
            display_order = 100
        )]
        no_capture: bool,

        #[clap(flatten)]
        cargo_options: CargoOptions,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        #[clap(flatten)]
        runner_opts: TestRunnerOpts,

        #[clap(flatten)]
        reporter_opts: TestReporterOpts,

        #[clap(flatten)]
        reuse_build: ReuseBuildOpts,
    },
    /// Build and archive tests
    ///
    /// This command builds test binaries and archives them to a file. The archive can then be
    /// transferred to another machine, and tests within it can be run with `cargo nextest run
    /// --archive-file`.
    ///
    /// The archive is a tarball compressed with Zstandard (.tar.zst).
    Archive {
        #[clap(flatten)]
        cargo_options: CargoOptions,

        /// File to write archive to
        #[arg(
            long,
            name = "archive-file",
            help_heading = "Archive options",
            value_name = "PATH"
        )]
        archive_file: Utf8PathBuf,

        /// Archive format
        ///
        /// `auto` uses the file extension to determine the archive format. Currently supported is
        /// `.tar.zst`.
        #[arg(
            long,
            value_enum,
            help_heading = "Archive options",
            value_name = "FORMAT",
            default_value_t
        )]
        archive_format: ArchiveFormatOpt,

        /// Zstandard compression level (-7 to 22, higher is more compressed + slower)
        #[arg(
            long,
            help_heading = "Archive options",
            value_name = "LEVEL",
            default_value_t = 0,
            allow_negative_numbers = true
        )]
        zstd_level: i32,
        // ReuseBuildOpts, while it can theoretically work, is way too confusing so skip it.
    },
    /// Manage the nextest installation
    #[clap(name = "self")]
    Self_ {
        #[clap(subcommand)]
        command: SelfCommand,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum PlatformFilterOpts {
    Target,
    Host,
    Any,
}

impl Default for PlatformFilterOpts {
    fn default() -> Self {
        Self::Any
    }
}

impl From<PlatformFilterOpts> for Option<BuildPlatform> {
    fn from(opt: PlatformFilterOpts) -> Self {
        match opt {
            PlatformFilterOpts::Target => Some(BuildPlatform::Target),
            PlatformFilterOpts::Host => Some(BuildPlatform::Host),
            PlatformFilterOpts::Any => None,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ListType {
    Full,
    BinariesOnly,
}

impl Default for ListType {
    fn default() -> Self {
        Self::Full
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum MessageFormatOpts {
    Human,
    Json,
    JsonPretty,
}

impl MessageFormatOpts {
    fn to_output_format(self, verbose: bool) -> OutputFormat {
        match self {
            Self::Human => OutputFormat::Human { verbose },
            Self::Json => OutputFormat::Serializable(SerializableFormat::Json),
            Self::JsonPretty => OutputFormat::Serializable(SerializableFormat::JsonPretty),
        }
    }
}

impl Default for MessageFormatOpts {
    fn default() -> Self {
        Self::Human
    }
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Filter options")]
struct TestBuildFilter {
    /// Run ignored tests
    #[arg(long, value_enum, value_name = "WHICH")]
    run_ignored: Option<RunIgnoredOpt>,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[arg(long)]
    partition: Option<PartitionerBuilder>,

    /// Filter test binaries by build platform (DEPRECATED)
    ///
    /// Instead, use -E with 'platform(host)' or 'platform(target)'.
    #[arg(
        long,
        hide_short_help = true,
        value_enum,
        value_name = "PLATFORM",
        default_value_t
    )]
    pub(crate) platform_filter: PlatformFilterOpts,

    /// Test filter expression (see {n}<https://nexte.st/book/filter-expressions>)
    #[arg(
        long,
        short = 'E',
        value_name = "EXPRESSION",
        action(ArgAction::Append)
    )]
    filter_expr: Vec<String>,

    // TODO: add regex-based filtering in the future?
    /// Test name filter
    #[arg(name = "FILTERS", help_heading = None)]
    filter: Vec<String>,

    /// Emulated cargo test binary arguments (partially supported)
    #[arg(help_heading = None, value_name = "TEST-BINARY-ARGS", last = true)]
    test_binary_args: Vec<String>,
}

impl TestBuildFilter {
    fn compute_test_list<'g>(
        &self,
        ctx: &TestExecuteContext<'_>,
        graph: &'g PackageGraph,
        binary_list: Arc<BinaryList>,
        test_filter_builder: TestFilterBuilder,
        env: EnvironmentMap,
        reuse_build: &ReuseBuildInfo,
    ) -> Result<TestList<'g>> {
        let path_mapper = make_path_mapper(
            reuse_build,
            graph,
            &binary_list.rust_build_meta.target_directory,
        )?;

        let rust_build_meta = binary_list.rust_build_meta.map_paths(&path_mapper);
        let test_artifacts = RustTestArtifact::from_binary_list(
            graph,
            binary_list,
            &rust_build_meta,
            &path_mapper,
            self.platform_filter.into(),
        )?;
        TestList::new(
            ctx,
            test_artifacts,
            rust_build_meta,
            &test_filter_builder,
            env,
            // TODO: do we need to allow customizing this?
            get_num_cpus(),
        )
        .map_err(|err| ExpectedError::CreateTestListError { err })
    }

    fn make_test_filter_builder(
        &self,
        filter_exprs: Vec<FilteringExpr>,
    ) -> Result<TestFilterBuilder> {
        // Merge the test binary args into the patterns.
        let mut run_ignored = self.run_ignored.map(Into::into);
        let mut patterns = self.filter.clone();
        self.merge_test_binary_args(&mut run_ignored, &mut patterns)?;

        Ok(TestFilterBuilder::new(
            run_ignored.unwrap_or_default(),
            self.partition.clone(),
            &patterns,
            filter_exprs,
        ))
    }

    fn merge_test_binary_args(
        &self,
        run_ignored: &mut Option<RunIgnored>,
        patterns: &mut Vec<String>,
    ) -> Result<()> {
        let mut ignore_filters = Vec::new();
        let mut read_trailing_filters = false;

        let mut skip_exact = Vec::new();
        let mut unsupported_args = Vec::new();

        patterns.extend(
            self.test_binary_args
                .iter()
                .filter(|&s| {
                    if read_trailing_filters || !s.starts_with('-') {
                        true
                    } else if s == "--include-ignored" {
                        ignore_filters.push((s.clone(), RunIgnored::All));
                        false
                    } else if s == "--ignored" {
                        ignore_filters.push((s.clone(), RunIgnored::IgnoredOnly));
                        false
                    } else if s == "--" {
                        read_trailing_filters = true;
                        false
                    } else if s == "--skip" || s == "--exact" {
                        skip_exact.push(s.clone());
                        false
                    } else {
                        unsupported_args.push(s.clone());
                        true
                    }
                })
                .cloned(),
        );

        for (s, f) in ignore_filters {
            if let Some(run_ignored) = run_ignored {
                if *run_ignored != f {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "mutually exclusive",
                        vec![s],
                    ));
                } else {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "duplicated",
                        vec![s],
                    ));
                }
            } else {
                *run_ignored = Some(f);
            }
        }

        if !skip_exact.is_empty() {
            return Err(ExpectedError::test_binary_args_parse_error(
                "unsupported\n(hint: use a filter expression instead: <https://nexte.st/book/filter-expressions>)",
                skip_exact,
            ));
        }

        if !unsupported_args.is_empty() {
            return Err(ExpectedError::test_binary_args_parse_error(
                "unsupported",
                unsupported_args,
            ));
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum RunIgnoredOpt {
    Default,
    IgnoredOnly,
    All,
}

impl From<RunIgnoredOpt> for RunIgnored {
    fn from(opt: RunIgnoredOpt) -> Self {
        match opt {
            RunIgnoredOpt::Default => RunIgnored::Default,
            RunIgnoredOpt::IgnoredOnly => RunIgnored::IgnoredOnly,
            RunIgnoredOpt::All => RunIgnored::All,
        }
    }
}

impl CargoOptions {
    fn compute_binary_list(
        &self,
        graph: &PackageGraph,
        manifest_path: Option<&Utf8Path>,
        output: OutputContext,
        target_triple: Option<TargetTriple>,
    ) -> Result<BinaryList> {
        // Don't use the manifest path from the graph to ensure that if the user cd's into a
        // particular crate and runs cargo nextest, then it behaves identically to cargo test.
        let mut cargo_cli = CargoCli::new("test", manifest_path, output);

        // Only build tests in the cargo test invocation, do not run them.
        cargo_cli.add_args(["--no-run", "--message-format", "json-render-diagnostics"]);
        cargo_cli.add_options(self);

        let expression = cargo_cli.to_expression();
        let output = expression
            .stdout_capture()
            .unchecked()
            .run()
            .map_err(|err| ExpectedError::build_exec_failed(cargo_cli.all_args(), err))?;
        if !output.status.success() {
            return Err(ExpectedError::build_failed(
                cargo_cli.all_args(),
                output.status.code(),
            ));
        }

        let test_binaries =
            BinaryList::from_messages(Cursor::new(output.stdout), graph, target_triple)?;
        Ok(test_binaries)
    }
}

/// Test runner options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Runner options")]
pub struct TestRunnerOpts {
    /// Compile, but don't run tests
    #[arg(long, name = "no-run")]
    no_run: bool,

    /// Number of tests to run simultaneously [possible values: integer or "num-cpus"]
    /// [default: from profile]
    #[arg(
        long,
        short = 'j',
        visible_alias = "jobs",
        value_name = "THREADS",
        conflicts_with_all = &["no-capture", "no-run"],
        env = "NEXTEST_TEST_THREADS",
        allow_negative_numbers = true,
    )]
    test_threads: Option<TestThreads>,

    /// Number of retries for failing tests [default: from profile]
    #[arg(long, env = "NEXTEST_RETRIES", conflicts_with = "no-run")]
    retries: Option<usize>,

    /// Cancel test run on the first failure
    #[arg(long, name = "fail-fast", conflicts_with = "no-run")]
    fail_fast: bool,

    /// Run all tests regardless of failure
    #[arg(long, conflicts_with = "no-run", overrides_with = "fail-fast")]
    no_fail_fast: bool,
}

impl TestRunnerOpts {
    fn to_builder(&self, no_capture: bool) -> Option<TestRunnerBuilder> {
        if self.no_run {
            return None;
        }

        let mut builder = TestRunnerBuilder::default();
        builder.set_no_capture(no_capture);
        if let Some(retries) = self.retries {
            builder.set_retries(RetryPolicy::new_without_delay(retries));
        }
        if self.no_fail_fast {
            builder.set_fail_fast(false);
        } else if self.fail_fast {
            builder.set_fail_fast(true);
        }
        if let Some(test_threads) = self.test_threads {
            builder.set_test_threads(test_threads);
        }

        Some(builder)
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum IgnoreOverridesOpt {
    Retries,
    All,
}

#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Reporter options")]
struct TestReporterOpts {
    /// Output stdout and stderr on failure
    #[arg(
        long,
        value_enum,
        conflicts_with_all = &["no-capture", "no-run"],
        value_name = "WHEN",
        env = "NEXTEST_FAILURE_OUTPUT",
    )]
    failure_output: Option<TestOutputDisplayOpt>,

    /// Output stdout and stderr on success
    #[arg(
        long,
        value_enum,
        conflicts_with_all = &["no-capture", "no-run"],
        value_name = "WHEN",
        env = "NEXTEST_SUCCESS_OUTPUT",
    )]
    success_output: Option<TestOutputDisplayOpt>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output
    #[arg(
        long,
        value_enum,
        conflicts_with = "no-run",
        value_name = "LEVEL",
        env = "NEXTEST_STATUS_LEVEL"
    )]
    status_level: Option<StatusLevelOpt>,

    /// Test statuses to output at the end of the run.
    #[arg(
        long,
        value_enum,
        conflicts_with = "no-run",
        value_name = "LEVEL",
        env = "NEXTEST_FINAL_STATUS_LEVEL"
    )]
    final_status_level: Option<FinalStatusLevelOpt>,

    /// Do not display the progress bar
    #[arg(long, env = "NEXTEST_HIDE_PROGRESS_BAR")]
    hide_progress_bar: bool,
}

impl TestReporterOpts {
    fn to_builder(&self, no_capture: bool) -> TestReporterBuilder {
        let mut builder = TestReporterBuilder::default();
        builder.set_no_capture(no_capture);
        if let Some(failure_output) = self.failure_output {
            builder.set_failure_output(failure_output.into());
        }
        if let Some(success_output) = self.success_output {
            builder.set_success_output(success_output.into());
        }
        if let Some(status_level) = self.status_level {
            builder.set_status_level(status_level.into());
        }
        if let Some(final_status_level) = self.final_status_level {
            builder.set_final_status_level(final_status_level.into());
        }
        builder.set_hide_progress_bar(self.hide_progress_bar);
        builder
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TestOutputDisplayOpt {
    Immediate,
    ImmediateFinal,
    Final,
    Never,
}

impl From<TestOutputDisplayOpt> for TestOutputDisplay {
    fn from(opt: TestOutputDisplayOpt) -> Self {
        match opt {
            TestOutputDisplayOpt::Immediate => TestOutputDisplay::Immediate,
            TestOutputDisplayOpt::ImmediateFinal => TestOutputDisplay::ImmediateFinal,
            TestOutputDisplayOpt::Final => TestOutputDisplay::Final,
            TestOutputDisplayOpt::Never => TestOutputDisplay::Never,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum StatusLevelOpt {
    None,
    Fail,
    Retry,
    Slow,
    Leak,
    Pass,
    Skip,
    All,
}

impl From<StatusLevelOpt> for StatusLevel {
    fn from(opt: StatusLevelOpt) -> Self {
        match opt {
            StatusLevelOpt::None => StatusLevel::None,
            StatusLevelOpt::Fail => StatusLevel::Fail,
            StatusLevelOpt::Retry => StatusLevel::Retry,
            StatusLevelOpt::Slow => StatusLevel::Slow,
            StatusLevelOpt::Leak => StatusLevel::Leak,
            StatusLevelOpt::Pass => StatusLevel::Pass,
            StatusLevelOpt::Skip => StatusLevel::Skip,
            StatusLevelOpt::All => StatusLevel::All,
        }
    }
}

/// This is copied from `FinalStatusLevel` except it also has a retry option.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FinalStatusLevelOpt {
    None,
    Fail,
    #[clap(alias = "retry")]
    Flaky,
    Slow,
    Skip,
    Pass,
    All,
}

impl From<FinalStatusLevelOpt> for FinalStatusLevel {
    fn from(opt: FinalStatusLevelOpt) -> FinalStatusLevel {
        match opt {
            FinalStatusLevelOpt::None => FinalStatusLevel::None,
            FinalStatusLevelOpt::Fail => FinalStatusLevel::Fail,
            FinalStatusLevelOpt::Flaky => FinalStatusLevel::Flaky,
            FinalStatusLevelOpt::Slow => FinalStatusLevel::Slow,
            FinalStatusLevelOpt::Skip => FinalStatusLevel::Skip,
            FinalStatusLevelOpt::Pass => FinalStatusLevel::Pass,
            FinalStatusLevelOpt::All => FinalStatusLevel::All,
        }
    }
}

#[derive(Debug)]
struct BaseApp {
    output: OutputContext,
    graph_data: Arc<(String, PackageGraph)>,
    // Potentially remapped workspace root (might not be the same as the graph).
    workspace_root: Utf8PathBuf,
    manifest_path: Option<Utf8PathBuf>,
    reuse_build: ReuseBuildInfo,
    cargo_opts: CargoOptions,
    config_opts: ConfigOpts,

    cargo_configs: CargoConfigs,
    double_spawn: OnceCell<DoubleSpawnInfo>,
    target_runner: OnceCell<TargetRunner>,
}

impl BaseApp {
    fn new(
        output: OutputOpts,
        reuse_build: ReuseBuildOpts,
        cargo_opts: CargoOptions,
        config_opts: ConfigOpts,
        manifest_path: Option<Utf8PathBuf>,
        graph_with_deps: bool,
        writer: &mut OutputWriter,
    ) -> Result<Self> {
        let output = output.init();
        reuse_build.check_experimental(output);

        let reuse_build = reuse_build.process(output, writer)?;

        let graph_data = match reuse_build.cargo_metadata() {
            Some(MetadataOrPath::Metadata(graph_data)) => graph_data.clone(),
            Some(MetadataOrPath::Path(path)) => {
                let json = std::fs::read_to_string(path).map_err(|err| {
                    ExpectedError::argument_file_read_error("cargo-metadata", path, err)
                })?;
                let graph = PackageGraph::from_json(&json).map_err(|err| {
                    ExpectedError::cargo_metadata_parse_error(Some(path.clone()), err)
                })?;
                Arc::new((json, graph))
            }
            None => {
                let json = acquire_graph_data(
                    manifest_path.as_deref(),
                    cargo_opts.target_dir.as_deref(),
                    output,
                    graph_with_deps,
                )?;
                let graph = PackageGraph::from_json(&json)
                    .map_err(|err| ExpectedError::cargo_metadata_parse_error(None, err))?;
                Arc::new((json, graph))
            }
        };

        let manifest_path = if reuse_build.cargo_metadata.is_some() {
            Some(graph_data.1.workspace().root().join("Cargo.toml"))
        } else {
            manifest_path
        };

        let workspace_root = match reuse_build.workspace_remap() {
            Some(path) => path.to_owned(),
            _ => graph_data.1.workspace().root().to_owned(),
        };

        let root_manifest_path = workspace_root.join("Cargo.toml");
        if !root_manifest_path.exists() {
            // This doesn't happen in normal use, but is a common situation if the build is being
            // reused.
            let reuse_build_kind = if reuse_build.workspace_remap().is_some() {
                ReuseBuildKind::ReuseWithWorkspaceRemap { workspace_root }
            } else if reuse_build.is_active() {
                ReuseBuildKind::Reuse
            } else {
                ReuseBuildKind::Normal
            };

            return Err(ExpectedError::RootManifestNotFound {
                path: root_manifest_path,
                reuse_build_kind,
            });
        }

        let cargo_configs = CargoConfigs::new(&cargo_opts.config)?;

        Ok(Self {
            output,
            graph_data,
            workspace_root,
            reuse_build,
            manifest_path,
            cargo_opts,
            config_opts,
            cargo_configs,

            double_spawn: OnceCell::new(),
            target_runner: OnceCell::new(),
        })
    }

    fn load_double_spawn(&self) -> &DoubleSpawnInfo {
        self.double_spawn.get_or_init(|| {
            if std::env::var("NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN") == Ok("1".to_owned()) {
                log::info!("using experimental double-spawn method for test processes");
                DoubleSpawnInfo::enabled()
            } else {
                DoubleSpawnInfo::disabled()
            }
        })
    }

    fn load_runner(&self, build_platforms: &BuildPlatforms) -> &TargetRunner {
        self.target_runner
            .get_or_init(|| runner_for_target(&self.cargo_configs, build_platforms))
    }

    fn exec_archive(
        &self,
        output_file: &Utf8Path,
        format: ArchiveFormatOpt,
        zstd_level: i32,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        // Do format detection first so we fail immediately.
        let format = format.to_archive_format(output_file)?;
        let binary_list = self.build_binary_list()?;
        let path_mapper = PathMapper::noop();

        let mut reporter = ArchiveReporter::new(self.output.verbose);
        if self
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr)
        {
            reporter.colorize();
        }

        let mut writer = output_writer.stderr_writer();
        archive_to_file(
            &binary_list,
            &self.graph_data.0,
            // Note that path_mapper is currently a no-op -- we don't support reusing builds for
            // archive creation because it's too confusing.
            &path_mapper,
            format,
            zstd_level,
            output_file,
            |event| {
                reporter.report_event(event, &mut writer)?;
                writer.flush()
            },
        )
        .map_err(|err| ExpectedError::ArchiveCreateError {
            archive_file: output_file.to_owned(),
            err,
        })?;

        Ok(())
    }

    fn build_binary_list(&self) -> Result<Arc<BinaryList>> {
        let binary_list = match self.reuse_build.binaries_metadata() {
            Some(MetadataOrPath::Metadata(binary_list)) => binary_list.clone(),
            Some(MetadataOrPath::Path(path)) => {
                let raw_binary_list = std::fs::read_to_string(path).map_err(|err| {
                    ExpectedError::argument_file_read_error("binaries-metadata", path, err)
                })?;
                let binary_list: BinaryListSummary = serde_json::from_str(&raw_binary_list)
                    .map_err(|err| {
                        ExpectedError::argument_json_parse_error("binaries-metadata", path, err)
                    })?;
                Arc::new(BinaryList::from_summary(binary_list)?)
            }
            None => {
                let target_triple =
                    discover_target_triple(&self.cargo_configs, self.cargo_opts.target.as_deref());
                Arc::new(self.cargo_opts.compute_binary_list(
                    self.graph(),
                    self.manifest_path.as_deref(),
                    self.output,
                    target_triple,
                )?)
            }
        };
        Ok(binary_list)
    }

    #[inline]
    fn graph(&self) -> &PackageGraph {
        &self.graph_data.1
    }
}

#[derive(Debug)]
struct App {
    base: BaseApp,
    build_filter: TestBuildFilter,
}

// (_output is not used, but must be passed in to ensure that the output is properly initialized
// before calling this method)
fn check_experimental_filtering(_output: OutputContext) {
    const EXPERIMENTAL_ENV: &str = "NEXTEST_EXPERIMENTAL_FILTER_EXPR";
    if std::env::var(EXPERIMENTAL_ENV).is_ok() {
        log::warn!("filter expressions are no longer experimental: NEXTEST_EXPERIMENTAL_FILTER_EXPR does not need to be set");
    }
}

impl App {
    fn new(base: BaseApp, build_filter: TestBuildFilter) -> Result<Self> {
        check_experimental_filtering(base.output);

        Ok(Self { base, build_filter })
    }

    fn build_filtering_expressions(&self) -> Result<Vec<FilteringExpr>> {
        let (exprs, all_errors): (Vec<_>, Vec<_>) = self
            .build_filter
            .filter_expr
            .iter()
            .map(|input| FilteringExpr::parse(input, self.base.graph()))
            .partition_result();

        if !all_errors.is_empty() {
            Err(ExpectedError::filter_expression_parse_error(all_errors))
        } else {
            Ok(exprs)
        }
    }

    fn build_test_list(
        &self,
        ctx: &TestExecuteContext<'_>,
        binary_list: Arc<BinaryList>,
        test_filter_builder: TestFilterBuilder,
    ) -> Result<TestList> {
        let env = EnvironmentMap::new(&self.base.cargo_configs);
        self.build_filter.compute_test_list(
            ctx,
            self.base.graph(),
            binary_list,
            test_filter_builder,
            env,
            &self.base.reuse_build,
        )
    }

    fn load_profile<'cfg>(
        &self,
        profile_name: Option<&str>,
        config: &'cfg NextestConfig,
    ) -> Result<NextestProfile<'cfg, PreBuildPlatform>> {
        let profile_name = profile_name.unwrap_or_else(|| {
            // The "official" way to detect a miri environment is with MIRI_SYSROOT.
            // https://github.com/rust-lang/miri/pull/2398#issuecomment-1190747685
            if std::env::var_os("MIRI_SYSROOT").is_some() {
                NextestConfig::DEFAULT_MIRI_PROFILE
            } else {
                NextestConfig::DEFAULT_PROFILE
            }
        });
        let profile = config
            .profile(profile_name)
            .map_err(ExpectedError::profile_not_found)?;
        let store_dir = profile.store_dir();
        std::fs::create_dir_all(store_dir).map_err(|err| ExpectedError::StoreDirCreateError {
            store_dir: store_dir.to_owned(),
            err,
        })?;
        Ok(profile)
    }

    fn exec_list(
        &self,
        message_format: MessageFormatOpts,
        list_type: ListType,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let filter_exprs = self.build_filtering_expressions()?;
        let test_filter_builder = self.build_filter.make_test_filter_builder(filter_exprs)?;

        let binary_list = self.base.build_binary_list()?;

        match list_type {
            ListType::BinariesOnly => {
                let mut writer = output_writer.stdout_writer();
                binary_list.write(
                    message_format.to_output_format(self.base.output.verbose),
                    &mut writer,
                    self.base
                        .output
                        .color
                        .should_colorize(supports_color::Stream::Stdout),
                )?;
                writer.flush().map_err(WriteTestListError::Io)?;
            }
            ListType::Full => {
                let double_spawn = self.base.load_double_spawn();
                let target_runner = self
                    .base
                    .load_runner(&binary_list.rust_build_meta.build_platforms()?);
                let ctx = TestExecuteContext {
                    double_spawn,
                    target_runner,
                };

                let test_list = self.build_test_list(&ctx, binary_list, test_filter_builder)?;

                let mut writer = output_writer.stdout_writer();
                test_list.write(
                    message_format.to_output_format(self.base.output.verbose),
                    &mut writer,
                    self.base
                        .output
                        .color
                        .should_colorize(supports_color::Stream::Stdout),
                )?;
                writer.flush().map_err(WriteTestListError::Io)?;
            }
        }
        Ok(())
    }

    fn exec_run(
        &self,
        profile_name: Option<&str>,
        no_capture: bool,
        runner_opts: &TestRunnerOpts,
        reporter_opts: &TestReporterOpts,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let config = self
            .base
            .config_opts
            .make_config(&self.base.workspace_root, self.base.graph())?;
        let profile = self.load_profile(profile_name, &config)?;

        let filter_exprs = self.build_filtering_expressions()?;
        let test_filter_builder = self.build_filter.make_test_filter_builder(filter_exprs)?;

        let binary_list = self.base.build_binary_list()?;
        let build_platforms = binary_list.rust_build_meta.build_platforms()?;
        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(&build_platforms);
        let ctx = TestExecuteContext {
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, test_filter_builder)?;

        let output = output_writer.reporter_output();
        let profile = profile.apply_build_platforms(&build_platforms);

        let mut reporter = reporter_opts
            .to_builder(no_capture)
            .set_verbose(self.base.output.verbose)
            .build(&test_list, &profile, output);
        if self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr)
        {
            reporter.colorize();
        }

        let handler = SignalHandlerKind::Standard;
        let runner_builder = match runner_opts.to_builder(no_capture) {
            Some(runner_builder) => runner_builder,
            None => {
                // This means --no-run was passed in. Exit.
                return Ok(());
            }
        };

        let runner = runner_builder.build(
            &test_list,
            profile,
            handler,
            double_spawn.clone(),
            target_runner.clone(),
        )?;

        configure_handle_inheritance(no_capture)?;
        let run_stats = runner.try_execute(|event| {
            // Write and flush the event.
            reporter.report_event(event)
        })?;
        if !run_stats.is_success() {
            return Err(ExpectedError::test_run_failed());
        }
        Ok(())
    }
}

#[derive(Debug, Subcommand)]
enum SelfCommand {
    #[cfg_attr(
        not(feature = "self-update"),
        doc = "This version of nextest does not have self-update enabled\n\
        \n\
        Always exits with code 93 (SELF_UPDATE_UNAVAILABLE).
        "
    )]
    #[cfg_attr(
        feature = "self-update",
        doc = "Download and install updates to nextest\n\
        \n\
        This command checks the internet for updates to nextest, then downloads and
        installs them if an update is available."
    )]
    Update {
        /// Version or version range to download
        #[arg(long, default_value = "latest")]
        version: String,

        /// Check for updates rather than downloading them
        ///
        /// If no update is available, exits with code 0. If an update is available, exits with code
        /// 80 (UPDATE_AVAILABLE).
        #[arg(short = 'n', long)]
        check: bool,

        /// Do not prompt for confirmation
        #[arg(short = 'y', long, conflicts_with = "check")]
        yes: bool,

        /// Force downgrades and reinstalls
        #[arg(short, long)]
        force: bool,

        /// URL to download releases.json from
        #[arg(long)]
        releases_url: Option<String>,
    },
}

impl SelfCommand {
    #[allow(unused_variables)]
    fn exec(self, output: OutputOpts) -> Result<i32> {
        let output = output.init();

        match self {
            Self::Update {
                version,
                check,
                yes,
                force,
                releases_url,
            } => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "self-update")] {
                        crate::update::perform_update(
                            &version,
                            check,
                            yes,
                            force,
                            releases_url,
                            output,
                        )
                    } else {
                        log::info!("this version of cargo-nextest cannot perform self-updates\n\
                                    (hint: this usually means nextest was installed by a package manager)");
                        Ok(nextest_metadata::NextestExitCode::SELF_UPDATE_UNAVAILABLE)
                    }
                }
            }
        }
    }
}

fn acquire_graph_data(
    manifest_path: Option<&Utf8Path>,
    target_dir: Option<&Utf8Path>,
    output: OutputContext,
    with_deps: bool,
) -> Result<String> {
    let mut cargo_cli = CargoCli::new("metadata", manifest_path, output);
    cargo_cli.add_args(["--format-version=1", "--all-features"]);

    if !with_deps {
        cargo_cli.add_arg("--no-deps");
    }

    let mut expression = cargo_cli.to_expression().stdout_capture().unchecked();
    // cargo metadata doesn't support "--target-dir" but setting the environment
    // variable works.
    if let Some(target_dir) = target_dir {
        expression = expression.env("CARGO_TARGET_DIR", target_dir);
    }
    // Capture stdout but not stderr.
    let output = expression
        .run()
        .map_err(|err| ExpectedError::cargo_metadata_exec_failed(cargo_cli.all_args(), err))?;
    if !output.status.success() {
        return Err(ExpectedError::cargo_metadata_failed(cargo_cli.all_args()));
    }

    let json = String::from_utf8(output.stdout).map_err(|error| {
        let io_error = std::io::Error::new(std::io::ErrorKind::InvalidData, error);
        ExpectedError::cargo_metadata_exec_failed(cargo_cli.all_args(), io_error)
    })?;
    Ok(json)
}

fn discover_target_triple(
    cargo_configs: &CargoConfigs,
    target_cli_option: Option<&str>,
) -> Option<TargetTriple> {
    match TargetTriple::find(cargo_configs, target_cli_option) {
        Ok(Some(triple)) => {
            log::debug!(
                "using target triple `{}` defined by `{}`",
                triple.platform.triple_str(),
                triple.source
            );
            Some(triple)
        }
        Ok(None) => {
            log::debug!("no target triple found, assuming no cross-compilation");
            None
        }
        Err(err) => {
            warn_on_err("target triple", &err).expect("writing to a string is infallible");
            None
        }
    }
}

fn runner_for_target(
    cargo_configs: &CargoConfigs,
    build_platforms: &BuildPlatforms,
) -> TargetRunner {
    match TargetRunner::new(cargo_configs, build_platforms) {
        Ok(runner) => {
            if build_platforms.target.is_some() {
                if let Some(runner) = runner.target() {
                    log_platform_runner("for the target platform, ", runner);
                }
                if let Some(runner) = runner.host() {
                    log_platform_runner("for the host platform, ", runner);
                }
            } else {
                // If triple is None, then the host and target platforms use the same runner if
                // any.
                if let Some(runner) = runner.target() {
                    log_platform_runner("", runner);
                }
            }
            runner
        }
        Err(err) => {
            warn_on_err("target runner", &err).expect("writing to a string is infallible");
            TargetRunner::empty()
        }
    }
}

fn log_platform_runner(prefix: &str, runner: &PlatformRunner) {
    let runner_command = shell_words::join(std::iter::once(runner.binary()).chain(runner.args()));
    log::info!(
        "{prefix}using target runner `{}` defined by {}",
        runner_command.if_supports_color(Stream::Stderr, |s| s.bold()),
        runner.source()
    )
}

fn warn_on_err(thing: &str, err: &(dyn std::error::Error)) -> Result<(), std::fmt::Error> {
    let mut s = String::with_capacity(256);
    write!(s, "could not determine {thing}: {}", err)?;
    let mut next_error = err.source();
    while let Some(err) = next_error {
        write!(
            s,
            "\n{} {}",
            "caused by:"
                .if_supports_color(Stream::Stderr, |s| s.style(Style::new().bold().yellow())),
            err
        )?;
        next_error = err.source();
    }

    log::warn!("{}", s);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argument_parsing() {
        use clap::error::ErrorKind::{self, *};

        let valid: &[&'static str] = &[
            // ---
            // Basic commands
            // ---
            "cargo nextest list",
            "cargo nextest run",
            // ---
            // Commands with arguments
            // ---
            "cargo nextest list --list-type binaries-only",
            "cargo nextest list --list-type full",
            "cargo nextest list --message-format json-pretty",
            "cargo nextest run --failure-output never",
            "cargo nextest run --success-output=immediate",
            "cargo nextest run --status-level=all",
            "cargo nextest run --no-capture",
            "cargo nextest run --nocapture",
            "cargo nextest run --no-run",
            "cargo nextest run --final-status-level flaky",
            // retry is an alias for flaky -- ensure that it parses
            "cargo nextest run --final-status-level retry",
            // ---
            // Cargo options
            // ---
            "cargo nextest list --lib --bins",
            "cargo nextest run --ignore-rust-version --unit-graph",
            // ---
            // Reuse build options
            // ---
            "cargo nextest list --binaries-metadata=foo",
            "cargo nextest run --binaries-metadata=foo --target-dir-remap=bar",
            "cargo nextest list --cargo-metadata path",
            "cargo nextest run --cargo-metadata=path --workspace-remap remapped-path",
            "cargo nextest archive --archive-file my-archive.tar.zst --zstd-level -1",
            "cargo nextest archive --archive-file my-archive.foo --archive-format tar-zst",
            "cargo nextest archive --archive-file my-archive.foo --archive-format tar-zstd",
            "cargo nextest list --archive-file my-archive.tar.zst",
            "cargo nextest list --archive-file my-archive.tar.zst --archive-format tar-zst",
            "cargo nextest list --archive-file my-archive.tar.zst --extract-to my-path",
            "cargo nextest list --archive-file my-archive.tar.zst --extract-to my-path --extract-overwrite",
            "cargo nextest list --archive-file my-archive.tar.zst --persist-extract-tempdir",
            "cargo nextest list --archive-file my-archive.tar.zst --workspace-remap foo",
            "cargo nextest list --archive-file my-archive.tar.zst --config target.'cfg(all())'.runner=\"my-runner\"",
            // ---
            // Filter expressions
            // ---
            "cargo nextest list -E deps(foo)",
            "cargo nextest run --filter-expr 'test(bar)' --package=my-package test-filter",
            // ---
            // Test binary arguments
            // ---
            "cargo nextest run -- --a an arbitrary arg",
            // Test negative test threads
            "cargo nextest run --jobs -3",
            "cargo nextest run --jobs 3",
        ];

        let invalid: &[(&'static str, ErrorKind)] = &[
            // ---
            // --no-capture and these options conflict
            // ---
            (
                "cargo nextest run --no-capture --test-threads=24",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-capture --failure-output=never",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-capture --success-output=final",
                ArgumentConflict,
            ),
            // ---
            // --no-run and these options conflict
            // ---
            ("cargo nextest run --no-run -j8", ArgumentConflict),
            ("cargo nextest run --no-run --retries 3", ArgumentConflict),
            ("cargo nextest run --no-run --fail-fast", ArgumentConflict),
            (
                "cargo nextest run --no-run --no-fail-fast",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-run --failure-output immediate",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-run --success-output never",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-run --status-level pass",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --no-run --final-status-level skip",
                ArgumentConflict,
            ),
            // ---
            // Reuse build options conflict with cargo options
            // ---
            (
                // NOTE: cargo nextest --manifest-path foo run --cargo-metadata bar is currently
                // accepted. This is a bug: https://github.com/clap-rs/clap/issues/1204
                "cargo nextest run --manifest-path foo --cargo-metadata bar",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --binaries-metadata=foo --lib",
                ArgumentConflict,
            ),
            // ---
            // workspace-remap requires cargo-metadata
            // ---
            (
                "cargo nextest run --workspace-remap foo",
                MissingRequiredArgument,
            ),
            // ---
            // target-dir-remap requires binaries-metadata
            // ---
            (
                "cargo nextest run --target-dir-remap bar",
                MissingRequiredArgument,
            ),
            // ---
            // Archive options
            // ---
            (
                "cargo nextest run --archive-format tar-zst",
                MissingRequiredArgument,
            ),
            (
                "cargo nextest run --archive-file foo --archive-format no",
                InvalidValue,
            ),
            (
                "cargo nextest run --extract-to foo",
                MissingRequiredArgument,
            ),
            (
                "cargo nextest run --archive-file foo --extract-overwrite",
                MissingRequiredArgument,
            ),
            (
                "cargo nextest run --extract-to foo --extract-overwrite",
                MissingRequiredArgument,
            ),
            (
                "cargo nextest run --persist-extract-tempdir",
                MissingRequiredArgument,
            ),
            (
                "cargo nextest run --archive-file foo --extract-to bar --persist-extract-tempdir",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --archive-file foo --cargo-metadata bar",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --archive-file foo --binaries-metadata bar",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --archive-file foo --target-dir-remap bar",
                ArgumentConflict,
            ),
            // Invalid test threads: 0
            ("cargo nextest run --jobs 0", ValueValidation),
        ];

        // Unset all NEXTEST_ env vars because they can conflict with the try_parse_from below.
        for (k, _) in std::env::vars() {
            if k.starts_with("NEXTEST_") {
                std::env::remove_var(k);
            }
        }

        for valid_args in valid {
            if let Err(error) = CargoNextestApp::try_parse_from(
                shell_words::split(valid_args).expect("valid command line"),
            ) {
                panic!(
                    "{} should have successfully parsed, but didn't: {}",
                    valid_args, error
                );
            }
        }

        for &(invalid_args, kind) in invalid {
            match CargoNextestApp::try_parse_from(
                shell_words::split(invalid_args).expect("valid command"),
            ) {
                Ok(_) => {
                    panic!(
                        "{} should have errored out but successfully parsed",
                        invalid_args
                    );
                }
                Err(error) => {
                    let actual_kind = error.kind();
                    if kind != actual_kind {
                        panic!(
                            "{} should error with kind {kind:?}, but actual kind was {actual_kind:?}",
                            invalid_args,
                        );
                    }
                }
            }
        }
    }

    #[derive(Debug, Parser)]
    struct TestCli {
        #[structopt(flatten)]
        build_filter: TestBuildFilter,
    }

    #[test]
    fn test_test_binary_argument_parsing() {
        fn get_test_filter_builder(cmd: &str) -> Result<TestFilterBuilder> {
            let app = TestCli::try_parse_from(shell_words::split(cmd).expect("valid command line"))
                .unwrap_or_else(|_| panic!("{} should have successfully parsed", cmd));
            app.build_filter.make_test_filter_builder(vec![])
        }

        let valid = &[
            // ---
            // substring filter
            // ---
            ("foo -- str1", "foo str1"),
            ("foo -- str2 str3", "foo str2 str3"),
            // ---
            // ignored
            // ---
            ("foo -- --ignored", "foo --run-ignored ignored-only"),
            ("foo -- --include-ignored", "foo --run-ignored all"),
            // ---
            // two escapes
            // ---
            (
                "foo -- --ignored -- str --- --ignored",
                "foo --run-ignored ignored-only str -- -- --- --ignored",
            ),
            ("foo -- -- str1 str2 --", "foo str1 str2 -- -- --"),
        ];
        let invalid = &[
            // ---
            // duplicated
            // ---
            ("foo -- --include-ignored --include-ignored", "duplicated"),
            ("foo -- --ignored --ignored", "duplicated"),
            // ---
            // mutually exclusive
            // ---
            ("foo -- --ignored --include-ignored", "mutually exclusive"),
            ("foo --run-ignored all -- --ignored", "mutually exclusive"),
            // ---
            // unsupported
            // ---
            ("foo -- --bar", "unsupported"),
            ("foo -- --exact", "unsupported\n(hint: use a filter expression instead: <https://nexte.st/book/filter-expressions>)"),
            ("foo -- --skip", "unsupported\n(hint: use a filter expression instead: <https://nexte.st/book/filter-expressions>)"),
        ];

        for (a, b) in valid {
            let a_str = format!(
                "{:?}",
                get_test_filter_builder(a).unwrap_or_else(|_| panic!("failed to parse {}", a))
            );
            let b_str = format!(
                "{:?}",
                get_test_filter_builder(b).unwrap_or_else(|_| panic!("failed to parse {}", b))
            );
            assert_eq!(a_str, b_str);
        }

        for (s, r) in invalid {
            let res = get_test_filter_builder(s);
            if let Err(ExpectedError::TestBinaryArgsParseError { reason, .. }) = &res {
                assert_eq!(reason, r);
            } else {
                panic!(
                    "{s} should have errored out with TestBinaryArgsParseError, actual: {res:?}",
                );
            }
        }
    }
}
