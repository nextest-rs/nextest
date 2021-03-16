// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::test_list::{TestInstance, TestList};
use anyhow::Result;
use duct::cmd;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::{
    convert::Infallible,
    fmt,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use structopt::StructOpt;

/// Test runner options.
#[derive(Debug, Default, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestRunnerOpts {
    /// Number of tests to run simultaneously [default: physical CPU count]
    #[structopt(short, long, alias = "test-threads")]
    pub jobs: Option<usize>,
    // TODO: more test runner options
}

impl TestRunnerOpts {
    /// Creates a new test runner.
    pub fn build(self, test_list: &TestList) -> TestRunner {
        let jobs = self.jobs.unwrap_or_else(num_cpus::get);
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
    /// The callback is called with the results of each test.
    pub fn execute<F>(&self, mut callback: F)
    where
        F: FnMut(TestInstance<'a>, TestRunStatus<'a>) + Send,
    {
        let _ = self.try_execute::<Infallible, _>(|test_instance, run_status| {
            callback(test_instance, run_status);
            Ok(())
        });
    }

    /// Executes the listed tests, each one in its own process.
    ///
    /// Accepts a callback that is called with the results of each test. If the callback returns an
    /// error, the callback is no longer called.
    pub fn try_execute<E, F>(&self, mut callback: F) -> Result<(), E>
    where
        F: FnMut(TestInstance<'a>, TestRunStatus<'a>) -> Result<(), E> + Send,
        E: Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        let (sender, receiver) = mpsc::channel();

        // This is move so that sender is moved into it. When the scope finishes the sender is
        // dropped, and the receiver below completes iteration.

        let canceled = AtomicBool::new(false);
        let canceled_ref = &canceled;

        // XXX rayon requires its scope callback to be Send, there's no good reason for it but
        // there's also no other well-maintained scoped threadpool :(
        self.run_pool.scope(move |run_scope| {
            self.test_list.iter().for_each(|test| {
                if canceled_ref.load(Ordering::Acquire) {
                    // Check for test cancellation.
                    return;
                }

                let run_sender = sender.clone();
                run_scope.spawn(move |_| {
                    if canceled_ref.load(Ordering::Acquire) {
                        // Check for test cancellation.
                        return;
                    }

                    let res = self.run_test(test);
                    // Failure to send means the receiver was dropped.
                    let _ = run_sender.send((test, res));
                })
            });

            drop(sender);

            for (test_instance, run_status) in receiver.iter() {
                if let Err(err) = callback(test_instance, run_status) {
                    canceled_ref.store(true, Ordering::Release);
                    return Err(err);
                }
            }

            Ok(())
        })
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
        let mut cmd = cmd!(
            AsRef::<Path>::as_ref(test.binary),
            "--exact",
            test.test_name,
            "--nocapture",
        )
        .stdout_capture()
        .stderr_capture()
        .unchecked();

        if let Some(cwd) = test.cwd {
            cmd = cmd.dir(cwd);
        }

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
}

impl fmt::Display for TestStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TestStatus::Success => write!(f, "success"),
            TestStatus::Failure => write!(f, "failure"),
            TestStatus::ExecutionFailure => write!(f, "exec-fail"),
        }
    }
}
