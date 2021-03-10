// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::test_list::{TestInstance, TestList};
use anyhow::Result;
use duct::cmd;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::{
    fmt,
    path::Path,
    sync::{mpsc, mpsc::Receiver},
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
    pub fn build(self, test_list: &TestList) -> TestRunner {
        let jobs = self.jobs.unwrap_or_else(num_cpus::get_physical);
        TestRunner {
            opts: self,
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
    test_list: &'a TestList,
    run_pool: ThreadPool,
}

impl<'a> TestRunner<'a> {
    /// Executes the listed tests, each one in its own process.
    ///
    /// Returns a receiver which can be used to gather test results.
    pub fn execute(&self) -> Receiver<(TestInstance<'a>, TestRunStatus<'a>)> {
        // TODO: add support for other test-running approaches, measure performance.

        let (sender, receiver) = mpsc::channel();

        // This is move so that sender is moved into it. When the scope finishes the sender is
        // dropped, and the receiver below completes iteration.
        self.run_pool.scope(move |run_scope| {
            self.test_list.iter().for_each(|test| {
                let run_sender = sender.clone();
                run_scope.spawn(move |_| {
                    let res = self.run_test(test);
                    // Failure to send means the receiver was dropped.
                    let _ = run_sender.send((test, res));
                })
            });
        });

        receiver
    }

    // ---
    // Helper methods
    // ---

    /// Run an individual test in its own process.
    fn run_test(&self, test: TestInstance<'a>) -> TestRunStatus<'a> {
        let start_time = Instant::now();

        match self.run_test_inner(test, &start_time) {
            Ok(run_status) => run_status,
            Err(_) => TestRunStatus {
                test,
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: vec![],
                stderr: vec![],
                status: TestStatus::ExecutionFailure,
                time_taken: start_time.elapsed(),
            },
        }
    }

    fn run_test_inner(
        &self,
        test: TestInstance<'a>,
        start_time: &Instant,
    ) -> Result<TestRunStatus<'a>> {
        // Capture stdout and stderr.
        let cmd = cmd!(
            AsRef::<Path>::as_ref(test.test_bin),
            test.test_name,
            "--nocapture"
        )
        .stdout_capture()
        .stderr_capture()
        .unchecked();

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
            test,
            stdout: output.stdout,
            stderr: output.stderr,
            status,
            time_taken,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TestRunStatus<'a> {
    pub test: TestInstance<'a>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: TestStatus,
    pub time_taken: Duration,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TestStatus {
    Success,
    Failure,
    ExecutionFailure,
    InfraFailure,
}

impl fmt::Display for TestStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TestStatus::Success => write!(f, "success"),
            TestStatus::Failure => write!(f, "failure"),
            TestStatus::ExecutionFailure => write!(f, "execution failure"),
            TestStatus::InfraFailure => write!(f, "infra failure"),
        }
    }
}
