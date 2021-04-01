// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    reporter::{CancelReason, TestEvent},
    test_list::{TestInstance, TestList},
};
use anyhow::Result;
use crossbeam_channel::Sender;
use duct::cmd;
use rayon::{ThreadPool, ThreadPoolBuilder};
use signal_hook::{iterator::Handle, low_level::emulate_default_handler};
use std::{
    convert::Infallible,
    fmt,
    marker::PhantomData,
    os::raw::c_int,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
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
                // The main run_pool closure will need its own thread.
                .num_threads(jobs + 1)
                .thread_name(|idx| format!("testrunner-run-{}", idx))
                .build()
                .expect("run pool built"),
        }
    }
}

/// Context for running tests.
pub struct TestRunner<'list> {
    #[allow(dead_code)]
    opts: TestRunnerOpts,
    test_list: &'list TestList,
    run_pool: ThreadPool,
}

impl<'list> TestRunner<'list> {
    /// Executes the listed tests, each one in its own process.
    ///
    /// The callback is called with the results of each test.
    pub fn execute<F>(&self, mut callback: F) -> RunStats
    where
        F: FnMut(TestEvent<'list>) + Send,
    {
        self.try_execute::<Infallible, _>(|test_event| {
            callback(test_event);
            Ok(())
        })
        .expect("Err branch is infallible")
    }

    /// Executes the listed tests, each one in its own process.
    ///
    /// Accepts a callback that is called with the results of each test. If the callback returns an
    /// error, the callback is no longer called.
    pub fn try_execute<E, F>(&self, callback: F) -> Result<RunStats, E>
    where
        F: FnMut(TestEvent<'list>) -> Result<(), E> + Send,
        E: Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        let (run_sender, run_receiver) = crossbeam_channel::unbounded();

        // This is move so that sender is moved into it. When the scope finishes the sender is
        // dropped, and the receiver below completes iteration.

        let canceled = AtomicBool::new(false);
        let canceled_ref = &canceled;

        // ---
        // Spawn the signal handler thread.
        // ---
        let (srp_sender, srp_receiver) = crossbeam_channel::bounded(1);
        let (signal_sender, signal_receiver) = crossbeam_channel::unbounded();
        spawn_signal_thread(signal_sender, srp_sender);

        let mut ctx = CallbackContext::new(callback, self.test_list.run_count());

        // Send the initial event.
        // (Don't need to set the canceled atomic if this fails because the run hasn't started
        // yet.)
        ctx.run_started(self.test_list)?;

        // Stores the first error that occurred. This error is propagated up.
        let mut first_error = None;

        let ctx_mut = &mut ctx;
        let first_error_mut = &mut first_error;

        // ---
        // Spawn the test threads.
        // ---
        // XXX rayon requires its scope callback to be Send, there's no good reason for it but
        // there's also no other well-maintained scoped threadpool :(
        self.run_pool.scope(move |run_scope| {
            // Block until signals are set up.
            let _ = srp_receiver.recv();

            self.test_list.iter_tests().for_each(|test_instance| {
                if canceled_ref.load(Ordering::Acquire) {
                    // Check for test cancellation.
                    return;
                }

                let this_run_sender = run_sender.clone();
                run_scope.spawn(move |_| {
                    if canceled_ref.load(Ordering::Acquire) {
                        // Check for test cancellation.
                        return;
                    }

                    if !test_instance.info.filter_match.is_match() {
                        // Failure to send means the receiver was dropped.
                        let _ = this_run_sender.send(InternalTestEvent::Skipped { test_instance });
                        return;
                    }

                    // Failure to send means the receiver was dropped.
                    let _ = this_run_sender.send(InternalTestEvent::Started { test_instance });

                    let run_status = self.run_test(test_instance);
                    // Failure to send means the receiver was dropped.
                    let _ = this_run_sender.send(InternalTestEvent::Finished {
                        test_instance,
                        run_status,
                    });
                })
            });

            drop(run_sender);

            loop {
                let internal_event = crossbeam_channel::select! {
                    recv(run_receiver) -> internal_event => {
                        match internal_event {
                            Ok(event) => InternalEvent::Test(event),
                            Err(_) => {
                                // All runs have been completed.
                                break;
                            }
                        }
                    },
                    recv(signal_receiver) -> internal_event => {
                        match internal_event {
                            Ok(event) => InternalEvent::Signal(event),
                            Err(_) => {
                                // Ignore the signal thread being dropped.
                                // XXX is this correct?
                                continue;
                            }
                        }
                    },
                };

                match ctx_mut.handle_event(internal_event) {
                    Ok(()) => {}
                    Err(err) => {
                        // If an error happens, it is because either the callback failed or
                        // a cancellation notice was received. If the callback failed, we need
                        // to send a further cancellation notice as well.
                        canceled_ref.store(true, Ordering::Release);

                        match err {
                            InternalError::Error(err) => {
                                // Ignore errors that happen during error cancellation.
                                if first_error_mut.is_none() {
                                    *first_error_mut = Some(err);
                                }
                                let _ = ctx_mut.error_cancel();
                            }
                            InternalError::SignalCanceled(Some(err)) => {
                                // Signal-based cancellation and an error was received during
                                // cancellation.
                                if first_error_mut.is_none() {
                                    *first_error_mut = Some(err);
                                }
                            }
                            InternalError::SignalCanceled(None) => {
                                // Signal-based cancellation and no error was returned during
                                // cancellation. Continue to handle events.
                            }
                        }
                    }
                }
            }

            Ok(())
        })?;

        match ctx.run_finished() {
            Ok(()) => {}
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }

        match first_error {
            None => Ok(ctx.run_stats),
            Some(err) => Err(err),
        }
    }

    // ---
    // Helper methods
    // ---

    /// Run an individual test in its own process.
    fn run_test(&self, test: TestInstance<'list>) -> TestRunStatus {
        let start_time = Instant::now();

        match self.run_test_inner(test, &start_time) {
            Ok(run_status) => run_status,
            Err(_) => TestRunStatus {
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: vec![],
                stderr: vec![],
                status: TestStatus::ExecFail,
                time_taken: start_time.elapsed(),
            },
        }
    }

    fn run_test_inner(
        &self,
        test: TestInstance<'list>,
        start_time: &Instant,
    ) -> Result<TestRunStatus> {
        let mut args = vec!["--exact", test.name, "--nocapture"];
        if test.info.ignored {
            args.push("--ignored");
        }
        let mut cmd = cmd(AsRef::<Path>::as_ref(test.binary), args)
            // Capture stdout and stderr.
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
            TestStatus::Pass
        } else {
            TestStatus::Fail
        };
        Ok(TestRunStatus {
            stdout: output.stdout,
            stderr: output.stderr,
            status,
            time_taken,
        })
    }
}

/// Information about a test that finished running.
#[derive(Clone, Debug)]
pub struct TestRunStatus {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: TestStatus,
    pub time_taken: Duration,
}

/// Statistics for a test run.
#[derive(Copy, Clone, Default, Debug)]
pub struct RunStats {
    /// The total number of tests that were expected to be run at the beginning.
    ///
    /// If the test run is canceled, this will be more than `final_run_count`.
    pub initial_run_count: usize,

    /// The total number of tests that were actually run.
    pub final_run_count: usize,

    /// The number of tests that passed.
    pub passed: usize,

    /// The number of tests that failed.
    pub failed: usize,

    /// The number of tests that encountered an execution failure.
    pub exec_failed: usize,

    /// The number of tests that were skipped.
    pub skipped: usize,
}

impl RunStats {
    /// Returns true if this run is considered a success.
    ///
    /// A run can be marked as failed if any of the following are true:
    /// * the run was canceled: the initial run count is greater than the final run count
    /// * any tests failed
    /// * any tests encountered an execution failure
    pub fn is_success(&self) -> bool {
        if self.initial_run_count > self.final_run_count {
            return false;
        }
        if self.failed > 0 || self.exec_failed > 0 {
            return false;
        }
        true
    }
}

fn spawn_signal_thread(sender: Sender<InternalSignalEvent>, srp_sender: Sender<()>) {
    std::thread::spawn(move || {
        use signal_hook::{
            consts::*,
            iterator::{exfiltrator::SignalOnly, SignalsInfo},
        };

        // Register the SignalsInfo.
        let mut signals =
            SignalsInfo::<SignalOnly>::new(TERM_SIGNALS).expect("SignalsInfo created");
        let _ = sender.send(InternalSignalEvent::Handle {
            handle: signals.handle(),
        });
        // Let the run pool know that the signal has been sent.
        let _ = srp_sender.send(());

        let mut term_once = false;

        for signal in &mut signals {
            if term_once {
                // TODO: handle this error better?
                let _ = emulate_default_handler(signal);
            } else {
                term_once = true;
                let _ = sender.send(InternalSignalEvent::Canceled { signal });
            }
        }
    });
}

struct CallbackContext<F, E> {
    callback: F,
    start_time: Instant,
    run_stats: RunStats,
    running: usize,
    signal_handle: Option<Handle>,
    cancel_state: CancelState,
    phantom: PhantomData<E>,
}

impl<'list, F, E> CallbackContext<F, E>
where
    F: FnMut(TestEvent<'list>) -> Result<(), E> + Send,
{
    fn new(callback: F, initial_run_count: usize) -> Self {
        Self {
            callback,
            start_time: Instant::now(),
            run_stats: RunStats {
                initial_run_count,
                ..RunStats::default()
            },
            running: 0,
            signal_handle: None,
            cancel_state: CancelState::None,
            phantom: PhantomData,
        }
    }

    fn run_started(&mut self, test_list: &'list TestList) -> Result<(), E> {
        (self.callback)(TestEvent::RunStarted { test_list })
    }

    fn handle_event(&mut self, event: InternalEvent<'list>) -> Result<(), InternalError<E>> {
        match event {
            InternalEvent::Signal(InternalSignalEvent::Handle { handle }) => {
                self.signal_handle = Some(handle);
                Ok(())
            }
            InternalEvent::Test(InternalTestEvent::Started { test_instance }) => {
                self.running += 1;
                (self.callback)(TestEvent::TestStarted { test_instance })
                    .map_err(InternalError::Error)
            }
            InternalEvent::Test(InternalTestEvent::Finished {
                test_instance,
                run_status,
            }) => {
                self.running -= 1;
                self.run_stats.final_run_count += 1;
                match run_status.status {
                    TestStatus::Pass => self.run_stats.passed += 1,
                    TestStatus::Fail => self.run_stats.failed += 1,
                    TestStatus::ExecFail => self.run_stats.exec_failed += 1,
                }

                (self.callback)(TestEvent::TestFinished {
                    test_instance,
                    run_status,
                })
                .map_err(InternalError::Error)
            }
            InternalEvent::Test(InternalTestEvent::Skipped { test_instance }) => {
                self.run_stats.skipped += 1;
                (self.callback)(TestEvent::TestSkipped { test_instance })
                    .map_err(InternalError::Error)
            }
            InternalEvent::Signal(InternalSignalEvent::Canceled { signal: _signal }) => {
                debug_assert_ne!(
                    self.cancel_state,
                    CancelState::SignalCanceled,
                    "can't receive signal-canceled twice"
                );

                self.cancel_state = CancelState::SignalCanceled;
                // Don't close the signal handle because we're still interested in the second
                // ctrl-c.

                match (self.callback)(TestEvent::RunBeginCancel {
                    running: self.running,
                    reason: CancelReason::Signal,
                }) {
                    Ok(()) => Err(InternalError::SignalCanceled(None)),
                    Err(err) => Err(InternalError::SignalCanceled(Some(err))),
                }
            }
        }
    }

    fn error_cancel(&mut self) -> Result<(), E> {
        if self.cancel_state == CancelState::None {
            self.cancel_state = CancelState::ErrorCanceled;
        }
        (self.callback)(TestEvent::RunBeginCancel {
            running: self.running,
            reason: CancelReason::ReportError,
        })
    }

    fn run_finished(&mut self) -> Result<(), E> {
        (self.callback)(TestEvent::RunFinished {
            start_time: self.start_time,
            run_stats: self.run_stats,
        })
    }

    // TODO: do we ever want to actually close the handle?
    #[allow(dead_code)]
    fn close_handle(&mut self) {
        if let Some(handle) = &self.signal_handle {
            handle.close();
        }
        self.signal_handle = None;
    }
}

#[derive(Debug)]
enum InternalEvent<'list> {
    Test(InternalTestEvent<'list>),
    Signal(InternalSignalEvent),
}

#[derive(Debug)]
enum InternalTestEvent<'list> {
    Started {
        test_instance: TestInstance<'list>,
    },
    Finished {
        test_instance: TestInstance<'list>,
        run_status: TestRunStatus,
    },
    Skipped {
        test_instance: TestInstance<'list>,
    },
}

#[derive(Debug)]
enum InternalSignalEvent {
    Handle { handle: Handle },
    Canceled { signal: c_int },
}

#[derive(Debug)]
enum InternalError<E> {
    Error(E),
    SignalCanceled(Option<E>),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum CancelState {
    None,
    ErrorCanceled,
    SignalCanceled,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TestStatus {
    Pass,
    Fail,
    ExecFail,
}

impl TestStatus {
    /// Returns true if the test was successful.
    pub fn is_success(self) -> bool {
        match self {
            TestStatus::Pass => true,
            TestStatus::Fail | TestStatus::ExecFail => false,
        }
    }
}

impl fmt::Display for TestStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TestStatus::Pass => f.pad("PASS"),
            TestStatus::Fail => f.pad("FAIL"),
            TestStatus::ExecFail => f.pad("EXECFAIL"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_success() {
        assert!(RunStats::default().is_success(), "empty run => success");
        assert!(
            RunStats {
                initial_run_count: 42,
                final_run_count: 42,
                ..RunStats::default()
            }
            .is_success(),
            "initial run count = final run count => success"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                final_run_count: 41,
                ..RunStats::default()
            }
            .is_success(),
            "initial run count > final run count => failure"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                final_run_count: 42,
                failed: 1,
                ..RunStats::default()
            }
            .is_success(),
            "failed => failure"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                final_run_count: 42,
                exec_failed: 1,
                ..RunStats::default()
            }
            .is_success(),
            "exec failed => failure"
        );
        assert!(
            RunStats {
                initial_run_count: 42,
                final_run_count: 42,
                skipped: 1,
                ..RunStats::default()
            }
            .is_success(),
            "skipped => not considered a failure"
        );
    }
}
