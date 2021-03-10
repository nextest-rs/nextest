// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat, runner::TestRunnerOpts, test_filter::TestFilter, test_list::TestList,
};
use anyhow::Result;
use camino::Utf8PathBuf;
use std::io;
use structopt::StructOpt;

/// This test runner accepts a Rust test binary and does fancy things with it.
///
/// TODO: expand on this
#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub enum Opts {
    /// List tests in binary
    ListTests {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = &OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        bin_filter: TestBinFilter,
    },
    /// Run tests
    Run {
        #[structopt(flatten)]
        bin_filter: TestBinFilter,
        #[structopt(flatten)]
        opts: TestRunnerOpts,
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
        TestList::new(&self.test_bin, &test_filter)
    }
}

impl Opts {
    /// Execute this test binary, writing results to the given writer.
    pub fn exec(self, mut writer: impl io::Write) -> Result<()> {
        match self {
            Opts::ListTests { bin_filter, format } => {
                let test_list = bin_filter.compute()?;
                test_list.write(format, writer)?;
            }
            Opts::Run { bin_filter, opts } => {
                writeln!(writer, "Running {:?}", bin_filter.test_bin)?;

                let test_list = bin_filter.compute()?;
                let runner = opts.build(&test_list);
                let receiver = runner.execute();
                for (test, run_status) in receiver.iter() {
                    writeln!(
                        writer,
                        "{} {}: {} ({:?})",
                        test.test_bin, test.test_name, run_status.status, run_status.time_taken
                    )?;
                }
            }
        }
        Ok(())
    }
}
