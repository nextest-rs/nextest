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
    ListTests {
        /// Output format
        #[structopt(short = "T", long, default_value, possible_values = &OutputFormat::variants(), case_insensitive = true)]
        format: OutputFormat,

        #[structopt(flatten)]
        bin_filter: TestBinFilter,
    },
    Run {
        #[structopt(flatten)]
        bin_filter: TestBinFilter,
        #[structopt(flatten)]
        opts: TestRunnerOpts,
    },
}

#[derive(Debug, StructOpt)]
pub struct TestBinFilter {
    /// Path to the test binary to run.
    test_bin: Utf8PathBuf,

    // TODO: add regex-based filtering in the future?
    /// Test filter
    filter: Vec<String>,
}

impl TestBinFilter {
    fn compute(&self) -> Result<TestList> {
        let test_filter = TestFilter::new(&self.filter);
        TestList::new(&self.test_bin, &test_filter)
    }
}

impl Opts {
    /// Execute this test binary.
    pub fn exec(self) -> Result<()> {
        match self {
            Opts::ListTests { bin_filter, format } => {
                let test_list = bin_filter.compute()?;
                let stdout = io::stdout();
                let stdout_lock = stdout.lock();
                test_list.write(format, stdout_lock)?;
            }
            Opts::Run { bin_filter, opts } => {
                println!("Running {}", bin_filter.test_bin);

                let test_list = bin_filter.compute()?;
                let runner = opts.build(&bin_filter.test_bin, &test_list);
                let results = runner.execute();
                println!("{:?}", results);
            }
        }
        Ok(())
    }
}
