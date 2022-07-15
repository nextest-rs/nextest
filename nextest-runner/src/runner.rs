// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! The main structure in this module is [`TestRunner`].

use crate::{
    config::{NextestProfile, ProfileOverrides, TestThreads},
    errors::TestRunnerBuildError,
    helpers::convert_build_platform,
    list::{TestInstance, TestList},
    reporter::{CancelReason, FinalStatusLevel, StatusLevel, TestEvent},
    signal::{SignalEvent, SignalHandler, SignalHandlerKind},
    stopwatch::{StopwatchEnd, StopwatchStart},
    target_runner::TargetRunner,
};
use async_scoped::TokioScope;
use futures::prelude::*;
use nextest_filtering::{BinaryQuery, TestQuery};
use nextest_metadata::{FilterMatch, MismatchReason};
use std::{
    convert::Infallible,
    marker::PhantomData,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::{runtime::Runtime, sync::mpsc::UnboundedSender};

/// Test runner options.
#[derive(Debug, Default)]
pub struct TestRunnerBuilder {
    no_capture: bool,
    retries: Option<usize>,
    fail_fast: Option<bool>,
    test_threads: Option<TestThreads>,
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
    pub fn set_test_threads(&mut self, test_threads: TestThreads) -> &mut Self {
        self.test_threads = Some(test_threads);
        self
    }

    /// Creates a new test runner.
    pub fn build<'a>(
        self,
        test_list: &'a TestList,
        profile: NextestProfile<'a>,
        handler_kind: SignalHandlerKind,
        target_runner: TargetRunner,
    ) -> Result<TestRunner<'a>, TestRunnerBuildError> {
        let test_threads = match self.no_capture {
            true => 1,
            false => self
                .test_threads
                .unwrap_or_else(|| profile.test_threads())
                .compute(),
        };
        let (retries, ignore_retry_overrides) = match self.retries {
            Some(retries) => (retries, true),
            None => (profile.retries(), false),
        };
        let fail_fast = self.fail_fast.unwrap_or_else(|| profile.fail_fast());
        let slow_timeout = profile.slow_timeout();

        let runtime = Runtime::new().map_err(TestRunnerBuildError::TokioRuntimeCreate)?;
        let _guard = runtime.enter();

        // This must be called from within the guard.
        let handler = handler_kind.build()?;

        Ok(TestRunner {
            inner: TestRunnerInner {
                no_capture: self.no_capture,
                profile,
                test_threads,
                // The number of tries = retries + 1.
                global_tries: retries + 1,
                ignore_retry_overrides,
                fail_fast,
                slow_timeout,
                test_list,
                target_runner,
                runtime,
            },
            handler,
        })
    }
}

/// Context for running tests.
///
/// Created using [`TestRunnerBuilder::build`].
#[derive(Debug)]
pub struct TestRunner<'a> {
    inner: TestRunnerInner<'a>,
    handler: SignalHandler,
}

impl<'a> TestRunner<'a> {
    /// Executes the listed tests, each one in its own process.
    ///
    /// The callback is called with the results of each test.
    pub fn execute<F>(&mut self, mut callback: F) -> RunStats
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
    pub fn try_execute<E, F>(&mut self, callback: F) -> Result<RunStats, E>
    where
        F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
        E: Send,
    {
        self.inner.try_execute(&mut self.handler, callback)
    }
}

#[derive(Debug)]
struct TestRunnerInner<'a> {
    no_capture: bool,
    profile: NextestProfile<'a>,
    test_threads: usize,
    global_tries: usize,
    ignore_retry_overrides: bool,
    fail_fast: bool,
    slow_timeout: crate::config::SlowTimeout,
    test_list: &'a TestList<'a>,
    target_runner: TargetRunner,
    runtime: Runtime,
}

impl<'a> TestRunnerInner<'a> {
    fn try_execute<E, F>(
        &self,
        signal_handler: &mut SignalHandler,
        callback: F,
    ) -> Result<RunStats, E>
    where
        F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
        E: Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

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

        let _guard = self.runtime.enter();

        TokioScope::scope_and_block(move |scope| {
            let (run_sender, mut run_receiver) = tokio::sync::mpsc::unbounded_channel();

            let run_fut = futures::stream::iter(self.test_list.iter_tests())
                .map(move |test_instance| {
                    let this_run_sender = run_sender.clone();

                    async move {
                        if canceled_ref.load(Ordering::Acquire) {
                            // Check for test cancellation.
                            return;
                        }

                        let query = TestQuery {
                            binary_query: BinaryQuery {
                                package_id: test_instance.bin_info.package.id(),
                                kind: test_instance.bin_info.kind.as_str(),
                                binary_name: &test_instance.bin_info.binary_name,
                                platform: convert_build_platform(
                                    test_instance.bin_info.build_platform,
                                ),
                            },
                            test_name: test_instance.name,
                        };
                        let overrides = self.profile.overrides_for(&query);
                        let total_attempts =
                            match (self.ignore_retry_overrides, overrides.retries()) {
                                (true, _) | (false, None) => self.global_tries,
                                (false, Some(retries)) => retries + 1,
                            };

                        if let FilterMatch::Mismatch { reason } =
                            test_instance.test_info.filter_match
                        {
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
                                .run_test(test_instance, attempt, &overrides, &this_run_sender)
                                .await
                                .into_external(attempt, total_attempts);

                            if run_status.result.is_success() {
                                // The test succeeded.
                                run_statuses.push(run_status);
                                break;
                            } else if attempt < total_attempts {
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
                    }
                })
                // buffer_unordered means tests are spawned in order but returned in any order.
                .buffer_unordered(self.test_threads)
                .collect();

            // Run the stream to completion.
            scope.spawn_cancellable(run_fut, || ());

            let exec_fut = async move {
                let mut signals_done = false;

                loop {
                    let internal_event = tokio::select! {
                        internal_event = signal_handler.recv(), if !signals_done => {
                            match internal_event {
                                Some(event) => InternalEvent::Signal(event),
                                None => {
                                    signals_done = true;
                                    continue;
                                }
                            }
                        },
                        internal_event = run_receiver.recv() => {
                            match internal_event {
                                Some(event) => InternalEvent::Test(event),
                                None => {
                                    // All runs have been completed.
                                    break;
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
            };

            // Read events from the receiver to completion.
            scope.spawn_cancellable(exec_fut, || ());
        });

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
    async fn run_test(
        &self,
        test: TestInstance<'a>,
        attempt: usize,
        overrides: &ProfileOverrides,
        run_sender: &UnboundedSender<InternalTestEvent<'a>>,
    ) -> InternalExecuteStatus {
        let stopwatch = StopwatchStart::now();

        match self
            .run_test_inner(test, attempt, &stopwatch, overrides, run_sender)
            .await
        {
            Ok(run_status) => run_status,
            Err(_) => InternalExecuteStatus {
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: vec![],
                stderr: vec![],
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.end(),
                is_slow: false,
            },
        }
    }

    async fn run_test_inner(
        &self,
        test: TestInstance<'a>,
        attempt: usize,
        stopwatch: &StopwatchStart,
        overrides: &ProfileOverrides,
        run_sender: &UnboundedSender<InternalTestEvent<'a>>,
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

        let handle = Arc::new(cmd.start()?);

        let mut status: Option<ExecutionResult> = None;
        let slow_timeout = overrides.slow_timeout().unwrap_or(self.slow_timeout);
        let mut is_slow = false;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let wait_handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            // This task is just waiting for the test to finish, we'll handle the output in the main task
            let _ = wait_handle.wait();
            // We don't care if the receiver got the message or not.
            let _ = sender.send(());
        });

        let mut interval = tokio::time::interval(slow_timeout.period);
        // The first tick is immediate.
        interval.tick().await;

        let mut timeout_hit = 0;

        loop {
            tokio::select! {
                biased;

                _ = receiver.recv() => {
                    // The test run finished.
                    break;
                }
                _ = interval.tick(), if status.is_none() => {
                    is_slow = true;
                    timeout_hit += 1;

                    let _ = run_sender.send(InternalTestEvent::Slow {
                        test_instance: test,
                        // Pass in the slow timeout period times timeout_hit, since stopwatch.elapsed() tends to be
                        // slightly longer.
                        elapsed: timeout_hit * slow_timeout.period,
                    });

                    if let Some(terminate_after) = slow_timeout.terminate_after {
                        if NonZeroUsize::new(timeout_hit as usize)
                            .expect("timeout_hit cannot be non-zero")
                            >= terminate_after
                        {
                            // attempt to terminate the slow test.
                            // as there is a race between shutting down a slow test and its own completion
                            // we silently ignore errors to avoid printing false warnings.

                            #[cfg(unix)]
                            let exited = {
                                use duct::unix::HandleExt;
                                use libc::SIGTERM;

                                let _ = handle.send_signal(SIGTERM);

                                // give the process a grace period of 10s
                                let sleep = tokio::time::sleep(Duration::from_secs(10));
                                tokio::select! {
                                    biased;

                                    _ = receiver.recv() => {
                                        // The process exited.
                                        true
                                    }
                                    _ = sleep => {
                                        // The process didn't exit -- need to do a hard shutdown.
                                        false
                                    }
                                }
                            };

                            #[cfg(not(unix))]
                            let exited = false;

                            if !exited {
                                let _ = handle.kill();
                            }

                            status = Some(ExecutionResult::Timeout);
                            // Don't break here to give the wait task a chance to finish. This is
                            // required because we want just one reference to the Arc to remain for
                            // the try_unwrap call below.
                        }
                    }
                }
            };
        }

        let output = Arc::try_unwrap(handle)
            .expect("at this point just one handle should remain")
            .into_output()?;

        let status = status.unwrap_or_else(|| {
            if output.status.success() {
                ExecutionResult::Pass
            } else {
                cfg_if::cfg_if! {
                    if #[cfg(unix)] {
                        // On Unix, extract the signal if it's found.
                        use std::os::unix::process::ExitStatusExt;
                        let abort_status = output.status.signal().map(AbortStatus::UnixSignal);
                    } else if #[cfg(windows)] {
                        let abort_status = output.status.code().and_then(|code| {
                            let exception = windows::Win32::Foundation::NTSTATUS(code);
                            exception.is_err().then(|| AbortStatus::WindowsNtStatus(exception))
                        });
                    } else {
                        let abort_status = None;
                    }
                }
                ExecutionResult::Fail { abort_status }
            }
        });

        Ok(InternalExecuteStatus {
            stdout: output.stdout,
            stderr: output.stderr,
            result: status,
            stopwatch_end: stopwatch.end(),
            is_slow,
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
    /// Returns the status level for this `ExecutionDescription`.
    pub fn status_level(&self) -> StatusLevel {
        match self {
            ExecutionDescription::Success { .. } => StatusLevel::Pass,
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => StatusLevel::Retry,
            ExecutionDescription::Failure { .. } => StatusLevel::Fail,
        }
    }

    /// Returns the final status level for this `ExecutionDescription`.
    pub fn final_status_level(&self) -> FinalStatusLevel {
        match self {
            ExecutionDescription::Success { single_status, .. } => {
                if single_status.is_slow {
                    FinalStatusLevel::Slow
                } else {
                    FinalStatusLevel::Pass
                }
            }
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => FinalStatusLevel::Flaky,
            ExecutionDescription::Failure { .. } => FinalStatusLevel::Fail,
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
    /// Whether this test counts as slow.
    pub is_slow: bool,
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
    is_slow: bool,
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
            is_slow: self.is_slow,
        }
    }
}

/// Statistics for a test run.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
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

    /// The number of tests that timed out.
    pub timed_out: usize,

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
        if self.any_failed() {
            return false;
        }
        true
    }

    /// Returns true if any tests failed or were timed out.
    #[inline]
    pub fn any_failed(&self) -> bool {
        self.failed > 0 || self.exec_failed > 0 || self.timed_out > 0
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
            ExecutionResult::Fail { .. } => self.failed += 1,
            ExecutionResult::Timeout => self.timed_out += 1,
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
    Fail {
        /// The abort status of the test, if any (for example, the signal on Unix).
        abort_status: Option<AbortStatus>,
    },
    /// An error occurred while executing the test.
    ExecFail,
    /// The test was terminated due to timeout.
    Timeout,
}

impl ExecutionResult {
    /// Returns true if the test was successful.
    pub fn is_success(self) -> bool {
        match self {
            ExecutionResult::Pass => true,
            ExecutionResult::Fail { .. } | ExecutionResult::ExecFail | ExecutionResult::Timeout => {
                false
            }
        }
    }
}

/// A signal or other abort status for a test.
///
/// Returned as part of the [`ExecutionResult::Fail`] variant.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AbortStatus {
    /// The test was aborted due to a signal on Unix.
    #[cfg(unix)]
    UnixSignal(i32),

    /// The test was determined to have aborted because the high bit was set on Windows.
    #[cfg(windows)]
    WindowsNtStatus(windows::Win32::Foundation::NTSTATUS),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NextestConfig;

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut builder = TestRunnerBuilder::default();
        builder
            .set_no_capture(true)
            .set_test_threads(TestThreads::Count(20));
        let test_list = TestList::empty();
        let config = NextestConfig::default_config("/fake/dir");
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let handler_kind = SignalHandlerKind::Noop;
        let runner = builder
            .build(&test_list, profile, handler_kind, TargetRunner::empty())
            .unwrap();
        assert!(runner.inner.no_capture, "no_capture is true");
        assert_eq!(runner.inner.test_threads, 1, "tests run serially");
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
            !RunStats {
                initial_run_count: 42,
                finished_count: 42,
                timed_out: 1,
                ..RunStats::default()
            }
            .is_success(),
            "timed out => failure"
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

    #[test]
    fn test_any_failed() {
        assert!(
            !RunStats::default().any_failed(),
            "empty run => none failed"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                finished_count: 41,
                ..RunStats::default()
            }
            .any_failed(),
            "initial run count > final run count doesn't necessarily mean any failed"
        );
        assert!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed: 1,
                ..RunStats::default()
            }
            .any_failed(),
            "failed => failure"
        );
        assert!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                exec_failed: 1,
                ..RunStats::default()
            }
            .any_failed(),
            "exec failed => failure"
        );
        assert!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                timed_out: 1,
                ..RunStats::default()
            }
            .any_failed(),
            "timed out => failure"
        );
        assert!(
            !RunStats {
                initial_run_count: 42,
                finished_count: 42,
                skipped: 1,
                ..RunStats::default()
            }
            .any_failed(),
            "skipped => not considered a failure"
        );
    }
}
