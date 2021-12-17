// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputOpts},
    partition::PartitionerBuilder,
    reporter::{ReporterOpts, TestReporter},
    runner::TestRunnerOpts,
    signal::SignalHandler,
    test_filter::{RunIgnored, TestFilterBuilder},
    test_list::{OutputFormat, TestBinary, TestList},
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, Result, WrapErr};
use guppy::{graph::PackageGraph, MetadataCommand};
use nextest_config::{errors::ConfigReadError, NextestConfig};
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
    ListTests {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = &OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        build_filter: TestBuildFilter,
    },
    /// Run tests
    Run {
        /// Nextest profile to use
        #[structopt(long, short = "P")]
        profile: Option<String>,
        #[structopt(flatten)]
        build_filter: TestBuildFilter,
        #[structopt(flatten)]
        runner_opts: TestRunnerOpts,
        #[structopt(flatten)]
        reporter_opts: ReporterOpts,
    },
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestBuildFilter {
    #[structopt(flatten)]
    cargo_options: CargoOptions,

    /// Run ignored tests
    #[structopt(long, possible_values = &RunIgnored::variants(), default_value, case_insensitive = true)]
    run_ignored: RunIgnored,

    /// Test partition, e.g. hash:1/2 or count:2/3
    #[structopt(long)]
    partition: Option<PartitionerBuilder>,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    filter: Vec<String>,
}

impl TestBuildFilter {
    fn compute(&self, graph: &PackageGraph, output: OutputContext) -> Result<TestList> {
        let mut cargo_cli = CargoCli::new("test", output);
        let manifest_path = graph.workspace().root().join("Cargo.toml");
        cargo_cli.add_args(["--manifest-path", manifest_path.as_str()]);
        // Only build tests in the cargo test invocation, do not run them.
        cargo_cli.add_args(["--no-run", "--message-format", "json-render-diagnostics"]);
        cargo_cli.add_options(&self.cargo_options);

        let expression = cargo_cli.to_expression();
        let output = expression
            .stdout_capture()
            .run()
            .wrap_err("failed to build tests")?;
        let test_binaries = TestBinary::from_messages(graph, Cursor::new(output.stdout))?;

        let test_filter =
            TestFilterBuilder::new(self.run_ignored, self.partition.clone(), &self.filter);
        TestList::new(test_binaries, &test_filter)
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
            Command::ListTests {
                build_filter,
                format,
            } => {
                let mut test_list = build_filter.compute(&graph, output)?;
                if output.color.should_colorize(Stream::Stdout) {
                    test_list.colorize();
                }
                let lock = stdout.lock();
                test_list.write(format, lock)?;
            }
            Command::Run {
                ref profile,
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

                let mut reporter = TestReporter::new(&test_list, &profile, reporter_opts);
                if output.color.should_colorize(Stream::Stdout) {
                    reporter.colorize();
                }

                let handler = SignalHandler::new().wrap_err("failed to set up Ctrl-C handler")?;
                let runner = runner_opts.build(&test_list, &profile, handler);
                let run_stats = runner.try_execute(|event| {
                    // TODO: consider turning this into a trait, to initialize and carry the lock
                    // across callback invocations
                    let lock = stdout.lock();
                    reporter.report_event(event, lock)
                    // TODO: no-fail-fast logic
                })?;
                if !run_stats.is_success() {
                    bail!("test run failed");
                }
            }
        }
        Ok(())
    }
}
