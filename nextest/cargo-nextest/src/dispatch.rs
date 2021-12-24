// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputOpts},
    ExpectedError,
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{Report, Result, WrapErr};
use guppy::{graph::PackageGraph, MetadataCommand};
use nextest_config::{errors::ConfigReadError, NextestConfig, StatusLevel, TestOutputDisplay};
use nextest_runner::{
    partition::PartitionerBuilder,
    reporter::TestReporterBuilder,
    runner::TestRunnerBuilder,
    test_filter::{RunIgnored, TestFilterBuilder},
    test_list::{OutputFormat, TestBinary, TestList},
    SignalHandler,
};
use std::io::Cursor;
use structopt::StructOpt;
use supports_color::Stream;

/// This test runner accepts a Rust test binary and does fancy things with it.
///
/// TODO: expand on this
#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct Opts {
    /// Path to Cargo.toml
    #[structopt(long, global = true)]
    manifest_path: Option<Utf8PathBuf>,

    #[structopt(flatten)]
    output: OutputOpts,

    #[structopt(flatten)]
    config_opts: ConfigOpts,

    #[structopt(subcommand)]
    command: Command,
}

#[derive(Debug, StructOpt)]
pub struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml]
    #[structopt(long, global = true)]
    pub config_file: Option<Utf8PathBuf>,
}

impl ConfigOpts {
    /// Creates a nextest config with the given options.
    pub fn make_config(&self, workspace_root: &Utf8Path) -> Result<NextestConfig, ConfigReadError> {
        NextestConfig::from_sources(workspace_root, self.config_file.as_deref())
    }
}

#[derive(Debug, StructOpt)]
pub enum Command {
    /// List tests in binary
    List {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        build_filter: TestBuildFilter,
    },
    /// Run tests
    Run {
        /// Nextest profile to use
        #[structopt(long, short = "P")]
        profile: Option<String>,

        /// Run tests serially and do not capture output
        #[structopt(long, alias = "nocapture")]
        no_capture: bool,

        #[structopt(flatten)]
        build_filter: TestBuildFilter,

        #[structopt(flatten)]
        runner_opts: TestRunnerOpts,

        #[structopt(flatten)]
        reporter_opts: TestReporterOpts,
    },
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestBuildFilter {
    #[structopt(flatten)]
    cargo_options: CargoOptions,

    /// Run ignored tests
    #[structopt(long, possible_values = RunIgnored::variants(), default_value, case_insensitive = true)]
    run_ignored: RunIgnored,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[structopt(long)]
    partition: Option<PartitionerBuilder>,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    #[structopt(name = "FILTERS")]
    filter: Vec<String>,
}

impl TestBuildFilter {
    fn compute<'g>(&self, graph: &'g PackageGraph, output: OutputContext) -> Result<TestList<'g>> {
        let mut cargo_cli = CargoCli::new("test", output);
        let manifest_path = graph.workspace().root().join("Cargo.toml");
        cargo_cli.add_args(["--manifest-path", manifest_path.as_str()]);
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

        let test_binaries = TestBinary::from_messages(graph, Cursor::new(output.stdout))?;

        let test_filter =
            TestFilterBuilder::new(self.run_ignored, self.partition.clone(), &self.filter);
        TestList::new(test_binaries, &test_filter).wrap_err("error building test list")
    }
}

/// Test runner options.
#[derive(Debug, Default, StructOpt)]
pub struct TestRunnerOpts {
    /// Number of retries for failing tests [default: from profile]
    #[structopt(long)]
    retries: Option<usize>,

    /// Cancel test run on the first failure
    #[structopt(long)]
    fail_fast: bool,

    /// Run all tests regardless of failure
    #[structopt(long, overrides_with = "fail-fast")]
    no_fail_fast: bool,

    /// Number of tests to run simultaneously [default: logical CPU count]
    #[structopt(
        long,
        short = "j",
        visible_alias = "jobs",
        value_name = "THREADS",
        conflicts_with = "no-capture"
    )]
    test_threads: Option<usize>,
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

#[derive(Debug, Default, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestReporterOpts {
    /// Output stdout and stderr on failure
    #[structopt(
        long,
        possible_values = TestOutputDisplay::variants(),
        case_insensitive = true,
        conflicts_with = "no-capture"
    )]
    failure_output: Option<TestOutputDisplay>,
    /// Output stdout and stderr on success

    #[structopt(
        long,
        possible_values = TestOutputDisplay::variants(),
        case_insensitive = true,
        conflicts_with = "no-capture"
    )]
    success_output: Option<TestOutputDisplay>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output
    #[structopt(long, possible_values = StatusLevel::variants(), case_insensitive = true)]
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

impl Opts {
    /// Execute the command.
    pub fn exec(self) -> Result<()> {
        let output = self.output.init();

        let graph = {
            let mut metadata_command = MetadataCommand::new();
            if let Some(path) = &self.manifest_path {
                metadata_command.manifest_path(path);
            }
            // Construct a package graph with --no-deps since we don't need full dependency
            // information.
            metadata_command.no_deps().build_graph()?
        };

        match self.command {
            Command::List {
                build_filter,
                format,
            } => {
                let mut test_list = build_filter.compute(&graph, output)?;
                if output.color.should_colorize(Stream::Stdout) {
                    test_list.colorize();
                }
                let stdout = std::io::stdout();
                let lock = stdout.lock();
                test_list.write(format, lock)?;
            }
            Command::Run {
                ref profile,
                no_capture,
                ref build_filter,
                ref runner_opts,
                ref reporter_opts,
            } => {
                let config = self.config_opts.make_config(graph.workspace().root())?;
                let profile =
                    config.profile(profile.as_deref().unwrap_or(NextestConfig::DEFAULT_PROFILE))?;
                let metadata_dir = profile.metadata_dir();
                std::fs::create_dir_all(&metadata_dir).wrap_err_with(|| {
                    format!("failed to create metadata dir '{}'", metadata_dir)
                })?;

                let test_list = build_filter.compute(&graph, output)?;

                let mut reporter = reporter_opts
                    .to_builder(no_capture)
                    .build(&test_list, &profile);
                if output.color.should_colorize(Stream::Stderr) {
                    reporter.colorize();
                }

                let handler = SignalHandler::new().wrap_err("failed to set up Ctrl-C handler")?;
                let runner = runner_opts
                    .to_builder(no_capture)
                    .build(&test_list, &profile, handler);
                let stderr = std::io::stderr();
                let run_stats = runner.try_execute(|event| {
                    // TODO: consider turning this into a trait, to initialize and carry the lock
                    // across callback invocations
                    let lock = stderr.lock();
                    reporter.report_event(event, lock)
                })?;
                if !run_stats.is_success() {
                    return Err(Report::new(ExpectedError::test_run_failed()));
                }
            }
        }
        Ok(())
    }
}
