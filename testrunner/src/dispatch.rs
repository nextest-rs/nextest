// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat,
    reporter::{Color, ReporterOpts, TestReporter},
    runner::TestRunnerOpts,
    test_filter::TestFilter,
    test_list::{TestBinary, TestList},
};
use anyhow::Result;
use camino::Utf8PathBuf;
use structopt::StructOpt;

/// This test runner accepts a Rust test binary and does fancy things with it.
///
/// TODO: expand on this
#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct Opts {
    #[structopt(long, default_value)]
    /// Coloring: always, auto, never
    color: Color,

    #[structopt(subcommand)]
    command: Command,
}

#[derive(Debug, StructOpt)]
pub enum Command {
    /// List tests in binary
    ListTests {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = & OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        bin_filter: TestBinFilter,
    },
    /// Run tests
    Run {
        #[structopt(flatten)]
        bin_filter: TestBinFilter,
        #[structopt(flatten)]
        runner_opts: TestRunnerOpts,
        #[structopt(flatten)]
        reporter_opts: ReporterOpts,
    },
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestBinFilter {
    /// Path to test binary
    #[structopt(
        short = "b",
        long,
        required = true,
        min_values = 1,
        number_of_values = 1
    )]
    pub test_bin: Vec<Utf8PathBuf>,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    pub filter: Vec<String>,
}

impl TestBinFilter {
    fn compute(&self) -> Result<TestList> {
        let test_filter = TestFilter::new(&self.filter);
        TestList::new(
            self.test_bin.iter().map(|binary| TestBinary {
                binary: binary.clone(),
                // TODO: add support for these through the CLI interface?
                friendly_name: None,
                cwd: None,
            }),
            &test_filter,
        )
    }
}

impl Opts {
    /// Execute the command.
    pub fn exec(self) -> Result<()> {
        match self.command {
            Command::ListTests { bin_filter, format } => {
                let test_list = bin_filter.compute()?;
                let reporter = TestReporter::new(&test_list, self.color, ReporterOpts::default());
                reporter.write_list(&test_list, format)?;
            }
            Command::Run {
                bin_filter,
                runner_opts,
                reporter_opts,
            } => {
                let test_list = bin_filter.compute()?;
                let reporter = TestReporter::new(&test_list, self.color, reporter_opts);
                let runner = runner_opts.build(&test_list);
                runner.try_execute(|event| {
                    reporter.report_event(event)
                    // TODO: no-fail-fast logic
                })?;
            }
        }
        Ok(())
    }
}
