// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::test_list::TestList;
use anyhow::Result;
use camino::Utf8Path;
use duct::cmd;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
use structopt::StructOpt;

/// Test runner options.
#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestRunnerOpts {
    /// Number of tests to run simultaneously [default: physical CPU count]
    #[structopt(short, long, alias = "test-threads")]
    jobs: Option<usize>,
    // TODO: more test runner options
}

impl TestRunnerOpts {
    /// Creates a new test runner.
    pub fn build<'a>(self, test_bin: &'a Utf8Path, test_list: &'a TestList) -> TestRunner<'a> {
        let jobs = self.jobs.unwrap_or_else(num_cpus::get_physical);
        TestRunner {
            opts: self,
            test_bin,
            test_list,
            run_pool: ThreadPoolBuilder::new()
                .num_threads(jobs)
                .thread_name(|idx| format!("testrunner-run-{}", idx))
                .build()
                .expect("thread pool should build"),
        }
    }
}

/// Context for running tests.
pub struct TestRunner<'a> {
    #[allow(dead_code)]
    opts: TestRunnerOpts,
    test_bin: &'a Utf8Path,
    test_list: &'a TestList,
    run_pool: ThreadPool,
}

impl<'a> TestRunner<'a> {
    /// Executes the listed tests, each one in its own process.
    pub fn execute(&self) -> BTreeMap<&'a str, Result<TestRunStatus<'a>>> {
        // TODO: add support for other test-running approaches, measure performance.

        let (sender, receiver) = mpsc::channel();

        // This is move so that sender is moved into it. When the scope finishes the sender is
        // dropped, and the receiver below completes iteration.
        self.run_pool.scope(move |run_scope| {
            self.test_list.iter().for_each(|test_name| {
                let run_sender = sender.clone();
                run_scope.spawn(move |_| {
                    let res = self.run_test(test_name);
                    run_sender
                        .send((test_name, res))
                        .expect("receiver is still around at this point");
                })
            });
        });

        let results: BTreeMap<&'a str, Result<TestRunStatus<'a>>> = receiver.iter().collect();
        results
    }

    // ---
    // Helper methods
    // ---

    /// Run an individual test in its own process.
    fn run_test(&self, test_name: &'a str) -> Result<TestRunStatus<'a>> {
        // Capture stdout and stderr.
        let cmd = cmd!(
            AsRef::<Path>::as_ref(self.test_bin),
            test_name,
            "--nocapture"
        )
        .stdout_capture()
        .stderr_capture()
        .unchecked();

        let start_time = Instant::now();
        let handle = cmd.start()?;

        // TODO: timeout/kill logic

        let output = handle.into_output()?;
        let time_taken = start_time.elapsed();
        let status = if output.status.success() {
            TestStatus::Success
        } else {
            TestStatus::Failure
        };
        Ok(TestRunStatus {
            test_name,
            stdout: output.stdout,
            stderr: output.stderr,
            status,
            time_taken,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TestRunStatus<'a> {
    test_name: &'a str,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: TestStatus,
    time_taken: Duration,
}

#[derive(Copy, Clone, Debug)]
pub enum TestStatus {
    Success,
    Failure,
    InfraFailure,
}
