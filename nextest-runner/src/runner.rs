// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    reporter::{CancelReason, TestEvent},
    stopwatch::{StopwatchEnd, StopwatchStart},
    test_filter::{FilterMatch, MismatchReason},
    test_list::{TestInstance, TestList},
};
use anyhow::Result;
use crossbeam_channel::Sender;
use duct::cmd;
use rayon::{ThreadPool, ThreadPoolBuilder};
use signal_hook::{iterator::Handle, low_level::emulate_default_handler};
use std::{
    convert::Infallible,
    marker::PhantomData,
    os::raw::c_int,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use structopt::StructOpt;

/// Test runner options.
#[derive(Debug, Default, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct TestRunnerOpts {
    /// Number of tests to run simultaneously [default: physical CPU count]
    #[structopt(short, long, alias = "test-threads")]
    pub test_threads: Option<usize>,
    /// Number of times tests are run on failure (tests that pass on retry will be considered flaky)
    #[structopt(long, default_value = "1")]
    pub tries: usize,
}

impl TestRunnerOpts {
    /// Creates a new test runner.
    pub fn build(self, test_list: &TestList) -> TestRunner {
        let test_threads = self.test_threads.unwrap_or_else(num_cpus::get);
        TestRunner {
            opts: self,
            test_list,
            run_pool: ThreadPoolBuilder::new()
                // The main run_pool closure will need its own thread.
                .num_threads(test_threads + 1)
                .thread_name(|idx| format!("testrunner-run-{}", idx))
                .build()
                .expect("run pool built"),
        }
    }
}

/// Context for running tests.
pub struct TestRunner<'list> {
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

                    if let FilterMatch::Mismatch { reason } = test_instance.info.filter_match {
                        // Failure to send means the receiver was dropped.
                        let _ = this_run_sender.send(InternalTestEvent::Skipped {
                            test_instance,
                            reason,
                        });
                        return;
                    }

                    // Failure to send means the receiver was dropped.
                    let _ = this_run_sender.send(InternalTestEvent::Started { test_instance });

                    let mut run_statuses = vec![];

                    loop {
                        let attempt = run_statuses.len() + 1;

                        let run_status = self
                            .run_test(test_instance, attempt)
                            .into_external(attempt, self.opts.tries);

                        if run_status.status.is_success() {
                            // The test succeeded.
                            run_statuses.push(run_status);
                            break;
                        } else if attempt < self.opts.tries {
                            // Retry this test: send a retry event, then retry the loop.
                            let _ = this_run_sender.send(InternalTestEvent::Retry {
                                test_instance,
                                run_status: run_status.clone(),
                            });
                            run_statuses.push(run_status);
                        } else {
                            // This test failed and is out of retries.
                            run_statuses.push(run_status);
                            break;
                        }
                    }

                    // At this point, either:
                    // * the test has succeeded, or
                    // * the test has failed and we've run out of retries.
                    // In either case, the test is finished.
                    let _ = this_run_sender.send(InternalTestEvent::Finished {
                        test_instance,
                        run_statuses: RunStatuses::new(run_statuses),
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
    fn run_test(&self, test: TestInstance<'list>, attempt: usize) -> InternalRunStatus {
        let stopwatch = StopwatchStart::now();

        match self.run_test_inner(test, attempt, &stopwatch) {
            Ok(run_status) => run_status,
            Err(_) => InternalRunStatus {
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: vec![],
                stderr: vec![],
                status: TestStatus::ExecFail,
                stopwatch_end: stopwatch.end(),
            },
        }
    }

    fn run_test_inner(
        &self,
        test: TestInstance<'list>,
        attempt: usize,
        stopwatch: &StopwatchStart,
    ) -> Result<InternalRunStatus> {
        let mut args = vec!["--exact", test.name, "--nocapture"];
        if test.info.ignored {
            args.push("--ignored");
        }
        let mut cmd = cmd(AsRef::<Path>::as_ref(test.binary), args)
            // Capture stdout and stderr.
            .stdout_capture()
            .stderr_capture()
            .unchecked()
            // Debug environment variable for testing.
            .env("__NEXTEST_ATTEMPT", format!("{}", attempt));

        if let Some(cwd) = test.cwd {
            cmd = cmd.dir(cwd);
        }

        let handle = cmd.start()?;

        // TODO: timeout/kill logic

        let output = handle.into_output()?;

        let status = if output.status.success() {
            TestStatus::Pass
        } else {
            TestStatus::Fail
        };
        Ok(InternalRunStatus {
            stdout: output.stdout,
            stderr: output.stderr,
            status,
            stopwatch_end: stopwatch.end(),
        })
    }
}

/// Information about executions of a test, including retries.
#[derive(Clone, Debug)]
pub struct RunStatuses {
    /// This is guaranteed to be non-empty.
    statuses: Vec<TestRunStatus>,
}

#[allow(clippy::len_without_is_empty)] // RunStatuses is never empty
impl RunStatuses {
    fn new(statuses: Vec<TestRunStatus>) -> Self {
        Self { statuses }
    }

    /// Returns the last run status.
    ///
    /// This status is typically used as the final result.
    pub fn last_status(&self) -> &TestRunStatus {
        self.statuses.last().expect("run statuses is non-empty")
    }

    /// Iterates over all the statuses.
    pub fn iter(&self) -> impl Iterator<Item = &'_ TestRunStatus> + DoubleEndedIterator + '_ {
        self.statuses.iter()
    }

    /// Returns the number of statuses.
    pub fn len(&self) -> usize {
        self.statuses.len()
    }

    pub fn describe(&self) -> RunDescribe<'_> {
        let last_status = self.last_status();
        if last_status.status.is_success() {
            if self.statuses.len() > 1 {
                RunDescribe::Flaky {
                    last_status,
                    prior_statuses: &self.statuses[..self.statuses.len() - 1],
                }
            } else {
                RunDescribe::Success {
                    run_status: last_status,
                }
            }
        } else {
            let first_status = self.statuses.first().expect("run-statuses is non-empty");
            let retries = &self.statuses[1..];
            RunDescribe::Failure {
                first_status,
                last_status,
                retries,
            }
        }
    }
}

/// A description obtained from `RunStatuses`.
pub enum RunDescribe<'a> {
    /// The test was run once and was successful.
    Success { run_status: &'a TestRunStatus },

    /// The test was run more than once. The final result was successful.
    Flaky {
        /// The last, successful status.
        last_status: &'a TestRunStatus,

        /// Previous statuses, none of which are successes.
        prior_statuses: &'a [TestRunStatus],
    },

    /// The test was run once, or possibly multiple times. All runs failed.
    Failure {
        /// The first, failing status.
        first_status: &'a TestRunStatus,

        /// The last, failing status. Same as the first status if no retries were performed.
        last_status: &'a TestRunStatus,

        /// Any retries that were performed. All of these runs failed.
        ///
        /// May be empty.
        retries: &'a [TestRunStatus],
    },
}

/// Information about a single execution of a test.
#[derive(Clone, Debug)]
pub struct TestRunStatus {
    /// The current attempt. In the range `[1, total_attempts]`.
    pub attempt: usize,
    /// The total number of times this test can be run. Equal to `1 + retries`.
    pub total_attempts: usize,
    pub stdout_stderr: Arc<(Vec<u8>, Vec<u8>)>,
    pub status: TestStatus,
    pub start_time: SystemTime,
    pub time_taken: Duration,
}

impl TestRunStatus {
    /// Returns the standard output.
    pub fn stdout(&self) -> &[u8] {
        &self.stdout_stderr.0
    }

    /// Returns the standard error.
    pub fn stderr(&self) -> &[u8] {
        &self.stdout_stderr.1
    }
}

struct InternalRunStatus {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: TestStatus,
    stopwatch_end: StopwatchEnd,
}

impl InternalRunStatus {
    fn into_external(self, attempt: usize, total_attempts: usize) -> TestRunStatus {
        TestRunStatus {
            attempt,
            total_attempts,
            stdout_stderr: Arc::new((self.stdout, self.stderr)),
            status: self.status,
            start_time: self.stopwatch_end.start_time,
            time_taken: self.stopwatch_end.duration,
        }
    }
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

    /// The number of tests that passed. Includes `flaky`.
    pub passed: usize,

    /// The number of tests that passed on retry.
    pub flaky: usize,

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

    fn on_test_finished(&mut self, run_statuses: &RunStatuses) {
        self.final_run_count += 1;
        // run_statuses is guaranteed to have at least one element.
        // * If the last element is success, treat it as success (and possibly flaky).
        // * If the last element is a failure, use it to determine fail/exec fail.
        // Note that this is different from what Maven Surefire does (use the first failure):
        // https://maven.apache.org/surefire/maven-surefire-plugin/examples/rerun-failing-tests.html
        //
        // This is not likely to matter much in practice since failures are likely to be of the
        // same type.
        let last_status = run_statuses.last_status();
        match last_status.status {
            TestStatus::Pass => {
                self.passed += 1;
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            TestStatus::Fail => self.failed += 1,
            TestStatus::ExecFail => self.exec_failed += 1,
        }
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
    stopwatch: StopwatchStart,
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
            stopwatch: StopwatchStart::now(),
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
            InternalEvent::Test(InternalTestEvent::Retry {
                test_instance,
                run_status,
            }) => (self.callback)(TestEvent::TestRetry {
                test_instance,
                run_status,
            })
            .map_err(InternalError::Error),
            InternalEvent::Test(InternalTestEvent::Finished {
                test_instance,
                run_statuses,
            }) => {
                self.running -= 1;
                self.run_stats.on_test_finished(&run_statuses);

                (self.callback)(TestEvent::TestFinished {
                    test_instance,
                    run_statuses,
                })
                .map_err(InternalError::Error)
            }
            InternalEvent::Test(InternalTestEvent::Skipped {
                test_instance,
                reason,
            }) => {
                self.run_stats.skipped += 1;
                (self.callback)(TestEvent::TestSkipped {
                    test_instance,
                    reason,
                })
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
        let stopwatch_end = self.stopwatch.end();
        (self.callback)(TestEvent::RunFinished {
            start_time: stopwatch_end.start_time,
            elapsed: stopwatch_end.duration,
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
    Retry {
        test_instance: TestInstance<'list>,
        run_status: TestRunStatus,
    },
    Finished {
        test_instance: TestInstance<'list>,
        run_statuses: RunStatuses,
    },
    Skipped {
        test_instance: TestInstance<'list>,
        reason: MismatchReason,
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
