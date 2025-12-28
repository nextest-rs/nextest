// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CLI argument parsing structures and enums.

use crate::{
    ExpectedError, Result,
    cargo_cli::{CargoCli, CargoOptions},
    output::OutputContext,
    reuse_build::{ArchiveFormatOpt, ReuseBuildOpts, make_path_mapper},
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, Args, Subcommand, ValueEnum, builder::BoolishValueParser};
use guppy::graph::PackageGraph;
use nextest_filtering::ParseContext;
use nextest_metadata::BuildPlatform;
use nextest_runner::{
    cargo_config::EnvironmentMap,
    config::{
        core::{
            ConfigExperimental, EvaluatableProfile, NextestConfig, ToolConfigFile,
            VersionOnlyConfig, get_num_cpus,
        },
        elements::{MaxFail, RetryPolicy, TestThreads},
    },
    list::{
        BinaryList, OutputFormat, RustTestArtifact, SerializableFormat, TestExecuteContext,
        TestList,
    },
    partition::PartitionerBuilder,
    platform::BuildPlatforms,
    reporter::{
        FinalStatusLevel, MaxProgressRunning, ReporterBuilder, ShowProgress, StatusLevel,
        TestOutputDisplay,
    },
    reuse_build::ReuseBuildInfo,
    run_mode::NextestRunMode,
    runner::{
        DebuggerCommand, Interceptor, StressCondition, StressCount, TestRunnerBuilder,
        TracerCommand,
    },
    test_filter::{FilterBound, RunIgnored, TestFilterBuilder, TestFilterPatterns},
    test_output::CaptureStrategy,
};
use std::{collections::BTreeSet, io::Cursor, sync::Arc, time::Duration};
use tracing::{debug, warn};

// Options shared between cargo nextest and cargo ntr.
#[derive(Debug, Args)]
pub(super) struct CommonOpts {
    /// Path to Cargo.toml
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Manifest options"
    )]
    pub(super) manifest_path: Option<Utf8PathBuf>,

    #[clap(flatten)]
    pub(super) output: crate::output::OutputOpts,

    #[clap(flatten)]
    pub(super) config_opts: ConfigOpts,
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Config options")]
pub(super) struct ConfigOpts {
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

    /// Override checks for the minimum version defined in nextest's config.
    ///
    /// Repository and tool-specific configuration files can specify minimum required and
    /// recommended versions of nextest. This option overrides those checks.
    #[arg(long, global = true)]
    pub override_version_check: bool,

    /// The nextest profile to use.
    ///
    /// Nextest's configuration supports multiple profiles, which can be used to set up different
    /// configurations for different purposes. (For example, a configuration for local runs and one
    /// for CI.) This option selects the profile to use.
    #[arg(
        long,
        short = 'P',
        env = "NEXTEST_PROFILE",
        global = true,
        help_heading = "Config options"
    )]
    pub(super) profile: Option<String>,
}

impl ConfigOpts {
    /// Creates a nextest version-only config with the given options.
    pub(super) fn make_version_only_config(
        &self,
        workspace_root: &Utf8Path,
    ) -> Result<VersionOnlyConfig> {
        VersionOnlyConfig::from_sources(
            workspace_root,
            self.config_file.as_deref(),
            &self.tool_config_files,
        )
        .map_err(ExpectedError::config_parse_error)
    }

    /// Creates a nextest config with the given options.
    pub(super) fn make_config(
        &self,
        workspace_root: &Utf8Path,
        pcx: &ParseContext<'_>,
        experimental: &BTreeSet<ConfigExperimental>,
    ) -> Result<NextestConfig> {
        NextestConfig::from_sources(
            workspace_root,
            pcx,
            self.config_file.as_deref(),
            &self.tool_config_files,
            experimental,
        )
        .map_err(ExpectedError::config_parse_error)
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// List tests in workspace
    ///
    /// This command builds test binaries and queries them for the tests they contain.
    ///
    /// Use --verbose to get more information about tests, including test binary paths and skipped
    /// tests.
    ///
    /// Use --message-format json to get machine-readable output.
    ///
    /// For more information, see <https://nexte.st/docs/listing>.
    List(Box<ListOpts>),
    /// Build and run tests
    ///
    /// This command builds test binaries and queries them for the tests they contain,
    /// then runs each test in parallel.
    ///
    /// For more information, see <https://nexte.st/docs/running>.
    #[command(visible_alias = "r")]
    Run(Box<RunOpts>),
    /// Build and run benchmarks (experimental)
    ///
    /// This command builds benchmark binaries and queries them for the benchmarks they contain,
    /// then runs each benchmark **serially**.
    ///
    /// This is an experimental feature. To enable it, set the environment variable
    /// `NEXTEST_EXPERIMENTAL_BENCHMARKS=1`.
    #[command(visible_alias = "b")]
    Bench(Box<BenchOpts>),
    /// Build and archive tests
    ///
    /// This command builds test binaries and archives them to a file. The archive can then be
    /// transferred to another machine, and tests within it can be run with `cargo nextest run
    /// --archive-file`.
    ///
    /// The archive is a tarball compressed with Zstandard (.tar.zst).
    Archive(Box<ArchiveOpts>),
    /// Show information about nextest's configuration in this workspace.
    ///
    /// This command shows configuration information about nextest, including overrides applied to
    /// individual tests.
    ///
    /// In the future, this will show more information about configurations and overrides.
    ShowConfig {
        #[clap(subcommand)]
        command: super::commands::ShowConfigCommand,
    },
    /// Manage the nextest installation
    #[clap(name = "self")]
    Self_ {
        #[clap(subcommand)]
        command: super::commands::SelfCommand,
    },
    /// Debug commands
    ///
    /// The commands in this section are for nextest's own developers and those integrating with it
    /// to debug issues. They are not part of the public API and may change at any time.
    #[clap(hide = true)]
    Debug {
        #[clap(subcommand)]
        command: super::commands::DebugCommand,
    },
}

#[derive(Debug, Args)]
pub(super) struct ArchiveOpts {
    #[clap(flatten)]
    pub(super) cargo_options: CargoOptions,

    /// File to write archive to
    #[arg(
        long,
        name = "archive-file",
        help_heading = "Archive options",
        value_name = "PATH"
    )]
    pub(super) archive_file: Utf8PathBuf,

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
    pub(super) archive_format: ArchiveFormatOpt,

    #[clap(flatten)]
    pub(super) archive_build_filter: ArchiveBuildFilter,

    /// Zstandard compression level (-7 to 22, higher is more compressed + slower)
    #[arg(
        long,
        help_heading = "Archive options",
        value_name = "LEVEL",
        default_value_t = 0,
        allow_negative_numbers = true
    )]
    pub(super) zstd_level: i32,
    // ReuseBuildOpts, while it can theoretically work, is way too confusing so skip it.
}

#[derive(Debug, Args)]
pub(super) struct ListOpts {
    #[clap(flatten)]
    pub(super) cargo_options: CargoOptions,

    #[clap(flatten)]
    pub(super) build_filter: TestBuildFilter,

    /// Output format
    #[arg(
        short = 'T',
        long,
        value_enum,
        default_value_t,
        help_heading = "Output options",
        value_name = "FMT"
    )]
    pub(super) message_format: MessageFormatOpts,

    /// Type of listing
    #[arg(
        long,
        value_enum,
        default_value_t,
        help_heading = "Output options",
        value_name = "TYPE"
    )]
    pub(super) list_type: ListType,

    #[clap(flatten)]
    pub(super) reuse_build: ReuseBuildOpts,
}

#[derive(Debug, Args)]
pub(super) struct RunOpts {
    #[clap(flatten)]
    pub(super) cargo_options: CargoOptions,

    #[clap(flatten)]
    pub(super) build_filter: TestBuildFilter,

    #[clap(flatten)]
    pub(super) runner_opts: TestRunnerOpts,

    /// Run tests serially and do not capture output
    #[arg(
        long,
        name = "no-capture",
        alias = "nocapture",
        help_heading = "Runner options",
        display_order = 100
    )]
    pub(super) no_capture: bool,

    #[clap(flatten)]
    pub(super) reporter_opts: ReporterOpts,

    #[clap(flatten)]
    pub(super) reuse_build: ReuseBuildOpts,
}

#[derive(Debug, Args)]
pub(super) struct BenchOpts {
    #[clap(flatten)]
    pub(super) cargo_options: CargoOptions,

    #[clap(flatten)]
    pub(super) build_filter: TestBuildFilter,

    #[clap(flatten)]
    pub(super) runner_opts: BenchRunnerOpts,

    /// Run benchmarks serially and do not capture output (always enabled).
    ///
    /// Benchmarks in nextest always run serially, so this flag is kept only for
    /// compatibility and has no effect.
    #[arg(
        long,
        name = "no-capture",
        alias = "nocapture",
        help_heading = "Runner options",
        display_order = 100
    )]
    pub(super) no_capture: bool,

    #[clap(flatten)]
    pub(super) reporter_opts: BenchReporterOpts,
    // Note: no reuse_build for benchmarks since archive extraction is not supported
}

/// Benchmark runner options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Runner options")]
pub(super) struct BenchRunnerOpts {
    /// Compile, but don't run benchmarks.
    #[arg(long, name = "no-run")]
    pub(super) no_run: bool,

    /// Cancel benchmark run on the first failure.
    #[arg(
        long,
        visible_alias = "ff",
        name = "fail-fast",
        // TODO: It would be nice to warn rather than error if fail-fast is used
        // with no-run, so that this matches the other options like
        // test-threads. But there seem to be issues with that: clap 4.5 doesn't
        // appear to like `Option<bool>` very much. With `ArgAction::SetTrue` it
        // always sets the value to false or true rather than leaving it unset.
        conflicts_with = "no-run"
    )]
    fail_fast: bool,

    /// Run all benchmarks regardless of failure.
    #[arg(
        long,
        visible_alias = "nff",
        name = "no-fail-fast",
        conflicts_with = "no-run",
        overrides_with = "fail-fast"
    )]
    no_fail_fast: bool,

    /// Number of benchmarks that can fail before exiting run [possible
    /// values: integer or "all"]
    #[arg(
        long,
        name = "max-fail",
        value_name = "N",
        conflicts_with_all = &["no-run", "fail-fast", "no-fail-fast"],
    )]
    max_fail: Option<MaxFail>,

    /// Behavior if there are no benchmarks to run [default: fail]
    #[arg(long, value_enum, value_name = "ACTION", env = "NEXTEST_NO_TESTS")]
    pub(super) no_tests: Option<NoTestsBehavior>,

    /// Stress testing options.
    #[clap(flatten)]
    pub(super) stress: StressOptions,

    #[clap(flatten)]
    pub(super) interceptor: InterceptorOpt,
}

impl BenchRunnerOpts {
    pub(super) fn to_builder(&self, cap_strat: CaptureStrategy) -> Option<TestRunnerBuilder> {
        if self.no_tests.is_some() && self.no_run {
            warn!("ignoring --no-tests because --no-run is specified");
        }

        // ---

        if self.no_run {
            return None;
        }
        let mut builder = TestRunnerBuilder::default();
        builder.set_capture_strategy(cap_strat);
        if let Some(max_fail) = self.max_fail {
            builder.set_max_fail(max_fail);
            debug!(max_fail = ?max_fail, "set max fail");
        } else if self.no_fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(false));
            debug!("set max fail via from_fail_fast(false)");
        } else if self.fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(true));
            debug!("set max fail via from_fail_fast(true)");
        }

        // Benchmarks always run serially and use 1 test thread.
        builder.set_test_threads(TestThreads::Count(1));

        if let Some(condition) = self.stress.condition.as_ref() {
            builder.set_stress_condition(condition.stress_condition());
        }

        builder.set_interceptor(self.interceptor.to_interceptor());

        Some(builder)
    }
}

/// Benchmark reporter options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Reporter options")]
pub(super) struct BenchReporterOpts {
    /// Show nextest progress in the specified manner.
    ///
    /// For benchmarks, the default is "counter" which shows the benchmark index
    /// (e.g., "(1/10)") but no progress bar.
    #[arg(long, env = "NEXTEST_SHOW_PROGRESS")]
    show_progress: Option<ShowProgressOpt>,

    /// Disable handling of input keys from the terminal.
    ///
    /// By default, when running a terminal, nextest accepts the `t` key to dump
    /// test information. This flag disables that behavior.
    #[arg(long, env = "NEXTEST_NO_INPUT_HANDLER", value_parser = BoolishValueParser::new())]
    pub(super) no_input_handler: bool,
}

impl BenchReporterOpts {
    pub(super) fn to_builder(&self, should_colorize: bool) -> ReporterBuilder {
        let mut builder = ReporterBuilder::default();
        builder.set_no_capture(true);
        builder.set_colorize(should_colorize);
        let show_progress = self.show_progress.unwrap_or(ShowProgressOpt::Auto);
        builder.set_show_progress(show_progress.into());
        builder
    }
}

#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(crate) enum PlatformFilterOpts {
    Target,
    Host,
    #[default]
    Any,
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

#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(super) enum ListType {
    #[default]
    Full,
    BinariesOnly,
}

#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(super) enum MessageFormatOpts {
    /// Auto-detect: **human** if stdout is an interactive terminal, **oneline**
    /// otherwise.
    #[default]
    Auto,
    /// A human-readable output format.
    Human,
    /// One test per line.
    Oneline,
    /// JSON with no whitespace.
    Json,
    /// JSON, prettified.
    JsonPretty,
}

impl MessageFormatOpts {
    pub(super) fn to_output_format(self, verbose: bool, is_terminal: bool) -> OutputFormat {
        match self {
            Self::Auto => {
                if is_terminal {
                    OutputFormat::Human { verbose }
                } else {
                    OutputFormat::Oneline { verbose }
                }
            }
            Self::Human => OutputFormat::Human { verbose },
            Self::Oneline => OutputFormat::Oneline { verbose },
            Self::Json => OutputFormat::Serializable(SerializableFormat::Json),
            Self::JsonPretty => OutputFormat::Serializable(SerializableFormat::JsonPretty),
        }
    }
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Filter options")]
pub(super) struct TestBuildFilter {
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

    /// Test filterset (see {n}<https://nexte.st/docs/filtersets>).
    #[arg(
        long,
        alias = "filter-expr",
        short = 'E',
        value_name = "EXPR",
        action(ArgAction::Append)
    )]
    pub(super) filterset: Vec<String>,

    /// Ignore the default filter configured in the profile.
    ///
    /// By default, all filtersets are intersected with the default filter configured in the
    /// profile. This flag disables that behavior.
    ///
    /// This flag doesn't change the definition of the `default()` filterset.
    #[arg(long)]
    ignore_default_filter: bool,

    /// Test name filters.
    #[arg(help_heading = None, name = "FILTERS")]
    pre_double_dash_filters: Vec<String>,

    /// Test name filters and emulated test binary arguments.
    ///
    /// Supported arguments:{n}
    /// - --ignored:         Only run ignored tests{n}
    /// - --include-ignored: Run both ignored and non-ignored tests{n}
    /// - --skip PATTERN:    Skip tests that match the pattern{n}
    /// - --exact:           Run tests that exactly match patterns after `--`
    #[arg(help_heading = None, value_name = "FILTERS_AND_ARGS", last = true)]
    filters: Vec<String>,
}

impl TestBuildFilter {
    #[expect(clippy::too_many_arguments)]
    pub(super) fn compute_test_list<'g>(
        &self,
        ctx: &TestExecuteContext<'_>,
        graph: &'g PackageGraph,
        workspace_root: Utf8PathBuf,
        binary_list: Arc<BinaryList>,
        test_filter_builder: TestFilterBuilder,
        env: EnvironmentMap,
        profile: &EvaluatableProfile<'_>,
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
            workspace_root,
            env,
            profile,
            if self.ignore_default_filter {
                FilterBound::All
            } else {
                FilterBound::DefaultSet
            },
            // TODO: do we need to allow customizing this?
            get_num_cpus(),
        )
        .map_err(|err| ExpectedError::CreateTestListError { err })
    }

    pub(super) fn make_test_filter_builder(
        &self,
        mode: NextestRunMode,
        filter_exprs: Vec<nextest_filtering::Filterset>,
    ) -> Result<TestFilterBuilder> {
        // Merge the test binary args into the patterns.
        let mut run_ignored = self.run_ignored.map(Into::into);
        let mut patterns = TestFilterPatterns::new(self.pre_double_dash_filters.clone());
        self.merge_test_binary_args(&mut run_ignored, &mut patterns)?;

        Ok(TestFilterBuilder::new(
            mode,
            run_ignored.unwrap_or_default(),
            self.partition.clone(),
            patterns,
            filter_exprs,
        )?)
    }

    fn merge_test_binary_args(
        &self,
        run_ignored: &mut Option<RunIgnored>,
        patterns: &mut TestFilterPatterns,
    ) -> Result<()> {
        // First scan to see if `--exact` is specified. If so, then everything here will be added to
        // `--exact`.
        let mut is_exact = false;
        for arg in &self.filters {
            if arg == "--" {
                break;
            }
            if arg == "--exact" {
                if is_exact {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "duplicated",
                        vec![arg.clone()],
                    ));
                }
                is_exact = true;
            }
        }

        let mut ignore_filters = Vec::new();
        let mut read_trailing_filters = false;

        let mut unsupported_args = Vec::new();

        let mut it = self.filters.iter();
        while let Some(arg) = it.next() {
            if read_trailing_filters || !arg.starts_with('-') {
                if is_exact {
                    patterns.add_exact_pattern(arg.clone());
                } else {
                    patterns.add_substring_pattern(arg.clone());
                }
            } else if arg == "--include-ignored" {
                ignore_filters.push((arg.clone(), RunIgnored::All));
            } else if arg == "--ignored" {
                ignore_filters.push((arg.clone(), RunIgnored::Only));
            } else if arg == "--" {
                read_trailing_filters = true;
            } else if arg == "--skip" {
                let skip_arg = it.next().ok_or_else(|| {
                    ExpectedError::test_binary_args_parse_error(
                        "missing required argument",
                        vec![arg.clone()],
                    )
                })?;

                if is_exact {
                    patterns.add_skip_exact_pattern(skip_arg.clone());
                } else {
                    patterns.add_skip_pattern(skip_arg.clone());
                }
            } else if arg == "--exact" {
                // Already handled above.
            } else {
                unsupported_args.push(arg.clone());
            }
        }

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

        if !unsupported_args.is_empty() {
            return Err(ExpectedError::test_binary_args_parse_error(
                "unsupported",
                unsupported_args,
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Filter options")]
pub(super) struct ArchiveBuildFilter {
    /// Archive filterset (see <https://nexte.st/docs/filtersets>).
    ///
    /// This argument does not accept test predicates.
    #[arg(long, short = 'E', value_name = "EXPR", action(ArgAction::Append))]
    pub(super) filterset: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum RunIgnoredOpt {
    /// Run non-ignored tests.
    Default,

    /// Run ignored tests.
    #[clap(alias = "ignored-only")]
    Only,

    /// Run both ignored and non-ignored tests.
    All,
}

impl From<RunIgnoredOpt> for RunIgnored {
    fn from(opt: RunIgnoredOpt) -> Self {
        match opt {
            RunIgnoredOpt::Default => RunIgnored::Default,
            RunIgnoredOpt::Only => RunIgnored::Only,
            RunIgnoredOpt::All => RunIgnored::All,
        }
    }
}

impl CargoOptions {
    pub(super) fn compute_binary_list(
        &self,
        cargo_command: &str,
        graph: &PackageGraph,
        manifest_path: Option<&Utf8Path>,
        output: OutputContext,
        build_platforms: BuildPlatforms,
    ) -> Result<BinaryList> {
        // Don't use the manifest path from the graph to ensure that if the user cd's into a
        // particular crate and runs cargo nextest, then it behaves identically to cargo test.
        let mut cargo_cli = CargoCli::new(cargo_command, manifest_path, output);

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
            BinaryList::from_messages(Cursor::new(output.stdout), graph, build_platforms)?;
        Ok(test_binaries)
    }
}

/// Test runner options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Runner options")]
pub struct TestRunnerOpts {
    /// Compile, but don't run tests
    #[arg(long, name = "no-run")]
    pub(super) no_run: bool,

    /// Number of tests to run simultaneously [possible values: integer or "num-cpus"]
    /// [default: from profile]
    #[arg(
        long,
        short = 'j',
        visible_alias = "jobs",
        value_name = "N",
        env = "NEXTEST_TEST_THREADS",
        allow_negative_numbers = true
    )]
    test_threads: Option<TestThreads>,

    /// Number of retries for failing tests [default: from profile]
    #[arg(long, env = "NEXTEST_RETRIES", value_name = "N")]
    retries: Option<u32>,

    /// Cancel test run on the first failure
    #[arg(
        long,
        visible_alias = "ff",
        name = "fail-fast",
        // TODO: It would be nice to warn rather than error if fail-fast is used
        // with no-run, so that this matches the other options like
        // test-threads. But there seem to be issues with that: clap 4.5 doesn't
        // appear to like `Option<bool>` very much. With `ArgAction::SetTrue` it
        // always sets the value to false or true rather than leaving it unset.
        conflicts_with = "no-run"
    )]
    fail_fast: bool,

    /// Run all tests regardless of failure
    #[arg(
        long,
        visible_alias = "nff",
        name = "no-fail-fast",
        conflicts_with = "no-run",
        overrides_with = "fail-fast"
    )]
    no_fail_fast: bool,

    /// Number of tests that can fail before exiting test run
    ///
    /// To control whether currently running tests are waited for or terminated
    /// immediately, append ':wait' (default) or ':immediate' to the number
    /// (e.g., '5:immediate').
    ///
    /// [possible values: integer, "all", "N:wait", "N:immediate"]
    #[arg(
        long,
        name = "max-fail",
        value_name = "N[:MODE]",
        conflicts_with_all = &["no-run", "fail-fast", "no-fail-fast"],
    )]
    max_fail: Option<MaxFail>,

    /// Interceptor options (debugger or tracer)
    #[clap(flatten)]
    pub(super) interceptor: InterceptorOpt,

    /// Behavior if there are no tests to run [default: fail]
    #[arg(long, value_enum, value_name = "ACTION", env = "NEXTEST_NO_TESTS")]
    pub(super) no_tests: Option<NoTestsBehavior>,

    /// Stress testing options
    #[clap(flatten)]
    pub(super) stress: StressOptions,
}

#[derive(Debug, Default, Args)]
#[group(id = "interceptor", multiple = false)]
pub(super) struct InterceptorOpt {
    /// Debug a single test using a text-based or graphical debugger.
    ///
    /// Debugger mode automatically:
    ///
    /// - disables timeouts
    /// - disables output capture
    /// - passes standard input through to the debugger
    ///
    /// Example: `--debugger "rust-gdb --args"`
    #[arg(long, value_name = "DEBUGGER", conflicts_with_all = ["stress_condition", "no-run"])]
    pub(super) debugger: Option<DebuggerCommand>,

    /// Trace a single test using a syscall tracer like `strace` or `truss`.
    ///
    /// Tracer mode automatically:
    ///
    /// - disables timeouts
    /// - disables output capture
    ///
    /// Unlike `--debugger`, tracers do not need stdin passthrough or special signal handling.
    ///
    /// Example: `--tracer "strace -tt"`
    #[arg(long, value_name = "TRACER", conflicts_with_all = ["stress_condition", "no-run"])]
    pub(super) tracer: Option<TracerCommand>,
}

impl InterceptorOpt {
    /// Returns true if either a debugger or a tracer is active.
    pub(super) fn is_active(&self) -> bool {
        self.debugger.is_some() || self.tracer.is_some()
    }

    /// Converts to an [`Interceptor`] enum.
    pub(super) fn to_interceptor(&self) -> Interceptor {
        match (&self.debugger, &self.tracer) {
            (Some(debugger), None) => Interceptor::Debugger(debugger.clone()),
            (None, Some(tracer)) => Interceptor::Tracer(tracer.clone()),
            (None, None) => Interceptor::None,
            (Some(_), Some(_)) => {
                unreachable!("clap group ensures debugger and tracer are mutually exclusive")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum NoTestsBehavior {
    /// Silently exit with code 0.
    Pass,

    /// Produce a warning and exit with code 0.
    Warn,

    /// Produce an error message and exit with code 4.
    #[clap(alias = "error")]
    Fail,
}

impl TestRunnerOpts {
    pub(super) fn to_builder(&self, cap_strat: CaptureStrategy) -> Option<TestRunnerBuilder> {
        // Warn on conflicts between options. This is a warning and not an error
        // because these options can be specified via environment variables as
        // well.
        if self.test_threads.is_some()
            && let Some(reasons) =
                no_run_no_capture_reasons(self.no_run, cap_strat == CaptureStrategy::None)
        {
            warn!("ignoring --test-threads because {reasons}");
        }

        if self.retries.is_some() && self.no_run {
            warn!("ignoring --retries because --no-run is specified");
        }
        if self.no_tests.is_some() && self.no_run {
            warn!("ignoring --no-tests because --no-run is specified");
        }

        // ---

        if self.no_run {
            return None;
        }

        let mut builder = TestRunnerBuilder::default();
        builder.set_capture_strategy(cap_strat);
        if let Some(retries) = self.retries {
            builder.set_retries(RetryPolicy::new_without_delay(retries));
        }

        if let Some(max_fail) = self.max_fail {
            builder.set_max_fail(max_fail);
            debug!(max_fail = ?max_fail, "set max fail");
        } else if self.no_fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(false));
            debug!("set max fail via from_fail_fast(false)");
        } else if self.fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(true));
            debug!("set max fail via from_fail_fast(true)");
        }

        if let Some(test_threads) = self.test_threads {
            builder.set_test_threads(test_threads);
        }

        if let Some(condition) = self.stress.condition.as_ref() {
            builder.set_stress_condition(condition.stress_condition());
        }

        builder.set_interceptor(self.interceptor.to_interceptor());

        Some(builder)
    }
}

fn no_run_no_capture_reasons(no_run: bool, no_capture: bool) -> Option<&'static str> {
    match (no_run, no_capture) {
        (true, true) => Some("--no-run and --no-capture are specified"),
        (true, false) => Some("--no-run is specified"),
        (false, true) => Some("--no-capture is specified"),
        (false, false) => None,
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum IgnoreOverridesOpt {
    Retries,
    All,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub(super) enum MessageFormat {
    /// The default output format.
    #[default]
    Human,
    /// Output test information in the same format as libtest.
    LibtestJson,
    /// Output test information in the same format as libtest, with a `nextest` subobject that
    /// includes additional metadata.
    LibtestJsonPlus,
}

#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Stress testing options")]
pub(super) struct StressOptions {
    /// Stress testing condition.
    #[clap(flatten)]
    pub(super) condition: Option<StressConditionOpt>,
    // TODO: modes other than serial
}

#[derive(Clone, Debug, Default, Args)]
#[group(id = "stress_condition", multiple = false)]
pub(super) struct StressConditionOpt {
    /// The number of times to run each test, or `infinite` to run indefinitely.
    #[arg(long, value_name = "COUNT")]
    stress_count: Option<StressCount>,

    /// How long to run stress tests until (e.g. 24h).
    #[arg(long, value_name = "DURATION", value_parser = non_zero_duration)]
    stress_duration: Option<Duration>,
}

impl StressConditionOpt {
    fn stress_condition(&self) -> StressCondition {
        if let Some(count) = self.stress_count {
            StressCondition::Count(count)
        } else if let Some(duration) = self.stress_duration {
            StressCondition::Duration(duration)
        } else {
            unreachable!(
                "if StressOptions::condition is Some, \
                 one of these should be set"
            )
        }
    }
}

fn non_zero_duration(input: &str) -> std::result::Result<Duration, String> {
    let duration = humantime::parse_duration(input).map_err(|error| error.to_string())?;
    if duration.is_zero() {
        Err("duration must be non-zero".to_string())
    } else {
        Ok(duration)
    }
}

#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Reporter options")]
pub(super) struct ReporterOpts {
    /// Output stdout and stderr on failure
    #[arg(long, value_enum, value_name = "WHEN", env = "NEXTEST_FAILURE_OUTPUT")]
    failure_output: Option<TestOutputDisplayOpt>,

    /// Output stdout and stderr on success
    #[arg(long, value_enum, value_name = "WHEN", env = "NEXTEST_SUCCESS_OUTPUT")]
    success_output: Option<TestOutputDisplayOpt>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output
    #[arg(long, value_enum, value_name = "LEVEL", env = "NEXTEST_STATUS_LEVEL")]
    status_level: Option<StatusLevelOpt>,

    /// Test statuses to output at the end of the run.
    #[arg(
        long,
        value_enum,
        value_name = "LEVEL",
        env = "NEXTEST_FINAL_STATUS_LEVEL"
    )]
    final_status_level: Option<FinalStatusLevelOpt>,

    /// Show nextest progress in the specified manner.
    #[arg(long, env = "NEXTEST_SHOW_PROGRESS")]
    show_progress: Option<ShowProgressOpt>,

    /// Do not display the progress bar. Deprecated, use **--show-progress** instead.
    #[arg(long, env = "NEXTEST_HIDE_PROGRESS_BAR", value_parser = BoolishValueParser::new())]
    hide_progress_bar: bool,

    /// Do not indent captured test output.
    ///
    /// By default, test output produced by **--failure-output** and
    /// **--success-output** is indented for visual clarity. This flag disables
    /// that behavior.
    ///
    /// This option has no effect with **--no-capture**, since that passes
    /// through standard output and standard error.
    #[arg(long, env = "NEXTEST_NO_OUTPUT_INDENT", value_parser = BoolishValueParser::new())]
    no_output_indent: bool,

    /// Disable handling of input keys from the terminal.
    ///
    /// By default, when running a terminal, nextest accepts the `t` key to dump
    /// test information. This flag disables that behavior.
    #[arg(long, env = "NEXTEST_NO_INPUT_HANDLER", value_parser = BoolishValueParser::new())]
    pub(super) no_input_handler: bool,

    /// Maximum number of running tests to display progress for.
    ///
    /// When more tests are running than this limit, the progress bar will show
    /// the first N tests and a summary of remaining tests (e.g. "... and 24
    /// more tests running"). Set to **0** to hide running tests, or
    /// **infinite** for unlimited. This applies when using
    /// `--show-progress=bar` or `--show-progress=only`.
    #[arg(
        long = "max-progress-running",
        value_name = "N",
        env = "NEXTEST_MAX_PROGRESS_RUNNING",
        default_value = "8"
    )]
    max_progress_running: MaxProgressRunning,

    /// Format to use for test results (experimental).
    #[arg(
        long,
        name = "message-format",
        value_enum,
        value_name = "FORMAT",
        env = "NEXTEST_MESSAGE_FORMAT"
    )]
    pub(super) message_format: Option<MessageFormat>,

    /// Version of structured message-format to use (experimental).
    ///
    /// This allows the machine-readable formats to use a stable structure for consistent
    /// consumption across changes to nextest. If not specified, the latest version is used.
    #[arg(
        long,
        requires = "message-format",
        value_name = "VERSION",
        env = "NEXTEST_MESSAGE_FORMAT_VERSION"
    )]
    pub(super) message_format_version: Option<String>,
}

impl ReporterOpts {
    pub(super) fn to_builder(
        &self,
        no_run: bool,
        no_capture: bool,
        should_colorize: bool,
    ) -> ReporterBuilder {
        // Warn on conflicts between options. This is a warning and not an error
        // because these options can be specified via environment variables as
        // well.
        if no_run && no_capture {
            warn!("ignoring --no-capture because --no-run is specified");
        }

        let reasons = no_run_no_capture_reasons(no_run, no_capture);

        if self.failure_output.is_some()
            && let Some(reasons) = reasons
        {
            warn!("ignoring --failure-output because {}", reasons);
        }
        if self.success_output.is_some()
            && let Some(reasons) = reasons
        {
            warn!("ignoring --success-output because {}", reasons);
        }
        if self.status_level.is_some() && no_run {
            warn!("ignoring --status-level because --no-run is specified");
        }
        if self.final_status_level.is_some() && no_run {
            warn!("ignoring --final-status-level because --no-run is specified");
        }
        if self.message_format.is_some() && no_run {
            warn!("ignoring --message-format because --no-run is specified");
        }
        if self.message_format_version.is_some() && no_run {
            warn!("ignoring --message-format-version because --no-run is specified");
        }

        let show_progress = match (self.show_progress, self.hide_progress_bar) {
            (Some(show_progress), true) => {
                warn!("ignoring --hide-progress-bar because --show-progress is specified");
                show_progress
            }
            (Some(show_progress), false) => show_progress,
            (None, true) => ShowProgressOpt::None,
            (None, false) => ShowProgressOpt::default(),
        };

        // ---

        let mut builder = ReporterBuilder::default();
        builder.set_no_capture(no_capture);
        builder.set_colorize(should_colorize);

        if let Some(ShowProgressOpt::Only) = self.show_progress {
            // --show-progress=only implies --status-level=slow and
            // --final-status-level=none. But we allow overriding these options
            // explicitly as well.
            builder.set_status_level(StatusLevel::Slow);
            builder.set_final_status_level(FinalStatusLevel::None);
        }
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
        builder.set_show_progress(show_progress.into());
        builder.set_no_output_indent(self.no_output_indent);
        builder.set_max_progress_running(self.max_progress_running);
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
    fn from(opt: FinalStatusLevelOpt) -> Self {
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

#[derive(Default, Clone, Copy, Debug, ValueEnum)]
enum ShowProgressOpt {
    /// Automatically choose the best progress display based on whether nextest
    /// is running in an interactive terminal.
    #[default]
    Auto,

    /// Do not display a progress bar or counter.
    None,

    /// Display a progress bar with running tests: default for interactive
    /// terminals.
    #[clap(alias = "running")]
    Bar,

    /// Display a counter next to each completed test.
    Counter,

    /// Display a progress bar with running tests, and hide successful test
    /// output; equivalent to `--show-progress=running --status-level=slow
    /// --final-status-level=none`.
    Only,
}

impl From<ShowProgressOpt> for ShowProgress {
    fn from(opt: ShowProgressOpt) -> Self {
        match opt {
            ShowProgressOpt::Auto => ShowProgress::Auto,
            ShowProgressOpt::None => ShowProgress::None,
            ShowProgressOpt::Bar => ShowProgress::Running,
            ShowProgressOpt::Counter => ShowProgress::Counter,
            ShowProgressOpt::Only => ShowProgress::Running,
        }
    }
}

/// A next-generation test runner for Rust.
///
/// This binary should typically be invoked as `cargo nextest` (in which case
/// this message will not be seen), not `cargo-nextest`.
#[derive(Debug, clap::Parser)]
#[command(
    version = crate::version::short(),
    long_version = crate::version::long(),
    bin_name = "cargo",
    styles = crate::output::clap_styles::style(),
    max_term_width = 100,
)]
pub struct CargoNextestApp {
    #[clap(subcommand)]
    subcommand: NextestSubcommand,
}

impl CargoNextestApp {
    /// Initializes the output context.
    pub fn init_output(&self) -> OutputContext {
        match &self.subcommand {
            NextestSubcommand::Nextest(args) => args.common.output.init(),
            NextestSubcommand::Ntr(args) => args.common.output.init(),
            #[cfg(unix)]
            // Double-spawned processes should never use coloring.
            NextestSubcommand::DoubleSpawn(_) => OutputContext::color_never_init(),
        }
    }

    /// Executes the app.
    pub fn exec(
        self,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        if let Err(err) = nextest_runner::usdt::register_probes() {
            tracing::warn!("failed to register USDT probes: {}", err);
        }

        match self.subcommand {
            NextestSubcommand::Nextest(app) => app.exec(cli_args, output, output_writer),
            NextestSubcommand::Ntr(opts) => opts.exec(cli_args, output, output_writer),
            #[cfg(unix)]
            NextestSubcommand::DoubleSpawn(opts) => opts.exec(output),
        }
    }
}

#[derive(Debug, Subcommand)]
enum NextestSubcommand {
    /// A next-generation test runner for Rust. <https://nexte.st>
    Nextest(Box<AppOpts>),
    /// Build and run tests: a shortcut for `cargo nextest run`.
    Ntr(Box<NtrOpts>),
    /// Private command, used to double-spawn test processes.
    #[cfg(unix)]
    #[command(name = nextest_runner::double_spawn::DoubleSpawnInfo::SUBCOMMAND_NAME, hide = true)]
    DoubleSpawn(crate::double_spawn::DoubleSpawnOpts),
}

#[derive(Debug, Args)]
#[clap(
    version = crate::version::short(),
    long_version = crate::version::long(),
    display_name = "cargo-nextest",
)]
pub(super) struct AppOpts {
    #[clap(flatten)]
    common: CommonOpts,

    #[clap(subcommand)]
    command: Command,
}

impl AppOpts {
    /// Execute the command.
    ///
    /// Returns the exit code.
    fn exec(
        self,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        match self.command {
            Command::List(list_opts) => {
                let base = super::execution::BaseApp::new(
                    output,
                    list_opts.reuse_build,
                    list_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = super::execution::App::new(base, list_opts.build_filter)?;
                app.exec_list(list_opts.message_format, list_opts.list_type, output_writer)?;
                Ok(0)
            }
            Command::Run(run_opts) => {
                let base = super::execution::BaseApp::new(
                    output,
                    run_opts.reuse_build,
                    run_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = super::execution::App::new(base, run_opts.build_filter)?;
                app.exec_run(
                    run_opts.no_capture,
                    &run_opts.runner_opts,
                    &run_opts.reporter_opts,
                    cli_args,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::Bench(bench_opts) => {
                let base = super::execution::BaseApp::new(
                    output,
                    ReuseBuildOpts::default(),
                    bench_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = super::execution::App::new(base, bench_opts.build_filter)?;
                app.exec_bench(
                    &bench_opts.runner_opts,
                    &bench_opts.reporter_opts,
                    cli_args,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::Archive(archive_opts) => {
                let app = super::execution::BaseApp::new(
                    output,
                    ReuseBuildOpts::default(),
                    archive_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;

                let app =
                    super::execution::ArchiveApp::new(app, archive_opts.archive_build_filter)?;
                app.exec_archive(
                    &archive_opts.archive_file,
                    archive_opts.archive_format,
                    archive_opts.zstd_level,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::ShowConfig { command } => command.exec(
                self.common.manifest_path,
                self.common.config_opts,
                output,
                output_writer,
            ),
            Command::Self_ { command } => command.exec(self.common.output),
            Command::Debug { command } => command.exec(self.common.output),
        }
    }
}

#[derive(Debug, Args)]
struct NtrOpts {
    #[clap(flatten)]
    common: CommonOpts,

    #[clap(flatten)]
    run_opts: RunOpts,
}

impl NtrOpts {
    fn exec(
        self,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        let base = super::execution::BaseApp::new(
            output,
            self.run_opts.reuse_build,
            self.run_opts.cargo_options,
            self.common.config_opts,
            self.common.manifest_path,
            output_writer,
        )?;
        let app = super::execution::App::new(base, self.run_opts.build_filter)?;
        app.exec_run(
            self.run_opts.no_capture,
            &self.run_opts.runner_opts,
            &self.run_opts.reporter_opts,
            cli_args,
            output_writer,
        )?;
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use nextest_runner::run_mode::NextestRunMode;

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
            "cargo nextest list --message-format oneline",
            "cargo nextest list --message-format auto",
            "cargo nextest list -T oneline",
            "cargo nextest list -T auto",
            "cargo nextest run --failure-output never",
            "cargo nextest run --success-output=immediate",
            "cargo nextest run --status-level=all",
            "cargo nextest run --no-capture",
            "cargo nextest run --nocapture",
            "cargo nextest run --no-run",
            "cargo nextest run --final-status-level flaky",
            "cargo nextest run --max-fail 3",
            "cargo nextest run --max-fail=all",
            // retry is an alias for flaky -- ensure that it parses
            "cargo nextest run --final-status-level retry",
            "NEXTEST_HIDE_PROGRESS_BAR=1 cargo nextest run",
            "NEXTEST_HIDE_PROGRESS_BAR=true cargo nextest run",
            // ---
            // --no-run conflicts that produce warnings rather than errors
            // ---
            "cargo nextest run --no-run -j8",
            "cargo nextest run --no-run --retries 3",
            "NEXTEST_TEST_THREADS=8 cargo nextest run --no-run",
            "cargo nextest run --no-run --success-output never",
            "NEXTEST_SUCCESS_OUTPUT=never cargo nextest run --no-run",
            "cargo nextest run --no-run --failure-output immediate",
            "NEXTEST_FAILURE_OUTPUT=immediate cargo nextest run --no-run",
            "cargo nextest run --no-run --status-level pass",
            "NEXTEST_STATUS_LEVEL=pass cargo nextest run --no-run",
            "cargo nextest run --no-run --final-status-level skip",
            "NEXTEST_FINAL_STATUS_LEVEL=skip cargo nextest run --no-run",
            // ---
            // --no-capture conflicts that produce warnings rather than errors
            // ---
            "cargo nextest run --no-capture --test-threads=24",
            "NEXTEST_NO_CAPTURE=1 cargo nextest run --test-threads=24",
            "cargo nextest run --no-capture --failure-output=never",
            "NEXTEST_NO_CAPTURE=1 cargo nextest run --failure-output=never",
            "cargo nextest run --no-capture --success-output=final",
            "NEXTEST_SUCCESS_OUTPUT=final cargo nextest run --no-capture",
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
            // Filtersets
            // ---
            "cargo nextest list -E deps(foo)",
            "cargo nextest run --filterset 'test(bar)' --package=my-package test-filter",
            "cargo nextest run --filter-expr 'test(bar)' --package=my-package test-filter",
            "cargo nextest list -E 'deps(foo)' --ignore-default-filter",
            // ---
            // Stress test options
            // ---
            "cargo nextest run --stress-count 4",
            "cargo nextest run --stress-count infinite",
            "cargo nextest run --stress-duration 60m",
            "cargo nextest run --stress-duration 24h",
            // ---
            // Test binary arguments
            // ---
            "cargo nextest run -- --a an arbitrary arg",
            // Test negative test threads
            "cargo nextest run --jobs -3",
            "cargo nextest run --jobs 3",
            // Test negative cargo build jobs
            "cargo nextest run --build-jobs -1",
            "cargo nextest run --build-jobs 1",
            // ---
            // Bench command
            // ---
            "cargo nextest bench",
            "cargo nextest bench --no-run",
            "cargo nextest bench --fail-fast",
            "cargo nextest bench --no-fail-fast",
            "cargo nextest bench --max-fail 3",
            "cargo nextest bench --max-fail=all",
            "cargo nextest bench --stress-count 4",
            "cargo nextest bench --stress-count infinite",
            "cargo nextest bench --stress-duration 60m",
            "cargo nextest bench --debugger gdb",
            "cargo nextest bench --tracer strace",
        ];

        let invalid: &[(&'static str, ErrorKind)] = &[
            // ---
            // --no-run and these options conflict
            // ---
            ("cargo nextest run --no-run --fail-fast", ArgumentConflict),
            (
                "cargo nextest run --no-run --no-fail-fast",
                ArgumentConflict,
            ),
            ("cargo nextest run --no-run --max-fail=3", ArgumentConflict),
            // ---
            // --max-fail and these options conflict
            // ---
            (
                "cargo nextest run --max-fail=3 --no-fail-fast",
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
            // Test threads must be a number
            ("cargo nextest run --jobs -twenty", UnknownArgument),
            ("cargo nextest run --build-jobs -inf1", UnknownArgument),
            // Invalid stress count: 0
            ("cargo nextest run --stress-count 0", ValueValidation),
            // Invalid stress duration: 0
            ("cargo nextest run --stress-duration 0m", ValueValidation),
            // ---
            // --debugger conflicts with stress testing and --no-run
            // ---
            (
                "cargo nextest run --debugger gdb --stress-count 4",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --debugger gdb --stress-duration 1h",
                ArgumentConflict,
            ),
            (
                "cargo nextest run --debugger gdb --no-run",
                ArgumentConflict,
            ),
            // ---
            // Bench command conflicts
            // ---
            ("cargo nextest bench --no-run --fail-fast", ArgumentConflict),
            (
                "cargo nextest bench --no-run --no-fail-fast",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --no-run --max-fail=3",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --max-fail=3 --no-fail-fast",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --debugger gdb --stress-count 4",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --debugger gdb --stress-duration 1h",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --debugger gdb --no-run",
                ArgumentConflict,
            ),
            (
                "cargo nextest bench --tracer strace --stress-count 4",
                ArgumentConflict,
            ),
            // Invalid stress count: 0
            ("cargo nextest bench --stress-count 0", ValueValidation),
            // Invalid stress duration: 0
            ("cargo nextest bench --stress-duration 0m", ValueValidation),
        ];

        // Unset all NEXTEST_ env vars because they can conflict with the try_parse_from below.
        for (k, _) in std::env::vars() {
            if k.starts_with("NEXTEST_") {
                // SAFETY:
                // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
                unsafe { std::env::remove_var(k) };
            }
        }

        for valid_args in valid {
            let cmd = shell_words::split(valid_args).expect("valid command line");
            // Any args in the beginning with an equals sign should be parsed as environment variables.
            let env_vars: Vec<_> = cmd
                .iter()
                .take_while(|arg| arg.contains('='))
                .cloned()
                .collect();

            let mut env_keys = Vec::with_capacity(env_vars.len());
            for k_v in &env_vars {
                let (k, v) = k_v.split_once('=').expect("valid env var");
                // SAFETY:
                // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
                unsafe { std::env::set_var(k, v) };
                env_keys.push(k);
            }

            let cmd = cmd.iter().skip(env_vars.len());

            if let Err(error) = CargoNextestApp::try_parse_from(cmd) {
                panic!("{valid_args} should have successfully parsed, but didn't: {error}");
            }

            // Unset any environment variables we set. (Don't really need to preserve the old value
            // for now.)
            for &k in &env_keys {
                // SAFETY:
                // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
                unsafe { std::env::remove_var(k) };
            }
        }

        for &(invalid_args, kind) in invalid {
            match CargoNextestApp::try_parse_from(
                shell_words::split(invalid_args).expect("valid command"),
            ) {
                Ok(_) => {
                    panic!("{invalid_args} should have errored out but successfully parsed");
                }
                Err(error) => {
                    let actual_kind = error.kind();
                    if kind != actual_kind {
                        panic!(
                            "{invalid_args} should error with kind {kind:?}, but actual kind was {actual_kind:?}",
                        );
                    }
                }
            }
        }
    }

    #[derive(Debug, clap::Parser)]
    struct TestCli {
        #[structopt(flatten)]
        build_filter: TestBuildFilter,
    }

    #[test]
    fn test_test_binary_argument_parsing() {
        fn get_test_filter_builder(cmd: &str) -> Result<TestFilterBuilder> {
            let app = TestCli::try_parse_from(shell_words::split(cmd).expect("valid command line"))
                .unwrap_or_else(|_| panic!("{cmd} should have successfully parsed"));
            app.build_filter
                .make_test_filter_builder(NextestRunMode::Test, vec![])
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
            ("foo -- --ignored", "foo --run-ignored only"),
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
        let skip_exact = &[
            // ---
            // skip
            // ---
            ("foo -- --skip my-pattern --skip your-pattern", {
                let mut patterns = TestFilterPatterns::default();
                patterns.add_skip_pattern("my-pattern".to_owned());
                patterns.add_skip_pattern("your-pattern".to_owned());
                patterns
            }),
            ("foo -- pattern1 --skip my-pattern --skip your-pattern", {
                let mut patterns = TestFilterPatterns::default();
                patterns.add_substring_pattern("pattern1".to_owned());
                patterns.add_skip_pattern("my-pattern".to_owned());
                patterns.add_skip_pattern("your-pattern".to_owned());
                patterns
            }),
            // ---
            // skip and exact
            // ---
            (
                "foo -- --skip my-pattern --skip your-pattern exact1 --exact pattern2",
                {
                    let mut patterns = TestFilterPatterns::default();
                    patterns.add_skip_exact_pattern("my-pattern".to_owned());
                    patterns.add_skip_exact_pattern("your-pattern".to_owned());
                    patterns.add_exact_pattern("exact1".to_owned());
                    patterns.add_exact_pattern("pattern2".to_owned());
                    patterns
                },
            ),
        ];
        let invalid = &[
            // ---
            // duplicated
            // ---
            ("foo -- --include-ignored --include-ignored", "duplicated"),
            ("foo -- --ignored --ignored", "duplicated"),
            ("foo -- --exact --exact", "duplicated"),
            // ---
            // mutually exclusive
            // ---
            ("foo -- --ignored --include-ignored", "mutually exclusive"),
            ("foo --run-ignored all -- --ignored", "mutually exclusive"),
            // ---
            // missing required argument
            // ---
            ("foo -- --skip", "missing required argument"),
            // ---
            // unsupported
            // ---
            ("foo -- --bar", "unsupported"),
        ];

        for (a, b) in valid {
            let a_str = format!(
                "{:?}",
                get_test_filter_builder(a).unwrap_or_else(|_| panic!("failed to parse {a}"))
            );
            let b_str = format!(
                "{:?}",
                get_test_filter_builder(b).unwrap_or_else(|_| panic!("failed to parse {b}"))
            );
            assert_eq!(a_str, b_str);
        }

        for (args, patterns) in skip_exact {
            let builder =
                get_test_filter_builder(args).unwrap_or_else(|_| panic!("failed to parse {args}"));

            let builder2 = TestFilterBuilder::new(
                NextestRunMode::Test,
                RunIgnored::Default,
                None,
                patterns.clone(),
                Vec::new(),
            )
            .unwrap_or_else(|_| panic!("failed to build TestFilterBuilder"));

            assert_eq!(builder, builder2, "{args} matches expected");
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
