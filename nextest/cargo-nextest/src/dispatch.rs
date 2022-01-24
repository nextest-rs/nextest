// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputOpts},
    ExpectedError,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Parser, Subcommand};
use color_eyre::eyre::{Report, Result, WrapErr};
use guppy::graph::PackageGraph;
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
use supports_color::Stream;

/// Next-generation test runner for Rust.
///
/// TODO: expand on this
#[derive(Debug, Parser)]
#[clap(author, version)]
pub struct CargoNextestApp {
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

#[derive(Debug, Args)]
pub struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml]
    #[clap(long, global = true, value_name = "PATH")]
    pub config_file: Option<Utf8PathBuf>,
}

impl ConfigOpts {
    /// Creates a nextest config with the given options.
    pub fn make_config(&self, workspace_root: &Utf8Path) -> Result<NextestConfig, ConfigReadError> {
        NextestConfig::from_sources(workspace_root, self.config_file.as_deref())
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List tests in binary
    List {
        /// Output format
        #[clap(short = 'T', long, default_value_t, possible_values = OutputFormat::variants())]
        format: OutputFormat,

        #[clap(flatten)]
        build_filter: TestBuildFilter,
    },
    /// Run tests
    Run {
        /// Nextest profile to use
        #[clap(long, short = 'P')]
        profile: Option<String>,

        /// Run tests serially and do not capture output
        #[clap(long, alias = "nocapture")]
        no_capture: bool,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        #[clap(flatten)]
        runner_opts: TestRunnerOpts,

        #[clap(flatten)]
        reporter_opts: TestReporterOpts,
    },
}

#[derive(Debug, Args)]
pub struct TestBuildFilter {
    #[clap(flatten)]
    cargo_options: CargoOptions,

    /// Run ignored tests
    #[clap(long, possible_values = RunIgnored::variants(), default_value_t, value_name = "WHICH")]
    run_ignored: RunIgnored,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[clap(long)]
    partition: Option<PartitionerBuilder>,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    #[clap(name = "FILTERS")]
    filter: Vec<String>,
}

impl TestBuildFilter {
    fn compute<'g>(&self, graph: &'g PackageGraph, output: OutputContext) -> Result<TestList<'g>> {
        let manifest_path = graph.workspace().root().join("Cargo.toml");
        let mut cargo_cli = CargoCli::new("test", Some(&manifest_path), output);

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
#[derive(Debug, Default, Args)]
pub struct TestRunnerOpts {
    /// Number of retries for failing tests [default: from profile]
    #[clap(long)]
    retries: Option<usize>,

    /// Cancel test run on the first failure
    #[clap(long)]
    fail_fast: bool,

    /// Run all tests regardless of failure
    #[clap(long, overrides_with = "fail-fast")]
    no_fail_fast: bool,

    /// Number of tests to run simultaneously [default: logical CPU count]
    #[clap(
        long,
        short = 'j',
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

#[derive(Debug, Default, Args)]
pub struct TestReporterOpts {
    /// Output stdout and stderr on failure
    #[clap(
        long,
        possible_values = TestOutputDisplay::variants(),
        conflicts_with = "no-capture",
        value_name = "WHEN"
    )]
    failure_output: Option<TestOutputDisplay>,
    /// Output stdout and stderr on success

    #[clap(
        long,
        possible_values = TestOutputDisplay::variants(),
        conflicts_with = "no-capture",
        value_name = "WHEN"
    )]
    success_output: Option<TestOutputDisplay>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output
    #[clap(long, possible_values = StatusLevel::variants(), value_name = "LEVEL")]
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

impl CargoNextestApp {
    /// Execute the command.
    pub fn exec(self) -> Result<()> {
        let output = self.output.init();

        let graph = build_graph(self.manifest_path.as_deref(), output)?;

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

fn build_graph(manifest_path: Option<&Utf8Path>, output: OutputContext) -> Result<PackageGraph> {
    let mut cargo_cli = CargoCli::new("metadata", manifest_path, output);
    // Construct a package graph with --no-deps since we don't need full dependency
    // information.
    cargo_cli.add_args(["--format-version=1", "--all-features", "--no-deps"]);

    // Capture stdout but not stderr.
    let output = cargo_cli
        .to_expression()
        .stdout_capture()
        .unchecked()
        .run()
        .wrap_err("cargo metadata execution failed")?;
    if !output.status.success() {
        return Err(ExpectedError::cargo_metadata_failed().into());
    }

    let json =
        String::from_utf8(output.stdout).wrap_err("cargo metadata output is invalid UTF-8")?;
    Ok(guppy::CargoMetadata::parse_json(&json)?.build_graph()?)
}
