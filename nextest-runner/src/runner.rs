// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! The main structure in this module is [`TestRunner`].

use crate::{
    config::{NextestProfile, ProfileOverrides, TestThreads},
    errors::{ConfigureHandleInheritanceError, TestRunnerBuildError},
    helpers::convert_build_platform,
    list::{TestInstance, TestList},
    reporter::{CancelReason, FinalStatusLevel, StatusLevel, TestEvent},
    signal::{SignalEvent, SignalHandler, SignalHandlerKind},
    stopwatch::{StopwatchEnd, StopwatchStart},
    target_runner::TargetRunner,
};
use async_scoped::TokioScope;
use bytes::Bytes;
use futures::prelude::*;
use nextest_filtering::{BinaryQuery, TestQuery};
use nextest_metadata::{FilterMatch, MismatchReason};
use std::{
    convert::Infallible,
    marker::PhantomData,
    num::NonZeroUsize,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Child,
    runtime::Runtime,
    sync::mpsc::UnboundedSender,
};
use uuid::Uuid;

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
        let leak_timeout = profile.leak_timeout();

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
                leak_timeout,
                test_list,
                target_runner,
                runtime,
                run_id: Uuid::new_v4(),
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
    leak_timeout: Duration,
    test_list: &'a TestList<'a>,
    target_runner: TargetRunner,
    runtime: Runtime,
    run_id: Uuid,
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

        let mut ctx = CallbackContext::new(
            callback,
            self.run_id,
            self.test_list.run_count(),
            self.fail_fast,
        );

        // Send the initial event.
        // (Don't need to set the canceled atomic if this fails because the run hasn't started
        // yet.)
        ctx.run_started(self.test_list)?;

        // Stores the first error that occurred. This error is propagated up.
        let mut first_error = None;

        let ctx_mut = &mut ctx;
        let first_error_mut = &mut first_error;

        let _guard = self.runtime.enter();

        // 4 is greater than the number of messages that will ever be sent over this channel.
        // Also, hold a receiver open so there are no spurious SendErrors on the sender.
        let (forward_sender, _forward_receiver) =
            tokio::sync::broadcast::channel::<SignalForwardEvent>(4);
        let forward_sender_ref = &forward_sender;

        TokioScope::scope_and_block(move |scope| {
            let (run_sender, mut run_receiver) = tokio::sync::mpsc::unbounded_channel();

            {
                let run_fut = futures::stream::iter(self.test_list.iter_tests())
                    .map(move |test_instance| {
                        let this_run_sender = run_sender.clone();

                        async move {
                            // Subscribe to the receiver *before* checking canceled_ref. The ordering is
                            // important to avoid race conditions with the code that first sets
                            // canceled_ref and then sends the notification.
                            let mut this_forward_receiver = forward_sender_ref.subscribe();

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
                            let _ =
                                this_run_sender.send(InternalTestEvent::Started { test_instance });

                            let mut run_statuses = vec![];

                            loop {
                                let attempt = run_statuses.len() + 1;

                                let run_status = self
                                    .run_test(
                                        test_instance,
                                        attempt,
                                        &overrides,
                                        &this_run_sender,
                                        &mut this_forward_receiver,
                                    )
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
            }

            let exec_fut = async move {
                let mut signals_done = false;

                loop {
                    let internal_event = tokio::select! {
                        internal_event = run_receiver.recv() => {
                            match internal_event {
                                Some(event) => InternalEvent::Test(event),
                                None => {
                                    // All runs have been completed.
                                    break;
                                }
                            }
                        },
                        internal_event = signal_handler.recv(), if !signals_done => {
                            match internal_event {
                                Some(event) => InternalEvent::Signal(event),
                                None => {
                                    signals_done = true;
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
                            //
                            // Also note the ordering here: canceled_ref is set *before*
                            // notifications are broadcast. This prevents race conditions.
                            canceled_ref.store(true, Ordering::Release);

                            match err {
                                InternalError::Error(err) => {
                                    // Ignore errors that happen during error cancellation.
                                    if first_error_mut.is_none() {
                                        *first_error_mut = Some(err);
                                    }
                                    let _ = ctx_mut.begin_cancel(CancelReason::ReportError);
                                }
                                InternalError::TestFailureCanceled(err) => {
                                    // A test failure has caused cancellation to begin.
                                    if first_error_mut.is_none() {
                                        *first_error_mut = err;
                                    }
                                }
                                InternalError::SignalCanceled(forward_event, err) => {
                                    // A signal has caused cancellation to begin.
                                    if first_error_mut.is_none() {
                                        *first_error_mut = err;
                                    }
                                    // Let all the child processes know about the signal, and
                                    // continue to handle events.
                                    //
                                    // Ignore errors here: if there are no receivers to cancel, so
                                    // be it. Also note the ordering here: canceled_ref is set
                                    // *before* this is sent.
                                    let _ = forward_sender_ref.send(forward_event);
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
        forward_receiver: &mut tokio::sync::broadcast::Receiver<SignalForwardEvent>,
    ) -> InternalExecuteStatus {
        let stopwatch = StopwatchStart::now();

        match self
            .run_test_inner(
                test,
                attempt,
                &stopwatch,
                overrides,
                run_sender,
                forward_receiver,
            )
            .await
        {
            Ok(run_status) => run_status,
            Err(_) => InternalExecuteStatus {
                // TODO: can we return more information in stdout/stderr? investigate this
                stdout: Bytes::new(),
                stderr: Bytes::new(),
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
        forward_receiver: &mut tokio::sync::broadcast::Receiver<SignalForwardEvent>,
    ) -> std::io::Result<InternalExecuteStatus> {
        let mut cmd = test.make_expression(self.test_list, &self.target_runner);

        // Debug environment variable for testing.
        cmd.env("__NEXTEST_ATTEMPT", format!("{}", attempt));
        cmd.env("NEXTEST_RUN_ID", format!("{}", self.run_id));
        cmd.stdin(Stdio::null());
        imp::cmd_pre_exec(&mut cmd);

        // If creating a job fails, we might be on an old system. Ignore this -- job objects are a
        // best-effort thing.
        let job = imp::Job::new().ok();

        if !self.no_capture {
            // Capture stdout and stderr.
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        };

        let mut cmd = tokio::process::Command::from(cmd);
        let mut child = cmd.spawn()?;

        // If assigning the child to the job fails, ignore this. This can happen if the process has
        // exited.
        let _ = imp::assign_process_to_job(&child, job.as_ref());

        let mut status: Option<ExecutionResult> = None;
        let slow_timeout = overrides.slow_timeout().unwrap_or(self.slow_timeout);
        let leak_timeout = overrides.leak_timeout().unwrap_or(self.leak_timeout);
        let mut is_slow = false;

        let mut interval = tokio::time::interval(slow_timeout.period);
        // The first tick is immediate.
        interval.tick().await;

        let mut timeout_hit = 0;

        let child_stdout = child.stdout.take().map(BufReader::new);
        let child_stderr = child.stderr.take().map(BufReader::new);
        let mut stdout = bytes::BytesMut::with_capacity(4096);
        let mut stderr = bytes::BytesMut::with_capacity(4096);

        let (res, leaked) = {
            // Set up futures for reading from stdout and stderr.
            let stdout_fut = async {
                if let Some(mut child_stdout) = child_stdout {
                    loop {
                        stdout.reserve(4096);
                        let bytes_read = child_stdout.read_buf(&mut stdout).await?;
                        if bytes_read == 0 {
                            break;
                        }
                    }
                }
                Ok::<_, std::io::Error>(())
            };
            tokio::pin!(stdout_fut);
            let mut stdout_done = false;

            let stderr_fut = async {
                if let Some(mut child_stderr) = child_stderr {
                    loop {
                        stderr.reserve(4096);
                        let bytes_read = child_stderr.read_buf(&mut stderr).await?;
                        if bytes_read == 0 {
                            break;
                        }
                    }
                }
                Ok::<_, std::io::Error>(())
            };
            tokio::pin!(stderr_fut);
            let mut stderr_done = false;

            let res = loop {
                tokio::select! {
                    res = &mut stdout_fut, if !stdout_done => {
                        stdout_done = true;
                        res?;
                    }
                    res = &mut stderr_fut, if !stderr_done => {
                        stderr_done = true;
                        res?;
                    }
                    res = child.wait() => {
                        // The test finished executing.
                        break res;
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
                                imp::terminate_child(&mut child, TerminateMode::Timeout, forward_receiver, job.as_ref()).await;
                                status = Some(ExecutionResult::Timeout);
                                // Don't break here to give the wait task a chance to finish.
                            }
                        }
                    }
                    recv = forward_receiver.recv() => {
                        // The sender stays open longer than the whole loop, and the buffer is big
                        // enough for all messages ever sent through this channel, so a RecvError
                        // should never happen.
                        let forward_event = recv.expect("a RecvError should never happen here");

                        imp::terminate_child(&mut child, TerminateMode::Signal(forward_event), forward_receiver, job.as_ref()).await;
                    }
                };
            };

            // Once the process is done executing, wait up to leak_timeout for the pipes to shut down.
            // Previously, this used to hang if spawned grandchildren inherited stdout/stderr but
            // didn't shut down properly. Now, this detects those cases and marks them as leaked.
            let leaked = loop {
                let sleep = tokio::time::sleep(leak_timeout);

                tokio::select! {
                    res = &mut stdout_fut, if !stdout_done => {
                        stdout_done = true;
                        res?;
                    }
                    res = &mut stderr_fut, if !stderr_done => {
                        stderr_done = true;
                        res?;
                    }
                    () = sleep, if !(stdout_done && stderr_done) => {
                        // stdout and/or stderr haven't completed yet. In this case, break the loop
                        // and mark this as leaked.
                        break true;
                    }
                    else => {
                        break false;
                    }
                }
            };

            (res, leaked)
        };

        let output = res?;
        let exit_status = output;

        let status = status.unwrap_or_else(|| {
            if exit_status.success() {
                if leaked {
                    ExecutionResult::Leak
                } else {
                    ExecutionResult::Pass
                }
            } else {
                cfg_if::cfg_if! {
                    if #[cfg(unix)] {
                        // On Unix, extract the signal if it's found.
                        use std::os::unix::process::ExitStatusExt;
                        let abort_status = exit_status.signal().map(AbortStatus::UnixSignal);
                    } else if #[cfg(windows)] {
                        let abort_status = exit_status.code().and_then(|code| {
                            let exception = windows::Win32::Foundation::NTSTATUS(code);
                            exception.is_err().then(|| AbortStatus::WindowsNtStatus(exception))
                        });
                    } else {
                        let abort_status = None;
                    }
                }
                ExecutionResult::Fail {
                    abort_status,
                    leaked,
                }
            }
        });

        Ok(InternalExecuteStatus {
            // TODO: replace with Bytes
            stdout: stdout.freeze(),
            stderr: stderr.freeze(),
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
            ExecutionDescription::Success { single_status } => {
                if single_status.result == ExecutionResult::Leak {
                    StatusLevel::Leak
                } else {
                    StatusLevel::Pass
                }
            }
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => StatusLevel::Retry,
            ExecutionDescription::Failure { .. } => StatusLevel::Fail,
        }
    }

    /// Returns the final status level for this `ExecutionDescription`.
    pub fn final_status_level(&self) -> FinalStatusLevel {
        match self {
            ExecutionDescription::Success { single_status, .. } => {
                // Slow is higher priority than leaky, so return slow first here.
                if single_status.is_slow {
                    FinalStatusLevel::Slow
                } else if single_status.result == ExecutionResult::Leak {
                    FinalStatusLevel::Leak
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
    /// Standard output for this test.
    pub stdout: Bytes,
    /// Standard error for this test.
    pub stderr: Bytes,
    /// The result of execution this test: pass, fail or execution error.
    pub result: ExecutionResult,
    /// The time at which the test started.
    pub start_time: SystemTime,
    /// The time it took for the test to run.
    pub time_taken: Duration,
    /// Whether this test counts as slow.
    pub is_slow: bool,
}

struct InternalExecuteStatus {
    stdout: Bytes,
    stderr: Bytes,
    result: ExecutionResult,
    stopwatch_end: StopwatchEnd,
    is_slow: bool,
}

impl InternalExecuteStatus {
    fn into_external(self, attempt: usize, total_attempts: usize) -> ExecuteStatus {
        ExecuteStatus {
            attempt,
            total_attempts,
            stdout: self.stdout,
            stderr: self.stderr,
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

    /// The number of tests that passed. Includes `passed_slow`, `flaky` and `leaky`.
    pub passed: usize,

    /// The number of slow tests that passed.
    pub passed_slow: usize,

    /// The number of tests that passed on retry.
    pub flaky: usize,

    /// The number of tests that failed.
    pub failed: usize,

    /// The number of failed tests that were slow.
    pub failed_slow: usize,

    /// The number of tests that timed out.
    pub timed_out: usize,

    /// The number of tests that passed but leaked handles.
    pub leaky: usize,

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
                if last_status.is_slow {
                    self.passed_slow += 1;
                }
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResult::Leak => {
                self.passed += 1;
                self.leaky += 1;
                if last_status.is_slow {
                    self.passed_slow += 1;
                }
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResult::Fail { .. } => {
                self.failed += 1;
                if last_status.is_slow {
                    self.failed_slow += 1;
                }
            }
            ExecutionResult::Timeout => self.timed_out += 1,
            ExecutionResult::ExecFail => self.exec_failed += 1,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SignalCount {
    Once,
    Twice,
}

impl SignalCount {
    fn to_forward_event(self, event: SignalEvent) -> SignalForwardEvent {
        match self {
            Self::Once => SignalForwardEvent::Once(event),
            Self::Twice => SignalForwardEvent::Twice,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SignalForwardEvent {
    Once(SignalEvent),
    Twice,
}

struct CallbackContext<F, E> {
    callback: F,
    run_id: Uuid,
    stopwatch: StopwatchStart,
    run_stats: RunStats,
    fail_fast: bool,
    running: usize,
    cancel_state: Option<CancelReason>,
    signal_count: Option<SignalCount>,
    phantom: PhantomData<E>,
}

impl<'a, F, E> CallbackContext<F, E>
where
    F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
{
    fn new(callback: F, run_id: Uuid, initial_run_count: usize, fail_fast: bool) -> Self {
        Self {
            callback,
            run_id,
            stopwatch: StopwatchStart::now(),
            run_stats: RunStats {
                initial_run_count,
                ..RunStats::default()
            },
            fail_fast,
            running: 0,
            cancel_state: None,
            signal_count: None,
            phantom: PhantomData,
        }
    }

    fn run_started(&mut self, test_list: &'a TestList) -> Result<(), E> {
        (self.callback)(TestEvent::RunStarted {
            test_list,
            run_id: self.run_id,
        })
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
            InternalEvent::Signal(event) => {
                let signal_count = self.increment_signal_count();
                let forward_event = signal_count.to_forward_event(event);

                let cancel_reason = match event {
                    #[cfg(unix)]
                    SignalEvent::Hangup | SignalEvent::Term => CancelReason::Signal,
                    SignalEvent::Interrupt => CancelReason::Interrupt,
                };

                Err(InternalError::SignalCanceled(
                    forward_event,
                    self.begin_cancel(cancel_reason).err(),
                ))
            }
        }
    }

    fn increment_signal_count(&mut self) -> SignalCount {
        let new_count = match self.signal_count {
            None => SignalCount::Once,
            Some(SignalCount::Once) => SignalCount::Twice,
            Some(SignalCount::Twice) => {
                // The process was signaled 3 times. Time to panic.
                panic!("Signaled 3 times, exiting immediately");
            }
        };
        self.signal_count = Some(new_count);
        new_count
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
            run_id: self.run_id,
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
    SignalCanceled(SignalForwardEvent, Option<E>),
}

/// Whether a test passed, failed or an error occurred while executing the test.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ExecutionResult {
    /// The test passed.
    Pass,
    /// The test passed but leaked handles. This usually indicates that
    /// a subprocess that inherit standard IO was created, but it didn't shut down when
    /// the test failed.
    ///
    /// This is treated as a pass.
    Leak,
    /// The test failed.
    Fail {
        /// The abort status of the test, if any (for example, the signal on Unix).
        abort_status: Option<AbortStatus>,

        /// Whether a test leaked handles. If set to true, this usually indicates that
        /// a subprocess that inherit standard IO was created, but it didn't shut down when
        /// the test failed.
        leaked: bool,
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
            ExecutionResult::Pass | ExecutionResult::Leak => true,
            ExecutionResult::Fail { .. } | ExecutionResult::ExecFail | ExecutionResult::Timeout => {
                false
            }
        }
    }
}

/// A regular exit code or Windows NT abort status for a test.
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

/// Configures stdout, stdin and stderr inheritance by test processes on Windows.
///
/// With Rust on Windows, these handles can be held open by tests (and therefore by grandchild processes)
/// even if we run the tests with `Stdio::inherit`. This can cause problems with leaky tests.
///
/// This changes global state on the Win32 side, so the application must manage mutual exclusion
/// around it. Call this right before [`TestRunner::try_execute`].
///
/// This is a no-op on non-Windows platforms.
///
/// See [this issue on the Rust repository](https://github.com/rust-lang/rust/issues/54760) for more
/// discussion.
pub fn configure_handle_inheritance(
    no_capture: bool,
) -> Result<(), ConfigureHandleInheritanceError> {
    imp::configure_handle_inheritance_impl(no_capture)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use win32job::JobError;
    use windows::Win32::{
        Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT},
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
            JobObjects::TerminateJobObject,
        },
    };

    pub(super) fn configure_handle_inheritance_impl(
        no_capture: bool,
    ) -> Result<(), ConfigureHandleInheritanceError> {
        fn set_handle_inherit(handle: HANDLE, inherit: bool) -> windows::core::Result<()> {
            let flags = if inherit { HANDLE_FLAG_INHERIT.0 } else { 0 };
            unsafe {
                if SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(flags))
                    .as_bool()
                {
                    Ok(())
                } else {
                    Err(windows::core::Error::from_win32())
                }
            }
        }

        unsafe {
            let stdin = GetStdHandle(STD_INPUT_HANDLE)?;
            // Never inherit stdin.
            set_handle_inherit(stdin, false)?;

            // Inherit stdout and stderr if and only if no_capture is true.

            let stdout = GetStdHandle(STD_OUTPUT_HANDLE)?;
            set_handle_inherit(stdout, no_capture)?;
            let stderr = GetStdHandle(STD_ERROR_HANDLE)?;
            set_handle_inherit(stderr, no_capture)?;
        }

        Ok(())
    }

    pub(super) fn cmd_pre_exec(_cmd: &mut std::process::Command) {
        // TODO: set process group on Windows for better ctrl-C handling.
    }

    /// Wrapper around a Job that implements Send and Sync.
    #[derive(Debug)]
    pub(super) struct Job {
        inner: win32job::Job,
    }

    impl Job {
        pub(super) fn new() -> Result<Self, JobError> {
            Ok(Self {
                inner: win32job::Job::create()?,
            })
        }
    }

    // https://github.com/ohadravid/win32job-rs/issues/1
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    pub(super) fn assign_process_to_job(
        child: &tokio::process::Child,
        job: Option<&Job>,
    ) -> Result<(), JobError> {
        // NOTE: Ideally we'd suspend the process before using ResumeThread for this, but that's currently
        // not possible due to https://github.com/rust-lang/rust/issues/96723 not being stable.
        if let Some(job) = job {
            let handle = match child.raw_handle() {
                Some(handle) => handle,
                None => {
                    // If the handle is missing, the child has exited. Ignore this.
                    return Ok(());
                }
            };

            job.inner.assign_process(handle)?;
        }

        Ok(())
    }

    pub(super) async fn terminate_child(
        child: &mut Child,
        mode: TerminateMode,
        _forward_receiver: &mut tokio::sync::broadcast::Receiver<SignalForwardEvent>,
        job: Option<&Job>,
    ) {
        // Ignore signal events since Windows propagates them to child processes (this may change if
        // we start assigning processes to groups on Windows).
        if mode != TerminateMode::Timeout {
            return;
        }
        if let Some(job) = job {
            let handle = job.inner.handle();
            unsafe {
                // Ignore the error here -- it's likely due to the process exiting.
                // Note: 1 is the exit code returned by Windows.
                TerminateJobObject(HANDLE(handle as isize), 1);
            }
        }
        // Start killing the process directly for good measure.
        let _ = child.start_kill();
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use libc::{SIGHUP, SIGINT, SIGKILL, SIGTERM};
    use std::os::unix::process::CommandExt;

    // This is a no-op on non-windows platforms.
    pub(super) fn configure_handle_inheritance_impl(
        _no_capture: bool,
    ) -> Result<(), ConfigureHandleInheritanceError> {
        Ok(())
    }

    /// Pre-execution configuration on Unix.
    ///
    /// This sets up just the process group ID.
    #[cfg(process_group)]
    pub(super) fn cmd_pre_exec(cmd: &mut std::process::Command) {
        cmd.process_group(0);
    }

    /// Pre-execution configuration on Unix.
    ///
    /// This sets up just the process group ID.
    #[cfg(not(process_group))]
    pub(super) fn cmd_pre_exec(cmd: &mut std::process::Command) {
        unsafe {
            // TODO: replace with process_group once Rust 1.64 is out -- that will let this use the
            // posix_spawn fast path, which is significantly faster (0.5 seconds vs 1.5 on clap).
            cmd.pre_exec(|| {
                let pid = libc::getpid();
                if libc::setpgid(pid, pid) == 0 {
                    Ok(())
                } else {
                    // This is an error.
                    Err(std::io::Error::last_os_error())
                }
            })
        };
    }

    #[derive(Debug)]
    pub(super) struct Job(());

    impl Job {
        pub(super) fn new() -> Result<Self, Infallible> {
            Ok(Self(()))
        }
    }

    pub(super) fn assign_process_to_job(
        _child: &tokio::process::Child,
        _job: Option<&Job>,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    pub(super) async fn terminate_child(
        child: &mut Child,
        mode: TerminateMode,
        forward_receiver: &mut tokio::sync::broadcast::Receiver<SignalForwardEvent>,
        _job: Option<&Job>,
    ) {
        match child.id() {
            Some(pid) => {
                let pid = pid as i32;
                let term_signal = match mode {
                    TerminateMode::Timeout => SIGTERM,
                    TerminateMode::Signal(SignalForwardEvent::Once(SignalEvent::Hangup)) => SIGHUP,
                    TerminateMode::Signal(SignalForwardEvent::Once(SignalEvent::Term)) => SIGTERM,
                    TerminateMode::Signal(SignalForwardEvent::Once(SignalEvent::Interrupt)) => {
                        SIGINT
                    }
                    TerminateMode::Signal(SignalForwardEvent::Twice) => SIGKILL,
                };
                unsafe {
                    // We set up a process group in cmd_pre_exec -- now
                    // send a signal to that group.
                    libc::kill(-pid, term_signal)
                };

                if term_signal == SIGKILL {
                    // SIGKILL guarantees the process group is dead.
                    return;
                }

                // give the process a grace period of 10s
                let sleep = tokio::time::sleep(Duration::from_secs(10));
                tokio::select! {
                    biased;

                    _ = child.wait() => {
                        // The process exited.
                    }
                    recv = forward_receiver.recv() => {
                        // The sender stays open longer than the whole loop, and the buffer is big
                        // enough for all messages ever sent through this channel, so a RecvError
                        // should never happen.
                        let _ = recv.expect("a RecvError should never happen here");

                        // Receiving a signal while in this state always means kill immediately.
                        unsafe {
                            // Send SIGKILL to the entire process group.
                            libc::kill(-pid, SIGKILL);
                        }
                    }
                    _ = sleep => {
                        // The process didn't exit -- need to do a hard shutdown.
                        unsafe {
                            // Send SIGKILL to the entire process group.
                            libc::kill(-pid, SIGKILL);
                        }
                    }
                }
            }
            None => {
                // This means that the process has already exited.
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminateMode {
    Timeout,
    Signal(SignalForwardEvent),
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
