// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! The main structure in this module is [`TestRunner`].

use crate::{
    config::NextestProfile,
    list::{TestInstance, TestList},
    reporter::{CancelReason, StatusLevel, TestEvent},
    signal::{SignalEvent, SignalHandler},
    stopwatch::{StopwatchEnd, StopwatchStart},
    target_runner::TargetRunner,
};
use crossbeam_channel::{RecvTimeoutError, Sender};
use nextest_metadata::{FilterMatch, MismatchReason};
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::{
    convert::Infallible,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

/// Test runner options.
#[derive(Debug, Default)]
pub struct TestRunnerBuilder {
    no_capture: bool,
    retries: Option<usize>,
    fail_fast: Option<bool>,
    test_threads: Option<usize>,
}

impl TestRunnerBuilder {
    /// Sets no-capture mode.
    ///
    /// In this mode, tests will always be run serially: `test_threads` will always be 1.
    pub fn set_no_capture(&mut self, no_capture: bool) -> &mut Self {
        self.no_capture = no_capture;
        self
    }

    /// Sets the number of retries for this test runner.
    pub fn set_retries(&mut self, retries: usize) -> &mut Self {
        self.retries = Some(retries);
        self
    }

    /// Sets the fail-fast value for this test runner.
    pub fn set_fail_fast(&mut self, fail_fast: bool) -> &mut Self {
        self.fail_fast = Some(fail_fast);
        self
    }

    /// Sets the number of tests to run simultaneously.
    pub fn set_test_threads(&mut self, test_threads: usize) -> &mut Self {
        self.test_threads = Some(test_threads);
        self
    }

    /// Creates a new test runner.
    pub fn build<'a>(
        self,
        test_list: &'a TestList,
        profile: &NextestProfile<'_>,
        handler: SignalHandler,
        target_runner: TargetRunner,
    ) -> TestRunner<'a> {
        let test_threads = match self.no_capture {
            true => 1,
            false => self.test_threads.unwrap_or_else(num_cpus::get),
        };
        let retries = self.retries.unwrap_or_else(|| profile.retries());
        let fail_fast = self.fail_fast.unwrap_or_else(|| profile.fail_fast());
        let slow_timeout = profile.slow_timeout();

        TestRunner {
            no_capture: self.no_capture,
            // The number of tries = retries + 1.
            tries: retries + 1,
            fail_fast,
            slow_timeout,
            test_list,
            target_runner,
            run_pool: ThreadPoolBuilder::new()
                // The main run_pool closure will need its own thread.
                .num_threads(test_threads + 1)
                .thread_name(|idx| format!("testrunner-run-{}", idx))
                .build()
                .expect("run pool built"),
            wait_pool: ThreadPoolBuilder::new()
                .num_threads(test_threads)
                .thread_name(|idx| format!("testrunner-wait-{}", idx))
                .build()
                .expect("run pool built"),
            handler,
        }
    }
}

/// Context for running tests.
///
/// Created using [`TestRunnerBuilder::build`].
pub struct TestRunner<'a> {
    no_capture: bool,
    tries: usize,
    fail_fast: bool,
    slow_timeout: Duration,
    test_list: &'a TestList<'a>,
    target_runner: TargetRunner,
    run_pool: ThreadPool,
    wait_pool: ThreadPool,
    handler: SignalHandler,
}

impl<'a> TestRunner<'a> {
    /// Executes the listed tests, each one in its own process.
    ///
    /// The callback is called with the results of each test.
    pub fn execute<F>(&self, mut callback: F) -> RunStats
    where
        F: FnMut(TestEvent<'a>) + Send,
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
    /// error, the test run terminates and the callback is no longer called.
    pub fn try_execute<E, F>(&self, callback: F) -> Result<RunStats, E>
    where
        F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
        E: Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        let (run_sender, run_receiver) = crossbeam_channel::unbounded();

        // This is move so that sender is moved into it. When the scope finishes the sender is
        // dropped, and the receiver below completes iteration.

        let canceled = AtomicBool::new(false);
        let canceled_ref = &canceled;

        let mut ctx = CallbackContext::new(callback, self.test_list.run_count(), self.fail_fast);

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

                    if let FilterMatch::Mismatch { reason } = test_instance.test_info.filter_match {
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
                            .run_test(test_instance, attempt, &this_run_sender)
                            .into_external(attempt, self.tries);

                        if run_status.result.is_success() {
                            // The test succeeded.
                            run_statuses.push(run_status);
                            break;
                        } else if attempt < self.tries {
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
                        run_statuses: ExecutionStatuses::new(run_statuses),
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
                    recv(self.handler.receiver) -> internal_event => {
                        match internal_event {
                            Ok(event) => InternalEvent::Signal(event),
                            Err(_) => {
                                // Ignore the signal thread being dropped. This is done for
                                // noop signal handlers.
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
                                let _ = ctx_mut.begin_cancel(CancelReason::ReportError);
                            }
                            InternalError::TestFailureCanceled(None)
                            | InternalError::SignalCanceled(None) => {
                                // Cancellation has begun and no error was returned during that.
                                // Continue to handle events.
                            }
                            InternalError::TestFailureCanceled(Some(err))
                            | InternalError::SignalCanceled(Some(err)) => {
                                // Cancellation has begun and an error was received during
                                // cancellation.
                                if first_error_mut.is_none() {
                                    *first_error_mut = Some(err);
                                }
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
    fn run_test(
        &self,
        test: TestInstance<'a>,
        attempt: usize,
        run_sender: &Sender<InternalTestEvent<'a>>,
    ) -> InternalExecuteStatus {
        let stopwatch = StopwatchStart::now();

        match self.run_test_inner(test, attempt, &stopwatch, run_sender) {
            Ok(run_status) => run_status,
            Err(_) => InternalExecuteStatus {
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: vec![],
                stderr: vec![],
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.end(),
            },
        }
    }

    fn run_test_inner(
        &self,
        test: TestInstance<'a>,
        attempt: usize,
        stopwatch: &StopwatchStart,
        run_sender: &Sender<InternalTestEvent<'a>>,
    ) -> std::io::Result<InternalExecuteStatus> {
        let cmd = test
            .make_expression(self.test_list, &self.target_runner)
            .unchecked()
            // Debug environment variable for testing.
            .env("__NEXTEST_ATTEMPT", format!("{}", attempt));

        let cmd = if self.no_capture {
            cmd
        } else {
            // Capture stdout and stderr.
            cmd.stdout_capture().stderr_capture()
        };

        let handle = cmd.start()?;

        self.wait_pool.in_place_scope(|s| {
            let (sender, receiver) = crossbeam_channel::bounded::<()>(1);
            let wait_handle = &handle;

            // Spawn a task on the threadpool that waits for the test to finish.
            s.spawn(move |_| {
                // This thread is just waiting for the test to finish, we'll handle the output in the main thread
                let _ = wait_handle.wait();
                // We don't care if the receiver got the message or not
                let _ = sender.send(());
            });

            // Continue waiting for the test to finish with a timeout, logging at slow-timeout
            // intervals
            while let Err(error) = receiver.recv_timeout(self.slow_timeout) {
                match error {
                    RecvTimeoutError::Timeout => {
                        let _ = run_sender.send(InternalTestEvent::Slow {
                            test_instance: test,
                            elapsed: stopwatch.elapsed(),
                        });
                    }
                    RecvTimeoutError::Disconnected => {
                        unreachable!("Waiting thread should never drop the sender")
                    }
                }
            }
        });

        let output = handle.into_output()?;

        let status = if output.status.success() {
            ExecutionResult::Pass
        } else {
            ExecutionResult::Fail
        };
        Ok(InternalExecuteStatus {
            stdout: output.stdout,
            stderr: output.stderr,
            result: status,
            stopwatch_end: stopwatch.end(),
        })
    }
}

/// Information about executions of a test, including retries.
#[derive(Clone, Debug)]
pub struct ExecutionStatuses {
    /// This is guaranteed to be non-empty.
    statuses: Vec<ExecuteStatus>,
}

#[allow(clippy::len_without_is_empty)] // RunStatuses is never empty
impl ExecutionStatuses {
    fn new(statuses: Vec<ExecuteStatus>) -> Self {
        Self { statuses }
    }

    /// Returns the last execution status.
    ///
    /// This status is typically used as the final result.
    pub fn last_status(&self) -> &ExecuteStatus {
        self.statuses
            .last()
            .expect("execution statuses is non-empty")
    }

    /// Iterates over all the statuses.
    pub fn iter(&self) -> impl Iterator<Item = &'_ ExecuteStatus> + DoubleEndedIterator + '_ {
        self.statuses.iter()
    }

    /// Returns the number of times the test was executed.
    pub fn len(&self) -> usize {
        self.statuses.len()
    }

    /// Returns a description of self.
    pub fn describe(&self) -> ExecutionDescription<'_> {
        let last_status = self.last_status();
        if last_status.result.is_success() {
            if self.statuses.len() > 1 {
                ExecutionDescription::Flaky {
                    last_status,
                    prior_statuses: &self.statuses[..self.statuses.len() - 1],
                }
            } else {
                ExecutionDescription::Success {
                    single_status: last_status,
                }
            }
        } else {
            let first_status = self
                .statuses
                .first()
                .expect("execution statuses is non-empty");
            let retries = &self.statuses[1..];
            ExecutionDescription::Failure {
                first_status,
                last_status,
                retries,
            }
        }
    }
}

/// A description of test executions obtained from `ExecuteStatuses`.
///
/// This can be used to quickly determine whether a test passed, failed or was flaky.
#[derive(Copy, Clone, Debug)]
pub enum ExecutionDescription<'a> {
    /// The test was run once and was successful.
    Success {
        /// The status of the test.
        single_status: &'a ExecuteStatus,
    },

    /// The test was run more than once. The final result was successful.
    Flaky {
        /// The last, successful status.
        last_status: &'a ExecuteStatus,

        /// Previous statuses, none of which are successes.
        prior_statuses: &'a [ExecuteStatus],
    },

    /// The test was run once, or possibly multiple times. All runs failed.
    Failure {
        /// The first, failing status.
        first_status: &'a ExecuteStatus,

        /// The last, failing status. Same as the first status if no retries were performed.
        last_status: &'a ExecuteStatus,

        /// Any retries that were performed. All of these runs failed.
        ///
        /// May be empty.
        retries: &'a [ExecuteStatus],
    },
}

impl<'a> ExecutionDescription<'a> {
    /// Returns the status level for this `RunDescribe`.
    pub fn status_level(&self) -> StatusLevel {
        match self {
            ExecutionDescription::Success { .. } => StatusLevel::Pass,
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => StatusLevel::Retry,
            ExecutionDescription::Failure { .. } => StatusLevel::Fail,
        }
    }

    /// Returns the last run status.
    pub fn last_status(&self) -> &'a ExecuteStatus {
        match self {
            ExecutionDescription::Success {
                single_status: last_status,
            }
            | ExecutionDescription::Flaky { last_status, .. }
            | ExecutionDescription::Failure { last_status, .. } => last_status,
        }
    }
}

/// Information about a single execution of a test.
#[derive(Clone, Debug)]
pub struct ExecuteStatus {
    /// The current attempt. In the range `[1, total_attempts]`.
    pub attempt: usize,
    /// The total number of times this test can be run. Equal to `1 + retries`.
    pub total_attempts: usize,
    /// Standard output and standard error for this test.
    pub stdout_stderr: Arc<(Vec<u8>, Vec<u8>)>,
    /// The result of execution this test: pass, fail or execution error.
    pub result: ExecutionResult,
    /// The time at which the test started.
    pub start_time: SystemTime,
    /// The time it took for the test to run.
    pub time_taken: Duration,
}

impl ExecuteStatus {
    /// Returns the standard output.
    pub fn stdout(&self) -> &[u8] {
        &self.stdout_stderr.0
    }

    /// Returns the standard error.
    pub fn stderr(&self) -> &[u8] {
        &self.stdout_stderr.1
    }
}

struct InternalExecuteStatus {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result: ExecutionResult,
    stopwatch_end: StopwatchEnd,
}

impl InternalExecuteStatus {
    fn into_external(self, attempt: usize, total_attempts: usize) -> ExecuteStatus {
        ExecuteStatus {
            attempt,
            total_attempts,
            stdout_stderr: Arc::new((self.stdout, self.stderr)),
            result: self.result,
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
    /// If the test run is canceled, this will be more than `finished_count` at the end.
    pub initial_run_count: usize,

    /// The total number of tests that finished running.
    pub finished_count: usize,

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
        if self.initial_run_count > self.finished_count {
            return false;
        }
        if self.failed > 0 || self.exec_failed > 0 {
            return false;
        }
        true
    }

    fn on_test_finished(&mut self, run_statuses: &ExecutionStatuses) {
        self.finished_count += 1;
        // run_statuses is guaranteed to have at least one element.
        // * If the last element is success, treat it as success (and possibly flaky).
        // * If the last element is a failure, use it to determine fail/exec fail.
        // Note that this is different from what Maven Surefire does (use the first failure):
        // https://maven.apache.org/surefire/maven-surefire-plugin/examples/rerun-failing-tests.html
        //
        // This is not likely to matter much in practice since failures are likely to be of the
        // same type.
        let last_status = run_statuses.last_status();
        match last_status.result {
            ExecutionResult::Pass => {
                self.passed += 1;
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResult::Fail => self.failed += 1,
            ExecutionResult::ExecFail => self.exec_failed += 1,
        }
    }
}

struct CallbackContext<F, E> {
    callback: F,
    stopwatch: StopwatchStart,
    run_stats: RunStats,
    fail_fast: bool,
    running: usize,
    cancel_state: Option<CancelReason>,
    phantom: PhantomData<E>,
}

impl<'a, F, E> CallbackContext<F, E>
where
    F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
{
    fn new(callback: F, initial_run_count: usize, fail_fast: bool) -> Self {
        Self {
            callback,
            stopwatch: StopwatchStart::now(),
            run_stats: RunStats {
                initial_run_count,
                ..RunStats::default()
            },
            fail_fast,
            running: 0,
            cancel_state: None,
            phantom: PhantomData,
        }
    }

    fn run_started(&mut self, test_list: &'a TestList) -> Result<(), E> {
        (self.callback)(TestEvent::RunStarted { test_list })
    }

    fn handle_event(&mut self, event: InternalEvent<'a>) -> Result<(), InternalError<E>> {
        match event {
            InternalEvent::Test(InternalTestEvent::Started { test_instance }) => {
                self.running += 1;
                (self.callback)(TestEvent::TestStarted {
                    test_instance,
                    current_stats: self.run_stats,
                    running: self.running,
                    cancel_state: self.cancel_state,
                })
                .map_err(InternalError::Error)
            }
            InternalEvent::Test(InternalTestEvent::Slow {
                test_instance,
                elapsed,
            }) => (self.callback)(TestEvent::TestSlow {
                test_instance,
                elapsed,
            })
            .map_err(InternalError::Error),
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

                // should this run be canceled because of a failure?
                let fail_cancel = self.fail_fast && !run_statuses.last_status().result.is_success();

                (self.callback)(TestEvent::TestFinished {
                    test_instance,
                    run_statuses,
                    current_stats: self.run_stats,
                    running: self.running,
                    cancel_state: self.cancel_state,
                })
                .map_err(InternalError::Error)?;

                if fail_cancel {
                    // A test failed: start cancellation.
                    Err(InternalError::TestFailureCanceled(
                        self.begin_cancel(CancelReason::TestFailure).err(),
                    ))
                } else {
                    Ok(())
                }
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
            InternalEvent::Signal(SignalEvent::Interrupted) => {
                if self.cancel_state == Some(CancelReason::Signal) {
                    // Ctrl-C was pressed twice -- panic in this case.
                    panic!("Ctrl-C pressed twice, exiting immediately");
                }

                Err(InternalError::SignalCanceled(
                    self.begin_cancel(CancelReason::Signal).err(),
                ))
            }
        }
    }

    /// Begin cancellation of a test run. Report it if the current cancel state is less than
    /// the required one.
    fn begin_cancel(&mut self, reason: CancelReason) -> Result<(), E> {
        if self.cancel_state < Some(reason) {
            self.cancel_state = Some(reason);
            (self.callback)(TestEvent::RunBeginCancel {
                running: self.running,
                reason,
            })?;
        }
        Ok(())
    }

    fn run_finished(&mut self) -> Result<(), E> {
        let stopwatch_end = self.stopwatch.end();
        (self.callback)(TestEvent::RunFinished {
            start_time: stopwatch_end.start_time,
            elapsed: stopwatch_end.duration,
            run_stats: self.run_stats,
        })
    }
}

#[derive(Debug)]
enum InternalEvent<'a> {
    Test(InternalTestEvent<'a>),
    Signal(SignalEvent),
}

#[derive(Debug)]
enum InternalTestEvent<'a> {
    Started {
        test_instance: TestInstance<'a>,
    },
    Slow {
        test_instance: TestInstance<'a>,
        elapsed: Duration,
    },
    Retry {
        test_instance: TestInstance<'a>,
        run_status: ExecuteStatus,
    },
    Finished {
        test_instance: TestInstance<'a>,
        run_statuses: ExecutionStatuses,
    },
    Skipped {
        test_instance: TestInstance<'a>,
        reason: MismatchReason,
    },
}

#[derive(Debug)]
enum InternalError<E> {
    Error(E),
    TestFailureCanceled(Option<E>),
    SignalCanceled(Option<E>),
}

/// Whether a test passed, failed or an error occurred while executing the test.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ExecutionResult {
    /// The test passed.
    Pass,
    /// The test failed.
    Fail,
    /// An error occurred while executing the test.
    ExecFail,
}

impl ExecutionResult {
    /// Returns true if the test was successful.
    pub fn is_success(self) -> bool {
        match self {
            ExecutionResult::Pass => true,
            ExecutionResult::Fail | ExecutionResult::ExecFail => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NextestConfig;

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut builder = TestRunnerBuilder::default();
        builder.set_no_capture(true).set_test_threads(20);
        let test_list = TestList::empty();
        let config = NextestConfig::default_config("/fake/dir");
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let handler = SignalHandler::noop();
        let runner = builder.build(&test_list, &profile, handler, TargetRunner::empty());
        assert!(runner.no_capture, "no_capture is true");
        assert_eq!(
            runner.run_pool.current_num_threads(),
            2,
            "tests run serially => run pool has 1 + 1 = 2 threads"
        );
        assert_eq!(
            runner.wait_pool.current_num_threads(),
            1,
            "tests run serially => wait pool has 1 thread"
        );
    }

    #[test]
    fn test_is_success() {
        assert!(RunStats::default().is_success(), "empty run => success");
        assert!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                ..RunStats::default()
            }
            .is_success(),
            "initial run count = final run count => success"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                finished_count: 41,
                ..RunStats::default()
            }
            .is_success(),
            "initial run count > final run count => failure"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed: 1,
                ..RunStats::default()
            }
            .is_success(),
            "failed => failure"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                finished_count: 42,
                exec_failed: 1,
                ..RunStats::default()
            }
            .is_success(),
            "exec failed => failure"
        );
        assert!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                skipped: 1,
                ..RunStats::default()
            }
            .is_success(),
            "skipped => not considered a failure"
        );
    }
}
