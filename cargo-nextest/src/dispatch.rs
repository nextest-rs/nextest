// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputOpts, OutputWriter},
    reuse_build::ReuseBuildOpts,
    ExpectedError,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgEnum, Args, Parser, Subcommand};
use color_eyre::eyre::{Report, Result, WrapErr};
use guppy::graph::PackageGraph;
use nextest_filtering::FilteringExpr;
use nextest_metadata::{BinaryListSummary, BuildPlatform};
use nextest_runner::{
    config::{NextestConfig, NextestProfile},
    errors::{TargetRunnerError, WriteEventError},
    list::{BinaryList, OutputFormat, RustTestArtifact, SerializableFormat, TestList},
    partition::PartitionerBuilder,
    reporter::{StatusLevel, TestOutputDisplay, TestReporterBuilder},
    runner::TestRunnerBuilder,
    signal::SignalHandler,
    target_runner::TargetRunner,
    test_filter::{RunIgnored, TestFilterBuilder},
};
use owo_colors::{OwoColorize, Style};
use std::{
    error::Error,
    fmt::Write as _,
    io::{Cursor, Write},
};
use supports_color::Stream;

/// A next-generation test runner for Rust.
///
/// This binary should typically be invoked as `cargo nextest` (in which case
/// this message will not be seen), not `cargo-nextest`.
#[derive(Debug, Parser)]
#[clap(version, bin_name = "cargo")]
pub struct CargoNextestApp {
    #[clap(subcommand)]
    subcommand: NextestSubcommand,
}

impl CargoNextestApp {
    /// Executes the app.
    pub fn exec(self, output_writer: &mut OutputWriter) -> Result<()> {
        let NextestSubcommand::Nextest(app) = self.subcommand;
        app.exec(output_writer)
    }
}

#[derive(Debug, Subcommand)]
enum NextestSubcommand {
    /// A next-generation test runner for Rust. <https://nexte.st>
    Nextest(AppOpts),
}

#[derive(Debug, Args)]
#[clap(version)]
struct AppOpts {
    /// Path to Cargo.toml
    #[clap(long, global = true, value_name = "PATH")]
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
    fn exec(self, output_writer: &mut OutputWriter) -> Result<()> {
        match self.command {
            Command::List {
                build_filter,
                message_format,
                list_type,
                reuse_build,
            } => {
                let app = App::new(
                    self.output,
                    reuse_build,
                    build_filter,
                    self.config_opts,
                    self.manifest_path,
                )?;
                app.exec_list(message_format, list_type, output_writer)
            }
            Command::Run {
                profile,
                no_capture,
                build_filter,
                runner_opts,
                reporter_opts,
                reuse_build,
            } => {
                let app = App::new(
                    self.output,
                    reuse_build,
                    build_filter,
                    self.config_opts,
                    self.manifest_path,
                )?;
                app.exec_run(
                    profile.as_deref(),
                    no_capture,
                    &runner_opts,
                    &reporter_opts,
                    output_writer,
                )
            }
        }
    }
}

#[derive(Debug, Args)]
struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml]
    #[clap(long, global = true, value_name = "PATH")]
    pub config_file: Option<Utf8PathBuf>,
}

impl ConfigOpts {
    /// Creates a nextest config with the given options.
    pub fn make_config(&self, workspace_root: &Utf8Path) -> Result<NextestConfig, ExpectedError> {
        NextestConfig::from_sources(workspace_root, self.config_file.as_deref())
            .map_err(ExpectedError::config_parse_error)
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List tests in workspace
    ///
    /// This command builds test binaries and queries them for the tests they contain.
    /// Use --message-format json to get machine-readable output.
    ///
    /// For more information, see <https://nexte.st/book/listing>.
    List {
        #[clap(flatten)]
        build_filter: TestBuildFilter,

        /// Output format
        #[clap(
            short = 'T',
            long,
            arg_enum,
            default_value_t,
            help_heading = "OUTPUT OPTIONS",
            value_name = "FMT"
        )]
        message_format: MessageFormatOpts,

        /// Type of listing
        #[clap(
            long,
            arg_enum,
            default_value_t,
            help_heading = "OUTPUT OPTIONS",
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
        #[clap(long, short = 'P', env = "NEXTEST_PROFILE")]
        profile: Option<String>,

        /// Run tests serially and do not capture output
        #[clap(
            long,
            alias = "nocapture",
            help_heading = "RUNNER OPTIONS",
            display_order = 100
        )]
        no_capture: bool,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        #[clap(flatten)]
        runner_opts: TestRunnerOpts,

        #[clap(flatten)]
        reporter_opts: TestReporterOpts,

        #[clap(flatten)]
        reuse_build: ReuseBuildOpts,
    },
}

#[derive(Copy, Clone, Debug, ArgEnum)]
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

#[derive(Copy, Clone, Debug, ArgEnum)]
enum ListType {
    Full,
    BinariesOnly,
}

impl Default for ListType {
    fn default() -> Self {
        Self::Full
    }
}

#[derive(Copy, Clone, Debug, ArgEnum)]
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
#[clap(next_help_heading = "FILTER OPTIONS")]
struct TestBuildFilter {
    #[clap(flatten)]
    cargo_options: CargoOptions,

    /// Run ignored tests
    #[clap(
        long,
        possible_values = RunIgnored::variants(),
        default_value_t,
        value_name = "WHICH",
    )]
    run_ignored: RunIgnored,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[clap(long)]
    partition: Option<PartitionerBuilder>,

    /// Filter test binaries by build platform
    #[clap(long, arg_enum, value_name = "PLATFORM", default_value_t)]
    pub(crate) platform_filter: PlatformFilterOpts,

    /// A DSL based filter expression
    #[clap(
        long,
        short = 'E',
        value_name = "EXPRESSION",
        multiple_occurrences(true)
    )]
    expr_filter: Vec<String>,

    // TODO: add regex-based filtering in the future?
    /// Test name filter
    #[clap(name = "FILTERS", help_heading = None)]
    filter: Vec<String>,
}

impl TestBuildFilter {
    fn compute_test_list<'g>(
        &self,
        graph: &'g PackageGraph,
        binary_list: BinaryList,
        runner: &TargetRunner,
        reuse_build: &ReuseBuildOpts,
        filter_exprs: Vec<FilteringExpr>,
    ) -> Result<TestList<'g>> {
        let path_mapper =
            reuse_build.make_path_mapper(graph, &binary_list.rust_build_meta.target_directory);
        let rust_build_meta = binary_list.rust_build_meta.clone();
        let test_artifacts = RustTestArtifact::from_binary_list(
            graph,
            binary_list,
            &path_mapper,
            self.platform_filter.into(),
        )?;
        let test_filter = TestFilterBuilder::new(
            self.run_ignored,
            self.partition.clone(),
            &self.filter,
            filter_exprs,
        );
        TestList::new(
            test_artifacts,
            &rust_build_meta,
            &path_mapper,
            &test_filter,
            runner,
        )
        .wrap_err("error building test list")
    }

    fn compute_binary_list(
        &self,
        graph: &PackageGraph,
        manifest_path: Option<&Utf8Path>,
        output: OutputContext,
    ) -> Result<BinaryList> {
        // Don't use the manifest path from the graph to ensure that if the user cd's into a
        // particular crate and runs cargo nextest, then it behaves identically to cargo test.
        let mut cargo_cli = CargoCli::new("test", manifest_path, output);

        // Only build tests in the cargo test invocation, do not run them.
        cargo_cli.add_args(["--no-run", "--message-format", "json-render-diagnostics"]);
        cargo_cli.add_options(&self.cargo_options);

        let expression = cargo_cli.to_expression();
        let output = expression
            .stdout_capture()
            .unchecked()
            .run()
            .wrap_err("failed to build tests")?;
        if !output.status.success() {
            return Err(Report::new(ExpectedError::build_failed(
                cargo_cli.all_args(),
                output.status.code(),
            )));
        }

        let test_binaries = BinaryList::from_messages(Cursor::new(output.stdout), graph)?;
        Ok(test_binaries)
    }
}

/// Test runner options.
#[derive(Debug, Default, Args)]
#[clap(next_help_heading = "RUNNER OPTIONS")]
pub struct TestRunnerOpts {
    /// Number of tests to run simultaneously [default: logical CPU count]
    #[clap(
        long,
        short = 'j',
        visible_alias = "jobs",
        value_name = "THREADS",
        conflicts_with = "no-capture",
        env = "NEXTEST_TEST_THREADS"
    )]
    test_threads: Option<usize>,

    /// Number of retries for failing tests [default: from profile]
    #[clap(long)]
    retries: Option<usize>,

    /// Cancel test run on the first failure
    #[clap(long)]
    fail_fast: bool,

    /// Run all tests regardless of failure
    #[clap(long, overrides_with = "fail-fast")]
    no_fail_fast: bool,
}

impl TestRunnerOpts {
    fn to_builder(&self, no_capture: bool) -> TestRunnerBuilder {
        let mut builder = TestRunnerBuilder::default();
        builder.set_no_capture(no_capture);
        if let Some(retries) = self.retries {
            builder.set_retries(retries);
        }
        if self.no_fail_fast {
            builder.set_fail_fast(false);
        } else if self.fail_fast {
            builder.set_fail_fast(true);
        }
        if let Some(test_threads) = self.test_threads {
            builder.set_test_threads(test_threads);
        }

        builder
    }
}

#[derive(Debug, Default, Args)]
#[clap(next_help_heading = "REPORTER OPTIONS")]
struct TestReporterOpts {
    /// Output stdout and stderr on failure
    #[clap(
        long,
        possible_values = TestOutputDisplay::variants(),
        conflicts_with = "no-capture",
        value_name = "WHEN",
        env = "NEXTEST_FAILURE_OUTPUT",
    )]
    failure_output: Option<TestOutputDisplay>,
    /// Output stdout and stderr on success

    #[clap(
        long,
        possible_values = TestOutputDisplay::variants(),
        conflicts_with = "no-capture",
        value_name = "WHEN",
        env = "NEXTEST_SUCCESS_OUTPUT",
    )]
    success_output: Option<TestOutputDisplay>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output
    #[clap(
        long,
        possible_values = StatusLevel::variants(),
        value_name = "LEVEL",
        env = "NEXTEST_STATUS_LEVEL",
    )]
    status_level: Option<StatusLevel>,
}

impl TestReporterOpts {
    fn to_builder(&self, no_capture: bool) -> TestReporterBuilder {
        let mut builder = TestReporterBuilder::default();
        builder.set_no_capture(no_capture);
        if let Some(failure_output) = self.failure_output {
            builder.set_failure_output(failure_output);
        }
        if let Some(success_output) = self.success_output {
            builder.set_success_output(success_output);
        }
        if let Some(status_level) = self.status_level {
            builder.set_status_level(status_level);
        }
        builder
    }
}

struct App {
    output: OutputContext,
    graph: PackageGraph,
    workspace_root: Utf8PathBuf,
    manifest_path: Option<Utf8PathBuf>,
    reuse_build: ReuseBuildOpts,
    build_filter: TestBuildFilter,
    config_opts: ConfigOpts,
}

fn check_experimental_filtering(build_filter: &TestBuildFilter) -> Result<()> {
    const EXPERIMENTAL_ENV: &str = "NEXTEST_EXPERIMENTAL_EXPR_FILTER";
    let enabled = std::env::var(EXPERIMENTAL_ENV).is_ok();
    if !build_filter.expr_filter.is_empty() && !enabled {
        Err(Report::new(ExpectedError::experimental_feature_error(
            "expression filtering",
            EXPERIMENTAL_ENV,
        )))
    } else {
        Ok(())
    }
}

impl App {
    fn new(
        output: OutputOpts,
        reuse_build: ReuseBuildOpts,
        build_filter: TestBuildFilter,
        config_opts: ConfigOpts,
        manifest_path: Option<Utf8PathBuf>,
    ) -> Result<Self> {
        let output = output.init();
        reuse_build.check_experimental(output)?;
        check_experimental_filtering(&build_filter)?;

        let graph_data = match reuse_build.cargo_metadata.as_deref() {
            Some(path) => std::fs::read_to_string(path)?,
            None => {
                let with_deps = build_filter
                    .expr_filter
                    .iter()
                    .any(|expr| FilteringExpr::needs_deps(expr));
                acquire_graph_data(
                    manifest_path.as_deref(),
                    build_filter.cargo_options.target_dir.as_deref(),
                    output,
                    with_deps,
                )?
            }
        };
        let graph = guppy::CargoMetadata::parse_json(&graph_data)?.build_graph()?;

        let manifest_path = if reuse_build.cargo_metadata.is_some() {
            Some(graph.workspace().root().join("Cargo.toml"))
        } else {
            manifest_path
        };

        let workspace_root = match &reuse_build.workspace_remap {
            Some(path) => path.clone(),
            _ => graph.workspace().root().to_owned(),
        };

        Ok(Self {
            output,
            graph,
            reuse_build,
            build_filter,
            manifest_path,
            workspace_root,
            config_opts,
        })
    }

    fn build_binary_list(&self) -> Result<BinaryList> {
        let binary_list = match self.reuse_build.binaries_metadata.as_deref() {
            Some(path) => {
                let raw_binary_list = std::fs::read_to_string(path)?;
                let binary_list: BinaryListSummary = serde_json::from_str(&raw_binary_list)?;
                BinaryList::from_summary(binary_list)
            }
            None => self.build_filter.compute_binary_list(
                &self.graph,
                self.manifest_path.as_deref(),
                self.output,
            )?,
        };
        Ok(binary_list)
    }

    fn build_filtering_expressions(&self) -> Result<Vec<FilteringExpr>> {
        let mut exprs = Vec::new();
        let mut failed = false;
        for res in self
            .build_filter
            .expr_filter
            .iter()
            .map(|input| FilteringExpr::parse(input, &self.graph))
        {
            match res {
                Ok(expr) => exprs.push(expr),
                Err(nextest_filtering::errors::FilteringExprParsingError(_)) => {
                    failed = true;
                }
            }
        }
        if failed {
            Err(ExpectedError::filter_expression_parse_error().into())
        } else {
            Ok(exprs)
        }
    }

    fn build_test_list(
        &self,
        binary_list: BinaryList,
        target_runner: &TargetRunner,
        filter_exprs: Vec<FilteringExpr>,
    ) -> Result<TestList> {
        self.build_filter.compute_test_list(
            &self.graph,
            binary_list,
            target_runner,
            &self.reuse_build,
            filter_exprs,
        )
    }

    fn load_profile<'cfg>(
        &self,
        profile_name: Option<&str>,
        config: &'cfg NextestConfig,
    ) -> Result<NextestProfile<'cfg>> {
        let profile = config
            .profile(profile_name.unwrap_or(NextestConfig::DEFAULT_PROFILE))
            .map_err(ExpectedError::profile_not_found)?;
        let store_dir = profile.store_dir();
        std::fs::create_dir_all(&store_dir)
            .wrap_err_with(|| format!("failed to create store dir '{}'", store_dir))?;
        Ok(profile)
    }

    fn load_runner(&self) -> TargetRunner {
        // When cross-compiling we should not use the cross target runner
        // for running the host tests (like proc-macro ones).
        runner_for_target(self.build_filter.cargo_options.target.as_deref())
    }

    fn exec_list(
        &self,
        message_format: MessageFormatOpts,
        list_type: ListType,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let filter_exprs = self.build_filtering_expressions()?;
        let binary_list = self.build_binary_list()?;

        match list_type {
            ListType::BinariesOnly => {
                let mut writer = output_writer.stdout_writer();
                binary_list.write(
                    message_format.to_output_format(self.output.verbose),
                    &mut writer,
                    self.output.color.should_colorize(Stream::Stdout),
                )?;
                writer.flush()?;
            }
            ListType::Full => {
                let target_runner = self.load_runner();
                let test_list = self.build_test_list(binary_list, &target_runner, filter_exprs)?;

                let mut writer = output_writer.stdout_writer();
                test_list.write(
                    message_format.to_output_format(self.output.verbose),
                    &mut writer,
                    self.output.color.should_colorize(Stream::Stdout),
                )?;
                writer.flush()?;
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
            .config_opts
            .make_config(self.workspace_root.as_path())?;
        let profile = self.load_profile(profile_name, &config)?;

        let target_runner = self.load_runner();

        let filter_exprs = self.build_filtering_expressions()?;
        let binary_list = self.build_binary_list()?;
        let test_list = self.build_test_list(binary_list, &target_runner, filter_exprs)?;

        let mut reporter = reporter_opts
            .to_builder(no_capture)
            .set_verbose(self.output.verbose)
            .build(&test_list, &profile);
        if self.output.color.should_colorize(Stream::Stderr) {
            reporter.colorize();
        }

        let handler = SignalHandler::new().wrap_err("failed to set up Ctrl-C handler")?;
        let runner_builder = runner_opts.to_builder(no_capture);
        let runner = runner_builder.build(&test_list, &profile, handler, target_runner);

        let mut writer = output_writer.stderr_writer();
        let run_stats = runner.try_execute(|event| {
            // Write and flush the event.
            reporter.report_event(event, &mut writer)?;
            writer.flush().map_err(WriteEventError::Io)
        })?;
        if !run_stats.is_success() {
            return Err(Report::new(ExpectedError::test_run_failed()));
        }
        Ok(())
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
        .wrap_err("cargo metadata execution failed")?;
    if !output.status.success() {
        return Err(ExpectedError::cargo_metadata_failed().into());
    }

    let json =
        String::from_utf8(output.stdout).wrap_err("cargo metadata output is invalid UTF-8")?;
    Ok(json)
}

fn runner_for_target(triple: Option<&str>) -> TargetRunner {
    match TargetRunner::new(triple) {
        Ok(runner) => runner,
        Err(err) => {
            warn_on_target_runner_err(&err).expect("writing to a string is infallible");
            TargetRunner::empty()
        }
    }
}

fn warn_on_target_runner_err(err: &TargetRunnerError) -> Result<(), std::fmt::Error> {
    let mut s = String::with_capacity(256);
    write!(s, "could not determine target runner: {}", err)?;
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
