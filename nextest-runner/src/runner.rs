// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! The main structure in this module is [`TestRunner`].

use crate::{
    config::{
        EvaluatableProfile, RetryPolicy, ScriptConfig, ScriptId, SetupScriptCommand,
        SetupScriptEnvMap, SetupScriptExecuteData, SlowTimeout, TestGroup, TestSettings,
        TestThreads,
    },
    double_spawn::DoubleSpawnInfo,
    errors::{
        ChildError, ChildFdError, ChildStartError, ConfigureHandleInheritanceError, ErrorList,
        TestRunnerBuildError, TestRunnerExecuteErrors,
    },
    list::{TestExecuteContext, TestInstance, TestInstanceId, TestList},
    reporter::{
        CancelReason, FinalStatusLevel, StatusLevel, TestEvent, TestEventKind, TestOutputDisplay,
    },
    signal::{JobControlEvent, ShutdownEvent, SignalEvent, SignalHandler, SignalHandlerKind},
    target_runner::TargetRunner,
    test_command::{ChildAccumulator, ChildFds},
    test_output::{CaptureStrategy, ChildExecutionResult},
    time::{PausableSleep, StopwatchSnapshot, StopwatchStart},
};
use async_scoped::TokioScope;
use chrono::{DateTime, FixedOffset, Local};
use future_queue::StreamExt;
use futures::prelude::*;
use nextest_metadata::{FilterMatch, MismatchReason};
use quick_junit::ReportUuid;
use rand::{distributions::OpenClosed01, thread_rng, Rng};
use std::{
    collections::BTreeMap,
    convert::Infallible,
    num::NonZeroUsize,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    process::Child,
    runtime::Runtime,
    sync::{
        broadcast,
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinError,
};
use tracing::{debug, warn};

#[derive(Debug)]
struct BackoffIter {
    policy: RetryPolicy,
    current_factor: f64,
    remaining_attempts: usize,
}

impl BackoffIter {
    const BACKOFF_EXPONENT: f64 = 2.;

    fn new(policy: RetryPolicy) -> Self {
        let remaining_attempts = policy.count();
        Self {
            policy,
            current_factor: 1.,
            remaining_attempts,
        }
    }

    fn next_delay_and_jitter(&mut self) -> (Duration, bool) {
        match self.policy {
            RetryPolicy::Fixed { delay, jitter, .. } => (delay, jitter),
            RetryPolicy::Exponential {
                delay,
                jitter,
                max_delay,
                ..
            } => {
                let factor = self.current_factor;
                let exp_delay = delay.mul_f64(factor);

                // Stop multiplying the exponential factor if delay is greater than max_delay.
                if let Some(max_delay) = max_delay {
                    if exp_delay > max_delay {
                        return (max_delay, jitter);
                    }
                }

                let next_factor = self.current_factor * Self::BACKOFF_EXPONENT;
                self.current_factor = next_factor;

                (exp_delay, jitter)
            }
        }
    }

    fn apply_jitter(duration: Duration) -> Duration {
        let jitter: f64 = thread_rng().sample(OpenClosed01);
        // Apply jitter in the range (0.5, 1].
        duration.mul_f64(0.5 + jitter / 2.)
    }
}

impl Iterator for BackoffIter {
    type Item = Duration;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_attempts > 0 {
            let (mut delay, jitter) = self.next_delay_and_jitter();
            if jitter {
                delay = Self::apply_jitter(delay);
            }
            self.remaining_attempts -= 1;
            Some(delay)
        } else {
            None
        }
    }
}

/// Test runner options.
#[derive(Debug, Default)]
pub struct TestRunnerBuilder {
    capture_strategy: CaptureStrategy,
    retries: Option<RetryPolicy>,
    fail_fast: Option<bool>,
    test_threads: Option<TestThreads>,
}

impl TestRunnerBuilder {
    /// Sets the capture strategy for the test runner
    ///
    /// * [`CaptureStrategy::Split`]
    ///   * pro: output from `stdout` and `stderr` can be identified and easily split
    ///   * con: ordering between the streams cannot be guaranteed
    /// * [`CaptureStrategy::Combined`]
    ///   * pro: output is guaranteed to be ordered as it would in a terminal emulator
    ///   * con: distinction between `stdout` and `stderr` is lost
    /// * [`CaptureStrategy::None`] -
    ///   * In this mode, tests will always be run serially: `test_threads` will always be 1.
    pub fn set_capture_strategy(&mut self, strategy: CaptureStrategy) -> &mut Self {
        self.capture_strategy = strategy;
        self
    }

    /// Sets the number of retries for this test runner.
    pub fn set_retries(&mut self, retries: RetryPolicy) -> &mut Self {
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
        profile: &'a EvaluatableProfile<'a>,
        cli_args: Vec<String>,
        handler_kind: SignalHandlerKind,
        double_spawn: DoubleSpawnInfo,
        target_runner: TargetRunner,
    ) -> Result<TestRunner<'a>, TestRunnerBuildError> {
        let test_threads = match self.capture_strategy {
            CaptureStrategy::None => 1,
            CaptureStrategy::Combined | CaptureStrategy::Split => self
                .test_threads
                .unwrap_or_else(|| profile.test_threads())
                .compute(),
        };
        let fail_fast = self.fail_fast.unwrap_or_else(|| profile.fail_fast());

        let runtime = Runtime::new().map_err(TestRunnerBuildError::TokioRuntimeCreate)?;
        let _guard = runtime.enter();

        // This must be called from within the guard.
        let handler = handler_kind.build()?;

        Ok(TestRunner {
            inner: TestRunnerInner {
                capture_strategy: self.capture_strategy,
                profile,
                cli_args,
                test_threads,
                force_retries: self.retries,
                fail_fast,
                test_list,
                double_spawn,
                target_runner,
                runtime,
                run_id: ReportUuid::new_v4(),
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
    ///
    /// Returns an error if any of the tasks panicked.
    pub fn execute<F>(
        self,
        mut callback: F,
    ) -> Result<RunStats, TestRunnerExecuteErrors<Infallible>>
    where
        F: FnMut(TestEvent<'a>) + Send,
    {
        self.try_execute::<Infallible, _>(|test_event| {
            callback(test_event);
            Ok(())
        })
    }

    /// Executes the listed tests, each one in its own process.
    ///
    /// Accepts a callback that is called with the results of each test. If the callback returns an
    /// error, the test run terminates and the callback is no longer called.
    ///
    /// Returns an error if any of the tasks panicked.
    pub fn try_execute<E, F>(
        mut self,
        mut callback: F,
    ) -> Result<RunStats, TestRunnerExecuteErrors<E>>
    where
        F: FnMut(TestEvent<'a>) -> Result<(), E> + Send,
        E: Send,
    {
        let (report_cancel_tx, report_cancel_rx) = oneshot::channel();

        // If report_cancel_tx is None, at least one error has occurred and the
        // runner has been instructed to shut down. first_error is also set to
        // Some in that case.
        let mut report_cancel_tx = Some(report_cancel_tx);
        let mut first_error = None;

        let res = self
            .inner
            .execute(&mut self.handler, report_cancel_rx, |event| {
                match callback(event) {
                    Ok(()) => {}
                    Err(error) => {
                        // If the callback fails, we need to let the runner know to start shutting
                        // down. But we keep reporting results in case the callback starts working
                        // again.
                        if let Some(report_cancel_tx) = report_cancel_tx.take() {
                            let _ = report_cancel_tx.send(());
                            first_error = Some(error);
                        }
                    }
                }
            });

        // On Windows, the stdout and stderr futures might spawn processes that keep the runner
        // stuck indefinitely if it's dropped the normal way. Shut it down aggressively, being OK
        // with leaked resources.
        self.inner.runtime.shutdown_background();

        match (res, first_error) {
            (Ok(run_stats), None) => Ok(run_stats),
            (Ok(_), Some(report_error)) => Err(TestRunnerExecuteErrors {
                report_error: Some(report_error),
                join_errors: Vec::new(),
            }),
            (Err(join_errors), report_error) => Err(TestRunnerExecuteErrors {
                report_error,
                join_errors,
            }),
        }
    }
}

#[derive(Debug)]
struct TestRunnerInner<'a> {
    capture_strategy: CaptureStrategy,
    profile: &'a EvaluatableProfile<'a>,
    cli_args: Vec<String>,
    test_threads: usize,
    // This is Some if the user specifies a retry policy over the command-line.
    force_retries: Option<RetryPolicy>,
    fail_fast: bool,
    test_list: &'a TestList<'a>,
    double_spawn: DoubleSpawnInfo,
    target_runner: TargetRunner,
    runtime: Runtime,
    run_id: ReportUuid,
}

impl<'a> TestRunnerInner<'a> {
    fn execute<F>(
        &self,
        signal_handler: &mut SignalHandler,
        report_cancel_rx: oneshot::Receiver<()>,
        callback: F,
    ) -> Result<RunStats, Vec<JoinError>>
    where
        F: FnMut(TestEvent<'a>) + Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        let cancelled = AtomicBool::new(false);
        let cancelled_ref = &cancelled;

        let mut ctx = CallbackContext::new(
            callback,
            self.run_id,
            self.profile.name(),
            self.cli_args.clone(),
            self.test_list.run_count(),
            self.fail_fast,
        );

        // Send the initial event.
        // (Don't need to set the cancelled atomic if this fails because the run hasn't started
        // yet.)
        ctx.run_started(self.test_list);

        let ctx_mut = &mut ctx;

        let _guard = self.runtime.enter();

        let mut report_cancel_rx = std::pin::pin!(report_cancel_rx);

        let ((), results) = TokioScope::scope_and_block(move |scope| {
            let (resp_tx, mut resp_rx) = unbounded_channel();
            let (cancellation_sender, _cancel_receiver) = broadcast::channel(1);

            let exec_cancellation_sender = cancellation_sender.clone();
            let exec_fut = async move {
                let mut signals_done = false;
                let mut report_cancel_rx_done = false;

                loop {
                    let internal_event = tokio::select! {
                        internal_event = resp_rx.recv() => {
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
                        res = &mut report_cancel_rx, if !report_cancel_rx_done => {
                            report_cancel_rx_done = true;
                            match res {
                                Ok(()) => {
                                    InternalEvent::ReportCancel
                                }
                                Err(_) => {
                                    // In normal operation, the sender is kept alive until the end
                                    // of the run, so this should never fail. However there are
                                    // circumstances around shutdown where it may be possible that
                                    // the sender isn't kept alive. In those cases, we just ignore
                                    // the error and carry on.
                                    debug!(
                                        "report_cancel_rx was dropped early: \
                                         shutdown ordering issue?",
                                    );
                                    continue;
                                }
                            }
                        }
                    };

                    match ctx_mut.handle_event(internal_event) {
                        #[cfg(unix)]
                        Ok(Some(HandleEventResponse::JobControl(JobControlEvent::Stop))) => {
                            // This is in reality bounded by the number of tests
                            // currently running.
                            let (status_tx, mut status_rx) = unbounded_channel();
                            ctx_mut.broadcast_request(RunUnitRequest::Signal(SignalRequest::Stop(
                                status_tx,
                            )));

                            debug!(
                                remaining = status_rx.sender_strong_count(),
                                "stopping tests"
                            );

                            // There's a possibility of a race condition between
                            // a test exiting and sending the message to the
                            // receiver. For that reason, don't wait more than
                            // 100ms on children to stop.
                            let mut sleep =
                                std::pin::pin!(tokio::time::sleep(Duration::from_millis(100)));

                            loop {
                                tokio::select! {
                                    res = status_rx.recv() => {
                                        debug!(
                                            res = ?res,
                                            remaining = status_rx.sender_strong_count(),
                                            "test stopped",
                                        );
                                        if res.is_none() {
                                            // No remaining message in the
                                            // channel's buffer.
                                            break;
                                        }
                                    }
                                    _ = &mut sleep => {
                                        debug!(
                                            remaining = status_rx.sender_strong_count(),
                                            "timeout waiting for tests to stop, ignoring",
                                        );
                                        break;
                                    }
                                };
                            }

                            // Now stop nextest itself.
                            imp::raise_stop();
                        }
                        #[cfg(unix)]
                        Ok(Some(HandleEventResponse::JobControl(JobControlEvent::Continue))) => {
                            // Nextest has been resumed. Resume all the tests as well.
                            ctx_mut
                                .broadcast_request(RunUnitRequest::Signal(SignalRequest::Continue));
                        }
                        #[cfg(not(unix))]
                        Ok(Some(HandleEventResponse::JobControl(e))) => {
                            // On platforms other than Unix this enum is expected to be empty;
                            // we can check this assumption at compile time like so.
                            //
                            // Rust 1.82 handles empty enums better, and this
                            // won't be required after we bump the MSRV to that.
                            match e {}
                        }
                        Ok(None) => {}
                        Err(cancel) => {
                            // A cancellation notice was received. Note the ordering here:
                            // cancelled_ref is set *before* notifications are broadcast. This
                            // prevents race conditions.
                            cancelled_ref.store(true, Ordering::Release);
                            let _ = exec_cancellation_sender.send(());
                            match cancel {
                                // Some of the branches here don't do anything, but are specified
                                // for readability.
                                InternalCancel::Report => {
                                    // An error was produced by the reporter, and cancellation has
                                    // begun.
                                }
                                InternalCancel::TestFailure => {
                                    // A test failure has caused cancellation to begin. Nothing to
                                    // do here.
                                }
                                InternalCancel::Signal(req) => {
                                    // A signal has caused cancellation to begin. Let all the child
                                    // processes know about the signal, and continue to handle
                                    // events.
                                    //
                                    // Ignore errors here: if there are no receivers to cancel, so
                                    // be it. Also note the ordering here: cancelled_ref is set
                                    // *before* this is sent.
                                    ctx_mut.broadcast_request(RunUnitRequest::Signal(
                                        SignalRequest::Shutdown(req),
                                    ));
                                }
                            }
                        }
                    }
                }
            };

            // Read events from the receiver to completion.
            scope.spawn_cancellable(exec_fut, || ());

            {
                let setup_scripts = self.profile.setup_scripts(self.test_list);
                let total = setup_scripts.len();
                debug!("running {} setup scripts", total);

                let mut setup_script_data = SetupScriptExecuteData::new();

                // Run setup scripts one by one.
                for (index, script) in setup_scripts.into_iter().enumerate() {
                    let this_resp_tx = resp_tx.clone();
                    let (completion_sender, completion_receiver) = oneshot::channel();

                    let script_id = script.id.clone();
                    let config = script.config;

                    let script_fut = async move {
                        if cancelled_ref.load(Ordering::Acquire) {
                            // Check for test cancellation.
                            return;
                        }

                        let (req_rx_tx, req_rx_rx) = oneshot::channel();
                        let _ = this_resp_tx.send(InternalTestEvent::SetupScriptStarted {
                            script_id: script_id.clone(),
                            config,
                            index,
                            total,
                            req_rx_tx,
                        });
                        let mut req_rx = match req_rx_rx.await {
                            Ok(req_rx) => req_rx,
                            Err(_) => {
                                // The receiver was dropped -- most likely the
                                // test exited.
                                return;
                            }
                        };

                        let packet = SetupScriptPacket {
                            script_id: script_id.clone(),
                            config,
                        };

                        let status = self
                            .run_setup_script(packet, &this_resp_tx, &mut req_rx)
                            .await;
                        let (status, env_map) = status.into_external();

                        let _ = this_resp_tx.send(InternalTestEvent::SetupScriptFinished {
                            script_id,
                            config,
                            index,
                            total,
                            status,
                        });

                        drain_req_rx(req_rx);
                        _ = completion_sender.send(env_map.map(|env_map| (script, env_map)));
                    };

                    // Run this setup script to completion.
                    scope.spawn_cancellable(script_fut, || ());
                    let script_and_env_map = completion_receiver.blocking_recv().unwrap_or_else(|_| {
                        // This should never happen.
                        warn!("setup script future did not complete -- this is a bug, please report it");
                        None
                    });
                    if let Some((script, env_map)) = script_and_env_map {
                        setup_script_data.add_script(script, env_map);
                    }
                }

                // groups is going to be passed to future_queue_grouped.
                let groups = self
                    .profile
                    .test_group_config()
                    .iter()
                    .map(|(group_name, config)| (group_name, config.max_threads.compute()));

                let setup_script_data = Arc::new(setup_script_data);

                let run_fut = futures::stream::iter(self.test_list.iter_tests())
                    .map(move |test_instance| {
                        let this_resp_tx = resp_tx.clone();
                        let mut cancel_receiver = cancellation_sender.subscribe();

                        let query = test_instance.to_test_query();
                        let settings = self.profile.settings_for(&query);
                        let setup_script_data = setup_script_data.clone();
                        let threads_required =
                            settings.threads_required().compute(self.test_threads);
                        let test_group = match settings.test_group() {
                            TestGroup::Global => None,
                            TestGroup::Custom(name) => Some(name.clone()),
                        };

                        let fut = async move {
                            if cancelled_ref.load(Ordering::Acquire) {
                                // Check for test cancellation.
                                return;
                            }

                            let retry_policy =
                                self.force_retries.unwrap_or_else(|| settings.retries());
                            let total_attempts = retry_policy.count() + 1;
                            let mut backoff_iter = BackoffIter::new(retry_policy);

                            if let FilterMatch::Mismatch { reason } =
                                test_instance.test_info.filter_match
                            {
                                // Failure to send means the receiver was dropped.
                                let _ = this_resp_tx.send(InternalTestEvent::Skipped {
                                    test_instance,
                                    reason,
                                });
                                return;
                            }

                            let (req_rx_tx, req_rx_rx) = oneshot::channel();

                            // Wait for the Started event to be processed by the
                            // execution future.
                            _ = this_resp_tx.send(InternalTestEvent::Started {
                                test_instance,
                                req_rx_tx,
                            });
                            let mut req_rx = match req_rx_rx.await {
                                Ok(rx) => rx,
                                Err(_) => {
                                    // The receiver was dropped, which means the
                                    // test was cancelled.
                                    return;
                                }
                            };

                            let mut attempt = 0;
                            let mut delay = Duration::ZERO;
                            let last_run_status = loop {
                                attempt += 1;
                                let retry_data = RetryData {
                                    attempt,
                                    total_attempts,
                                };

                                // Note: do not check for cancellation here.
                                // Only check for cancellation after the first
                                // run, to avoid a situation where run_statuses
                                // is empty.

                                if retry_data.attempt > 1 {
                                    _ = this_resp_tx.send(InternalTestEvent::RetryStarted {
                                        test_instance,
                                        retry_data,
                                    });
                                }

                                // Some of this information is only useful for event reporting, but
                                // it's a lot easier to pass it in than to try and hook on
                                // additional information later.
                                let packet = TestPacket {
                                    test_instance,
                                    retry_data,
                                    settings: &settings,
                                    setup_script_data: &setup_script_data,
                                    delay_before_start: delay,
                                };

                                let run_status = self
                                    .run_test(packet, &this_resp_tx, &mut req_rx)
                                    .await
                                    .into_external(retry_data);

                                if run_status.result.is_success() {
                                    // The test succeeded.
                                    break run_status;
                                } else if cancelled_ref.load(Ordering::Acquire) {
                                    // The test was cancelled.
                                    break run_status;
                                } else if retry_data.attempt < retry_data.total_attempts {
                                    // Retry this test: send a retry event, then retry the loop.
                                    delay = backoff_iter
                                        .next()
                                        .expect("backoff delay must be non-empty");

                                    let _ = this_resp_tx.send(
                                        InternalTestEvent::AttemptFailedWillRetry {
                                            test_instance,
                                            failure_output: settings.failure_output(),
                                            run_status,
                                            delay_before_next_attempt: delay,
                                        },
                                    );

                                    tokio::select! {
                                        _ = tokio::time::sleep(delay) => {}
                                        // Cancel the sleep if the run is cancelled.
                                        _ = cancel_receiver.recv() => {
                                            // Don't need to do anything special for this because
                                            // cancel_receiver gets a message after
                                            // cancelled_ref is set.
                                        }
                                    }
                                } else {
                                    // This test failed and is out of retries.
                                    break run_status;
                                }
                            };

                            // At this point, either:
                            // * the test has succeeded, or
                            // * the test has failed and we've run out of retries.
                            // In either case, the test is finished.
                            let _ = this_resp_tx.send(InternalTestEvent::Finished {
                                test_instance,
                                success_output: settings.success_output(),
                                failure_output: settings.failure_output(),
                                junit_store_success_output: settings.junit_store_success_output(),
                                junit_store_failure_output: settings.junit_store_failure_output(),
                                last_run_status,
                            });

                            drain_req_rx(req_rx);
                        };
                        (threads_required, test_group, fut)
                    })
                    // future_queue_grouped means tests are spawned in order but returned in
                    // any order.
                    .future_queue_grouped(self.test_threads, groups)
                    .collect();

                // Run the stream to completion.
                scope.spawn_cancellable(run_fut, || ());
            }
        });

        ctx.run_finished();

        // Were there any join errors?
        let join_errors = results
            .into_iter()
            .filter_map(|r| r.err())
            .collect::<Vec<_>>();
        if !join_errors.is_empty() {
            return Err(join_errors);
        }
        Ok(ctx.run_stats)
    }

    // ---
    // Helper methods
    // ---

    /// Run an individual setup script in its own process.
    async fn run_setup_script(
        &self,
        script: SetupScriptPacket<'a>,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest>,
    ) -> InternalSetupScriptExecuteStatus {
        let mut stopwatch = crate::time::stopwatch();

        match self
            .run_setup_script_inner(script, &mut stopwatch, resp_tx, req_rx)
            .await
        {
            Ok(status) => status,
            Err(error) => InternalSetupScriptExecuteStatus {
                output: ChildExecutionResult::StartError(error),
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.snapshot(),
                is_slow: false,
                env_map: None,
            },
        }
    }

    async fn run_setup_script_inner(
        &self,
        script: SetupScriptPacket<'a>,
        stopwatch: &mut StopwatchStart,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest>,
    ) -> Result<InternalSetupScriptExecuteStatus, ChildStartError> {
        let mut cmd = script.make_command(&self.double_spawn, self.test_list)?;
        let command_mut = cmd.command_mut();

        command_mut.env("NEXTEST_RUN_ID", format!("{}", self.run_id));
        command_mut.stdin(Stdio::null());
        imp::set_process_group(command_mut);

        // If creating a job fails, we might be on an old system. Ignore this -- job objects are a
        // best-effort thing.
        let job = imp::Job::create().ok();

        // The --no-capture CLI argument overrides the config.
        if self.capture_strategy != CaptureStrategy::None {
            if script.config.capture_stdout {
                command_mut.stdout(std::process::Stdio::piped());
            }
            if script.config.capture_stderr {
                command_mut.stderr(std::process::Stdio::piped());
            }
        }

        let (mut child, env_path) = cmd
            .spawn()
            .map_err(|error| ChildStartError::Spawn(Arc::new(error)))?;

        // If assigning the child to the job fails, ignore this. This can happen if the process has
        // exited.
        let _ = imp::assign_process_to_job(&child, job.as_ref());

        let mut status: Option<ExecutionResult> = None;
        // Unlike with tests, we don't automatically assume setup scripts are slow if they take a
        // long time. For example, consider a setup script that performs a cargo build -- it can
        // take an indeterminate amount of time. That's why we set a very large slow timeout rather
        // than the test default of 60 seconds.
        let slow_timeout = script
            .config
            .slow_timeout
            .unwrap_or(SlowTimeout::VERY_LARGE);
        let leak_timeout = script
            .config
            .leak_timeout
            .unwrap_or(Duration::from_millis(100));
        let mut is_slow = false;

        let mut interval_sleep = std::pin::pin!(crate::time::pausable_sleep(slow_timeout.period));

        let mut timeout_hit = 0;

        let child_fds = ChildFds::new_split(child.stdout.take(), child.stderr.take());
        let mut child_acc = ChildAccumulator::new(child_fds);

        let (res, leaked) = {
            let res = loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    res = child.wait() => {
                        // The setup script finished executing.
                        break res;
                    }
                    _ = &mut interval_sleep, if status.is_none() => {
                        is_slow = true;
                        timeout_hit += 1;
                        let will_terminate = if let Some(terminate_after) = slow_timeout.terminate_after {
                            NonZeroUsize::new(timeout_hit as usize)
                                .expect("timeout_hit was just incremented")
                                >= terminate_after
                        } else {
                            false
                        };

                        if !slow_timeout.grace_period.is_zero() {
                            let _ = resp_tx.send(script.slow_event(
                                // Pass in the slow timeout period times timeout_hit, since
                                // stopwatch.elapsed() tends to be slightly longer.
                                timeout_hit * slow_timeout.period,
                                will_terminate.then_some(slow_timeout.grace_period)
                            ));
                        }

                        if will_terminate {
                            // attempt to terminate the slow test.
                            // as there is a race between shutting down a slow test and its own completion
                            // we silently ignore errors to avoid printing false warnings.
                            imp::terminate_child(
                                &mut child,
                                &mut child_acc,
                                TerminateMode::Timeout,
                                stopwatch,
                                req_rx,
                                job.as_ref(),
                                slow_timeout.grace_period,
                            ).await;
                            status = Some(ExecutionResult::Timeout);
                            if slow_timeout.grace_period.is_zero() {
                                break child.wait().await;
                            }
                            // Don't break here to give the wait task a chance to finish.
                        } else {
                            interval_sleep.as_mut().reset_last_duration();
                        }
                    }
                    recv = req_rx.recv() => {
                        // The sender stays open longer than the whole loop so a
                        // RecvError should never happen.
                        let req = recv.expect("req_rx sender is open");

                        match req {
                            RunUnitRequest::Signal(req) => {
                                handle_signal_request(
                                    &mut child,
                                    &mut child_acc,
                                    req,
                                    stopwatch,
                                    interval_sleep.as_mut(),
                                    req_rx,
                                    job.as_ref(),
                                    slow_timeout.grace_period
                                ).await;
                            }
                        }
                    }
                }
            };

            // Once the process is done executing, wait up to leak_timeout for the pipes to shut down.
            // Previously, this used to hang if spawned grandchildren inherited stdout/stderr but
            // didn't shut down properly. Now, this detects those cases and marks them as leaked.
            let leaked = loop {
                // Ignore stop and continue events here since the leak timeout should be very small.
                // TODO: we may want to consider them.
                let sleep = tokio::time::sleep(leak_timeout);

                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    () = sleep, if !child_acc.fds.is_done() => {
                        break true;
                    }
                    else => {
                        break false;
                    }
                }
            };

            (res, leaked)
        };

        let exit_status = match res {
            Ok(exit_status) => Some(exit_status),
            Err(err) => {
                child_acc.errors.push(ChildFdError::Wait(Arc::new(err)));
                None
            }
        };

        let exit_status = exit_status.expect("None always results in early return");

        let status = status
            .unwrap_or_else(|| create_execution_result(exit_status, &child_acc.errors, leaked));

        // Read from the environment map. If there's an error here, add it to the list of child errors.
        let mut errors: Vec<_> = child_acc.errors.into_iter().map(ChildError::from).collect();
        let env_map = if status.is_success() {
            match SetupScriptEnvMap::new(&env_path).await {
                Ok(env_map) => Some(env_map),
                Err(error) => {
                    errors.push(ChildError::SetupScriptOutput(error));
                    None
                }
            }
        } else {
            None
        };

        Ok(InternalSetupScriptExecuteStatus {
            output: ChildExecutionResult::Output {
                output: child_acc.output.freeze(),
                errors: ErrorList::new(
                    ChildExecutionResult::WAITING_ON_SETUP_SCRIPT_MESSAGE,
                    errors,
                ),
            },
            result: status,
            stopwatch_end: stopwatch.snapshot(),
            is_slow,
            env_map,
        })
    }

    /// Run an individual test in its own process.
    async fn run_test(
        &self,
        test: TestPacket<'a, '_>,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest>,
    ) -> InternalExecuteStatus {
        let mut stopwatch = crate::time::stopwatch();
        let delay_before_start = test.delay_before_start;

        match self
            .run_test_inner(test, &mut stopwatch, resp_tx, req_rx)
            .await
        {
            Ok(run_status) => run_status,
            Err(error) => InternalExecuteStatus {
                output: ChildExecutionResult::StartError(error),
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.snapshot(),
                is_slow: false,
                delay_before_start,
            },
        }
    }

    async fn run_test_inner(
        &self,
        test: TestPacket<'a, '_>,
        stopwatch: &mut StopwatchStart,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest>,
    ) -> Result<InternalExecuteStatus, ChildStartError> {
        let ctx = TestExecuteContext {
            double_spawn: &self.double_spawn,
            target_runner: &self.target_runner,
        };
        let mut cmd = test.test_instance.make_command(&ctx, self.test_list);
        let command_mut = cmd.command_mut();

        // Debug environment variable for testing.
        command_mut.env("__NEXTEST_ATTEMPT", format!("{}", test.retry_data.attempt));
        command_mut.env("NEXTEST_RUN_ID", format!("{}", self.run_id));
        command_mut.stdin(Stdio::null());
        test.setup_script_data.apply(
            &test.test_instance.to_test_query(),
            &self.profile.filterset_ecx(),
            command_mut,
        );
        imp::set_process_group(command_mut);

        // If creating a job fails, we might be on an old system. Ignore this -- job objects are a
        // best-effort thing.
        let job = imp::Job::create().ok();

        let crate::test_command::Child {
            mut child,
            child_fds,
        } = cmd
            .spawn(self.capture_strategy)
            .map_err(|error| ChildStartError::Spawn(Arc::new(error)))?;

        // If assigning the child to the job fails, ignore this. This can happen if the process has
        // exited.
        let _ = imp::assign_process_to_job(&child, job.as_ref());

        let mut child_acc = ChildAccumulator::new(child_fds);

        let mut status: Option<ExecutionResult> = None;
        let slow_timeout = test.settings.slow_timeout();
        let leak_timeout = test.settings.leak_timeout();
        let mut is_slow = false;

        // Use a pausable_sleep rather than an interval here because it's much harder to pause and
        // resume an interval.
        let mut interval_sleep = std::pin::pin!(crate::time::pausable_sleep(slow_timeout.period));

        let mut timeout_hit = 0;

        let (res, leaked) = {
            let res = loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    res = child.wait() => {
                        // The test finished executing.
                        break res;
                    }
                    _ = &mut interval_sleep, if status.is_none() => {
                        is_slow = true;
                        timeout_hit += 1;
                        let will_terminate = if let Some(terminate_after) = slow_timeout.terminate_after {
                            NonZeroUsize::new(timeout_hit as usize)
                                .expect("timeout_hit was just incremented")
                                >= terminate_after
                        } else {
                            false
                        };

                        if !slow_timeout.grace_period.is_zero() {
                            let _ = resp_tx.send(test.slow_event(
                                // Pass in the slow timeout period times timeout_hit, since
                                // stopwatch.elapsed() tends to be slightly longer.
                                timeout_hit * slow_timeout.period,
                                will_terminate.then_some(slow_timeout.grace_period),
                            ));
                        }

                        if will_terminate {
                            // Attempt to terminate the slow test. As there is a race between
                            // shutting down a slow test and its own completion, we silently ignore
                            // errors to avoid printing false warnings.
                            imp::terminate_child(
                                &mut child,
                                &mut child_acc,
                                TerminateMode::Timeout,
                                stopwatch,
                                req_rx,
                                job.as_ref(),
                                slow_timeout.grace_period,
                            ).await;
                            status = Some(ExecutionResult::Timeout);
                            if slow_timeout.grace_period.is_zero() {
                                break child.wait().await;
                            }
                            // Don't break here to give the wait task a chance to finish.
                        } else {
                            interval_sleep.as_mut().reset_last_duration();
                        }
                    }
                    recv = req_rx.recv() => {
                        // The sender stays open longer than the whole loop so a
                        // RecvError should never happen.
                        let req = recv.expect("req_rx sender is open");

                        match req {
                            RunUnitRequest::Signal(req) => {
                                handle_signal_request(
                                    &mut child,
                                    &mut child_acc,
                                    req,
                                    stopwatch,
                                    interval_sleep.as_mut(),
                                    req_rx,
                                    job.as_ref(),
                                    slow_timeout.grace_period
                                ).await;
                            }
                        }
                    }
                };
            };

            // Once the process is done executing, wait up to leak_timeout for the pipes to shut down.
            // Previously, this used to hang if spawned grandchildren inherited stdout/stderr but
            // didn't shut down properly. Now, this detects those cases and marks them as leaked.
            let leaked = loop {
                // Ignore stop and continue events here since the leak timeout should be very small.
                // TODO: we may want to consider them.
                let sleep = tokio::time::sleep(leak_timeout);

                tokio::select! {
                    // All of the branches here need to check for
                    // `!child_acc.fds.is_done()`, because if child_fds is done we
                    // want to hit the `else` block right away.
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    () = sleep, if !child_acc.fds.is_done() => {
                        break true;
                    }
                    recv = req_rx.recv(), if !child_acc.fds.is_done() => {
                        // The sender stays open longer than the whole loop, and the buffer is big
                        // enough for all messages ever sent through this channel, so a RecvError
                        // should never happen.
                        let req = recv.expect("a RecvError should never happen here");

                        match req {
                            RunUnitRequest::Signal(_) => {
                                // The process is done executing, so signals are moot.
                            }
                        }
                    }
                    else => {
                        break false;
                    }
                }
            };

            (res, leaked)
        };

        let exit_status = match res {
            Ok(exit_status) => Some(exit_status),
            Err(err) => {
                child_acc.errors.push(ChildFdError::Wait(Arc::new(err)));
                None
            }
        };

        let exit_status = exit_status.expect("None always results in early return");
        let result = status
            .unwrap_or_else(|| create_execution_result(exit_status, &child_acc.errors, leaked));

        Ok(InternalExecuteStatus {
            output: ChildExecutionResult::Output {
                output: child_acc.output.freeze(),
                errors: ErrorList::new(
                    ChildExecutionResult::WAITING_ON_TEST_MESSAGE,
                    child_acc.errors,
                ),
            },
            result,
            stopwatch_end: stopwatch.snapshot(),
            is_slow,
            delay_before_start: test.delay_before_start,
        })
    }
}

/// Drains the request receiver of any messages.
fn drain_req_rx(mut receiver: UnboundedReceiver<RunUnitRequest>) {
    loop {
        let message = receiver.try_recv();
        match message {
            Ok(message) => {
                message.drain();
            }
            Err(_) => {
                break;
            }
        }
    }
}

// It would be nice to fix this function to not have so many arguments, but this
// code is actively being refactored right now and imposing too much structure
// can cause more harm than good.
#[expect(clippy::too_many_arguments)]
async fn handle_signal_request(
    child: &mut Child,
    child_acc: &mut ChildAccumulator,
    req: SignalRequest,
    stopwatch: &mut StopwatchStart,
    // These annotations are needed to silence lints on non-Unix platforms.
    //
    // It would be nice to use an expect lint here, but Rust 1.81 appears to
    // have a bug where it complains about expectations not being fulfilled on
    // Windows, even though they are in reality. The bug is fixed in Rust 1.83,
    // so we should switch to expect after the MSRV is bumped to 1.83+.
    #[cfg_attr(not(unix), allow(unused_mut, unused_variables))] mut interval_sleep: Pin<
        &mut PausableSleep,
    >,
    req_rx: &mut UnboundedReceiver<RunUnitRequest>,
    job: Option<&imp::Job>,
    grace_period: Duration,
) {
    match req {
        #[cfg(unix)]
        SignalRequest::Stop(sender) => {
            // It isn't possible to receive a stop event twice since it gets
            // debounced in the main signal handler.
            stopwatch.pause();
            interval_sleep.as_mut().pause();
            imp::job_control_child(child, JobControlEvent::Stop);
            // The receiver being dead probably means the main thread panicked
            // or similar.
            let _ = sender.send(());
        }
        #[cfg(unix)]
        SignalRequest::Continue => {
            // It's possible to receive a resume event right at the beginning of
            // test execution, so debounce it.
            if stopwatch.is_paused() {
                stopwatch.resume();
                interval_sleep.as_mut().resume();
                imp::job_control_child(child, JobControlEvent::Continue);
            }
        }
        SignalRequest::Shutdown(event) => {
            imp::terminate_child(
                child,
                child_acc,
                TerminateMode::Signal(event),
                stopwatch,
                req_rx,
                job,
                grace_period,
            )
            .await;
        }
    }
}

fn create_execution_result(
    exit_status: ExitStatus,
    child_errors: &[ChildFdError],
    leaked: bool,
) -> ExecutionResult {
    if !child_errors.is_empty() {
        // If an error occurred while waiting on the child handles, treat it as
        // an execution failure.
        ExecutionResult::ExecFail
    } else if exit_status.success() {
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
                    (code < 0).then_some(AbortStatus::WindowsNtStatus(code))
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
}

/// Data related to retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct RetryData {
    /// The current attempt. In the range `[1, total_attempts]`.
    pub attempt: usize,

    /// The total number of times this test can be run. Equal to `1 + retries`.
    pub total_attempts: usize,
}

impl RetryData {
    /// Returns true if there are no more attempts after this.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.total_attempts
    }
}

/// Information about executions of a test, including retries.
#[derive(Clone, Debug)]
pub struct ExecutionStatuses {
    /// This is guaranteed to be non-empty.
    statuses: Vec<ExecuteStatus>,
}

#[expect(clippy::len_without_is_empty)] // RunStatuses is never empty
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
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &'_ ExecuteStatus> + '_ {
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
    /// Retry-related data.
    pub retry_data: RetryData,
    /// The stdout and stderr output for this test.
    pub output: ChildExecutionResult,
    /// The execution result for this test: pass, fail or execution error.
    pub result: ExecutionResult,
    /// The time at which the test started.
    pub start_time: DateTime<FixedOffset>,
    /// The time it took for the test to run.
    pub time_taken: Duration,
    /// Whether this test counts as slow.
    pub is_slow: bool,
    /// The delay will be non-zero if this is a retry and delay was specified.
    pub delay_before_start: Duration,
}

struct InternalExecuteStatus {
    output: ChildExecutionResult,
    result: ExecutionResult,
    stopwatch_end: StopwatchSnapshot,
    is_slow: bool,
    delay_before_start: Duration,
}

impl InternalExecuteStatus {
    fn into_external(self, retry_data: RetryData) -> ExecuteStatus {
        ExecuteStatus {
            retry_data,
            output: self.output,
            result: self.result,
            start_time: self.stopwatch_end.start_time.fixed_offset(),
            time_taken: self.stopwatch_end.active,
            is_slow: self.is_slow,
            delay_before_start: self.delay_before_start,
        }
    }
}

/// Information about the execution of a setup script.
#[derive(Clone, Debug)]
pub struct SetupScriptExecuteStatus {
    /// Output for this setup script.
    pub output: ChildExecutionResult,
    /// The execution result for this setup script: pass, fail or execution error.
    pub result: ExecutionResult,
    /// The time at which the script started.
    pub start_time: DateTime<FixedOffset>,
    /// The time it took for the script to run.
    pub time_taken: Duration,
    /// Whether this script counts as slow.
    pub is_slow: bool,
    /// The number of environment variables that were set by this script.
    ///
    /// `None` if an error occurred while running the script or reading the
    /// environment map.
    pub env_count: Option<usize>,
}

struct InternalSetupScriptExecuteStatus {
    output: ChildExecutionResult,
    result: ExecutionResult,
    stopwatch_end: StopwatchSnapshot,
    is_slow: bool,
    env_map: Option<SetupScriptEnvMap>,
}

impl InternalSetupScriptExecuteStatus {
    fn into_external(self) -> (SetupScriptExecuteStatus, Option<SetupScriptEnvMap>) {
        let env_count = self.env_map.as_ref().map(|map| map.len());
        (
            SetupScriptExecuteStatus {
                output: self.output,
                result: self.result,
                start_time: self.stopwatch_end.start_time.fixed_offset(),
                time_taken: self.stopwatch_end.active,
                is_slow: self.is_slow,
                env_count,
            },
            self.env_map,
        )
    }
}

/// Statistics for a test run.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub struct RunStats {
    /// The total number of tests that were expected to be run at the beginning.
    ///
    /// If the test run is cancelled, this will be more than `finished_count` at the end.
    pub initial_run_count: usize,

    /// The total number of tests that finished running.
    pub finished_count: usize,

    /// The total number of setup scripts that were expected to be run at the beginning.
    ///
    /// If the test run is cancelled, this will be more than `finished_count` at the end.
    pub setup_scripts_initial_count: usize,

    /// The total number of setup scripts that finished running.
    pub setup_scripts_finished_count: usize,

    /// The number of setup scripts that passed.
    pub setup_scripts_passed: usize,

    /// The number of setup scripts that failed.
    pub setup_scripts_failed: usize,

    /// The number of setup scripts that encountered an execution failure.
    pub setup_scripts_exec_failed: usize,

    /// The number of setup scripts that timed out.
    pub setup_scripts_timed_out: usize,

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
    /// Returns true if there are any failures recorded in the stats.
    pub fn has_failures(&self) -> bool {
        self.setup_scripts_failed > 0
            || self.setup_scripts_exec_failed > 0
            || self.setup_scripts_timed_out > 0
            || self.failed > 0
            || self.exec_failed > 0
            || self.timed_out > 0
    }

    /// Summarizes the stats as an enum at the end of a test run.
    pub fn summarize_final(&self) -> FinalRunStats {
        // Check for failures first. The order of setup scripts vs tests should not be important,
        // though we don't assert that here.
        if self.setup_scripts_failed > 0
            || self.setup_scripts_exec_failed > 0
            || self.setup_scripts_timed_out > 0
        {
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript)
        } else if self.setup_scripts_initial_count > self.setup_scripts_finished_count {
            FinalRunStats::Cancelled(RunStatsFailureKind::SetupScript)
        } else if self.failed > 0 || self.exec_failed > 0 || self.timed_out > 0 {
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: self.initial_run_count,
                not_run: self.initial_run_count.saturating_sub(self.finished_count),
            })
        } else if self.initial_run_count > self.finished_count {
            FinalRunStats::Cancelled(RunStatsFailureKind::Test {
                initial_run_count: self.initial_run_count,
                not_run: self.initial_run_count.saturating_sub(self.finished_count),
            })
        } else if self.finished_count == 0 {
            FinalRunStats::NoTestsRun
        } else {
            FinalRunStats::Success
        }
    }

    fn on_setup_script_finished(&mut self, status: &SetupScriptExecuteStatus) {
        self.setup_scripts_finished_count += 1;

        match status.result {
            ExecutionResult::Pass | ExecutionResult::Leak => {
                self.setup_scripts_passed += 1;
            }
            ExecutionResult::Fail { .. } => {
                self.setup_scripts_failed += 1;
            }
            ExecutionResult::ExecFail => {
                self.setup_scripts_exec_failed += 1;
            }
            ExecutionResult::Timeout => {
                self.setup_scripts_timed_out += 1;
            }
        }
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

/// A type summarizing the possible outcomes of a test run.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FinalRunStats {
    /// The test run was successful, or is successful so far.
    Success,

    /// The test run was successful, or is successful so far, but no tests were selected to run.
    NoTestsRun,

    /// The test run was cancelled.
    Cancelled(RunStatsFailureKind),

    /// At least one test failed.
    Failed(RunStatsFailureKind),
}

/// A type summarizing the step at which a test run failed.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RunStatsFailureKind {
    /// The run was interrupted during setup script execution.
    SetupScript,

    /// The run was interrupted during test execution.
    Test {
        /// The total number of tests scheduled.
        initial_run_count: usize,

        /// The number of tests not run, or for a currently-executing test the number queued up to
        /// run.
        not_run: usize,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SignalCount {
    Once,
    Twice,
}

impl SignalCount {
    fn to_request(self, event: ShutdownEvent) -> ShutdownRequest {
        match self {
            Self::Once => ShutdownRequest::Once(event),
            Self::Twice => ShutdownRequest::Twice,
        }
    }
}

/// Events sent from the test runner to individual test (or setup script) execution tasks.
#[derive(Clone, Debug)]
enum RunUnitRequest {
    Signal(SignalRequest),
}

impl RunUnitRequest {
    fn drain(self) {
        match self {
            #[cfg(unix)]
            Self::Signal(SignalRequest::Stop(sender)) => {
                // The receiver being dead isn't really important.
                let _ = sender.send(());
            }
            #[cfg(unix)]
            Self::Signal(SignalRequest::Continue) => {}
            Self::Signal(SignalRequest::Shutdown(_)) => {}
        }
    }
}

#[derive(Clone, Debug)]
enum SignalRequest {
    // The mpsc sender is used by each test to indicate that the stop signal has been sent.
    #[cfg(unix)]
    Stop(UnboundedSender<()>),
    #[cfg(unix)]
    Continue,
    Shutdown(ShutdownRequest),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ShutdownRequest {
    Once(ShutdownEvent),
    Twice,
}

#[derive(Clone, Copy, Debug)]
struct TestPacket<'a, 'test> {
    test_instance: TestInstance<'a>,
    retry_data: RetryData,
    settings: &'test TestSettings,
    setup_script_data: &'test SetupScriptExecuteData<'a>,
    delay_before_start: Duration,
}

impl<'a> TestPacket<'a, '_> {
    fn slow_event(
        &self,
        elapsed: Duration,
        will_terminate: Option<Duration>,
    ) -> InternalTestEvent<'a> {
        InternalTestEvent::Slow {
            test_instance: self.test_instance,
            retry_data: self.retry_data,
            elapsed,
            will_terminate,
        }
    }
}

#[derive(Clone, Debug)]
struct SetupScriptPacket<'a> {
    script_id: ScriptId,
    config: &'a ScriptConfig,
}

impl<'a> SetupScriptPacket<'a> {
    /// Turns self into a command that can be executed.
    fn make_command(
        &self,
        double_spawn: &DoubleSpawnInfo,
        test_list: &TestList<'_>,
    ) -> Result<SetupScriptCommand, ChildStartError> {
        SetupScriptCommand::new(self.config, double_spawn, test_list)
    }

    fn slow_event(
        &self,
        elapsed: Duration,
        will_terminate: Option<Duration>,
    ) -> InternalTestEvent<'a> {
        InternalTestEvent::SetupScriptSlow {
            script_id: self.script_id.clone(),
            config: self.config,
            elapsed,
            will_terminate,
        }
    }
}

/// The return result of `handle_event`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandleEventResponse {
    #[cfg_attr(not(unix), expect(dead_code))]
    JobControl(JobControlEvent),
}

struct CallbackContext<'a, F> {
    callback: F,
    run_id: ReportUuid,
    profile_name: String,
    cli_args: Vec<String>,
    stopwatch: StopwatchStart,
    run_stats: RunStats,
    fail_fast: bool,
    running_setup_script: Option<ContextSetupScript<'a>>,
    running_tests: BTreeMap<TestInstanceId<'a>, ContextTestInstance<'a>>,
    cancel_state: Option<CancelReason>,
    signal_count: Option<SignalCount>,
}

impl<'a, F> CallbackContext<'a, F>
where
    F: FnMut(TestEvent<'a>) + Send,
{
    fn new(
        callback: F,
        run_id: ReportUuid,
        profile_name: &str,
        cli_args: Vec<String>,
        initial_run_count: usize,
        fail_fast: bool,
    ) -> Self {
        Self {
            callback,
            run_id,
            stopwatch: crate::time::stopwatch(),
            profile_name: profile_name.to_owned(),
            cli_args,
            run_stats: RunStats {
                initial_run_count,
                ..RunStats::default()
            },
            fail_fast,
            running_setup_script: None,
            running_tests: BTreeMap::new(),
            cancel_state: None,
            signal_count: None,
        }
    }

    fn run_started(&mut self, test_list: &'a TestList) {
        self.basic_callback(TestEventKind::RunStarted {
            test_list,
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            cli_args: self.cli_args.clone(),
        })
    }

    #[inline]
    fn basic_callback(&mut self, kind: TestEventKind<'a>) {
        let snapshot = self.stopwatch.snapshot();
        let event = TestEvent {
            // We'd previously add up snapshot.start_time + snapshot.active +
            // paused, but that isn't resilient to clock changes. Instead, use
            // `Local::now()` time (which isn't necessarily monotonic) along
            // with snapshot.active (which is almost always monotonic).
            timestamp: Local::now().fixed_offset(),
            elapsed: snapshot.active,
            kind,
        };
        (self.callback)(event)
    }

    #[inline]
    fn callback(
        &mut self,
        kind: TestEventKind<'a>,
    ) -> Result<Option<HandleEventResponse>, InternalCancel> {
        self.basic_callback(kind);
        Ok(None)
    }

    fn handle_event(
        &mut self,
        event: InternalEvent<'a>,
    ) -> Result<Option<HandleEventResponse>, InternalCancel> {
        match event {
            InternalEvent::Test(InternalTestEvent::SetupScriptStarted {
                script_id,
                config,
                index,
                total,
                req_rx_tx,
            }) => {
                let (req_tx, req_rx) = unbounded_channel();
                match req_rx_tx.send(req_rx) {
                    Ok(_) => {}
                    Err(_) => {
                        // The test task died?
                        debug!(?script_id, "test task died, ignoring");
                        return Ok(None);
                    }
                }
                self.new_setup_script(script_id.clone(), config, index, total, req_tx);
                self.callback(TestEventKind::SetupScriptStarted {
                    index,
                    total,
                    script_id,
                    command: config.program(),
                    args: config.args(),
                    no_capture: config.no_capture(),
                })
            }
            InternalEvent::Test(InternalTestEvent::SetupScriptSlow {
                script_id,
                config,
                elapsed,
                will_terminate,
            }) => self.callback(TestEventKind::SetupScriptSlow {
                script_id,
                command: config.program(),
                args: config.args(),
                elapsed,
                will_terminate: will_terminate.is_some(),
            }),
            InternalEvent::Test(InternalTestEvent::SetupScriptFinished {
                script_id,
                config,
                index,
                total,
                status,
            }) => {
                self.finish_setup_script();
                self.run_stats.on_setup_script_finished(&status);
                // Setup scripts failing always cause the entire test run to be cancelled
                // (--no-fail-fast is ignored).
                let fail_cancel = !status.result.is_success();

                self.callback(TestEventKind::SetupScriptFinished {
                    index,
                    total,
                    script_id,
                    command: config.program(),
                    args: config.args(),
                    no_capture: config.no_capture(),
                    run_status: status,
                })?;

                if fail_cancel {
                    self.begin_cancel(CancelReason::SetupScriptFailure);
                    Err(InternalCancel::TestFailure)
                } else {
                    Ok(None)
                }
            }
            InternalEvent::Test(InternalTestEvent::Started {
                test_instance,
                req_rx_tx,
            }) => {
                let (req_tx, req_rx) = unbounded_channel();
                match req_rx_tx.send(req_rx) {
                    Ok(_) => {}
                    Err(_) => {
                        // The test task died?
                        debug!(test = ?test_instance.id(), "test task died, ignoring");
                        return Ok(None);
                    }
                }
                self.new_test(test_instance, req_tx);
                self.callback(TestEventKind::TestStarted {
                    test_instance,
                    current_stats: self.run_stats,
                    running: self.running_tests.len(),
                    cancel_state: self.cancel_state,
                })
            }
            InternalEvent::Test(InternalTestEvent::Slow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            }) => self.callback(TestEventKind::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate: will_terminate.is_some(),
            }),
            InternalEvent::Test(InternalTestEvent::AttemptFailedWillRetry {
                test_instance,
                failure_output,
                run_status,
                delay_before_next_attempt,
            }) => {
                let instance = self.existing_test(test_instance.id());
                instance.attempt_failed_will_retry(run_status.clone());
                self.callback(TestEventKind::TestAttemptFailedWillRetry {
                    test_instance,
                    failure_output,
                    run_status,
                    delay_before_next_attempt,
                })
            }
            InternalEvent::Test(InternalTestEvent::RetryStarted {
                test_instance,
                retry_data,
            }) => self.callback(TestEventKind::TestRetryStarted {
                test_instance,
                retry_data,
            }),
            InternalEvent::Test(InternalTestEvent::Finished {
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                last_run_status,
            }) => {
                let run_statuses = self.finish_test(test_instance.id(), last_run_status);
                self.run_stats.on_test_finished(&run_statuses);

                // should this run be cancelled because of a failure?
                let fail_cancel = self.fail_fast && !run_statuses.last_status().result.is_success();

                self.callback(TestEventKind::TestFinished {
                    test_instance,
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats: self.run_stats,
                    running: self.running(),
                    cancel_state: self.cancel_state,
                })?;

                if fail_cancel {
                    // A test failed: start cancellation.
                    self.begin_cancel(CancelReason::TestFailure);
                    Err(InternalCancel::TestFailure)
                } else {
                    Ok(None)
                }
            }
            InternalEvent::Test(InternalTestEvent::Skipped {
                test_instance,
                reason,
            }) => {
                self.run_stats.skipped += 1;
                self.callback(TestEventKind::TestSkipped {
                    test_instance,
                    reason,
                })
            }
            InternalEvent::Signal(event) => self.handle_signal_event(event),
            InternalEvent::ReportCancel => {
                self.begin_cancel(CancelReason::ReportError);
                Err(InternalCancel::Report)
            }
        }
    }

    fn new_setup_script(
        &mut self,
        id: ScriptId,
        config: &'a ScriptConfig,
        index: usize,
        total: usize,
        req_tx: UnboundedSender<RunUnitRequest>,
    ) {
        let prev = self.running_setup_script.replace(ContextSetupScript {
            id,
            config,
            index,
            total,
            req_tx,
        });
        debug_assert!(
            prev.is_none(),
            "new setup script expected, but already exists: {prev:?}",
        );
    }

    fn finish_setup_script(&mut self) {
        let prev = self.running_setup_script.take();
        debug_assert!(
            prev.is_some(),
            "existing setup script expected, but already exists: {prev:?}",
        );
    }

    fn new_test(&mut self, instance: TestInstance<'a>, req_tx: UnboundedSender<RunUnitRequest>) {
        let prev = self.running_tests.insert(
            instance.id(),
            ContextTestInstance {
                instance,
                past_attempts: Vec::new(),
                req_tx,
            },
        );
        if let Some(prev) = prev {
            panic!("new test instance expected, but already exists: {prev:?}");
        }
    }

    fn existing_test(&mut self, key: TestInstanceId<'a>) -> &mut ContextTestInstance<'a> {
        self.running_tests
            .get_mut(&key)
            .expect("existing test instance expected but not found")
    }

    fn finish_test(
        &mut self,
        key: TestInstanceId<'a>,
        last_run_status: ExecuteStatus,
    ) -> ExecutionStatuses {
        self.running_tests
            .remove(&key)
            .unwrap_or_else(|| {
                panic!(
                    "existing test instance {key:?} expected, \
                     but not found"
                )
            })
            .finish(last_run_status)
    }

    fn setup_scripts_running(&self) -> usize {
        if self.running_setup_script.is_some() {
            1
        } else {
            0
        }
    }

    fn running(&self) -> usize {
        self.running_tests.len()
    }

    fn broadcast_request(&self, req: RunUnitRequest) {
        if let Some(setup_script) = &self.running_setup_script {
            if let Err(error) = setup_script.req_tx.send(req.clone()) {
                // The most likely reason for this error is that the setup script
                // has exited but we haven't processed the exit event yet.
                debug!(?setup_script.id, ?error, "failed to send request to setup script");
            }
        }

        for (key, instance) in &self.running_tests {
            if let Err(error) = instance.req_tx.send(req.clone()) {
                // The most likely reason for this error is that the test
                // instance has exited but we haven't processed the exit event
                // yet.
                debug!(?key, ?error, "failed to send request to test instance");
            }
        }
    }

    fn handle_signal_event(
        &mut self,
        event: SignalEvent,
    ) -> Result<Option<HandleEventResponse>, InternalCancel> {
        match event {
            SignalEvent::Shutdown(event) => {
                let signal_count = self.increment_signal_count();
                let req = signal_count.to_request(event);

                let cancel_reason = match event {
                    #[cfg(unix)]
                    ShutdownEvent::Hangup | ShutdownEvent::Term | ShutdownEvent::Quit => {
                        CancelReason::Signal
                    }
                    ShutdownEvent::Interrupt => CancelReason::Interrupt,
                };

                self.begin_cancel(cancel_reason);
                Err(InternalCancel::Signal(req))
            }
            #[cfg(unix)]
            SignalEvent::JobControl(JobControlEvent::Stop) => {
                // Debounce stop signals.
                if !self.stopwatch.is_paused() {
                    self.callback(TestEventKind::RunPaused {
                        setup_scripts_running: self.setup_scripts_running(),
                        running: self.running(),
                    })?;
                    self.stopwatch.pause();
                    Ok(Some(HandleEventResponse::JobControl(JobControlEvent::Stop)))
                } else {
                    Ok(None)
                }
            }
            #[cfg(unix)]
            SignalEvent::JobControl(JobControlEvent::Continue) => {
                // Debounce continue signals.
                if self.stopwatch.is_paused() {
                    self.stopwatch.resume();
                    self.callback(TestEventKind::RunContinued {
                        setup_scripts_running: self.setup_scripts_running(),
                        running: self.running(),
                    })?;
                    Ok(Some(HandleEventResponse::JobControl(
                        JobControlEvent::Continue,
                    )))
                } else {
                    Ok(None)
                }
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
    fn begin_cancel(&mut self, reason: CancelReason) {
        if self.cancel_state < Some(reason) {
            self.cancel_state = Some(reason);
            self.basic_callback(TestEventKind::RunBeginCancel {
                setup_scripts_running: self.setup_scripts_running(),
                running: self.running(),
                reason,
            });
        }
    }

    fn run_finished(&mut self) {
        let stopwatch_end = self.stopwatch.snapshot();
        self.basic_callback(TestEventKind::RunFinished {
            start_time: stopwatch_end.start_time.fixed_offset(),
            run_id: self.run_id,
            elapsed: stopwatch_end.active,
            run_stats: self.run_stats,
        })
    }
}

#[derive(Debug)]
struct ContextSetupScript<'a> {
    id: ScriptId,
    // Store these details primarily for debugging.
    #[expect(dead_code)]
    config: &'a ScriptConfig,
    #[expect(dead_code)]
    index: usize,
    #[expect(dead_code)]
    total: usize,
    req_tx: UnboundedSender<RunUnitRequest>,
}

#[derive(Debug)]
struct ContextTestInstance<'a> {
    // Store the instance primarily for debugging.
    #[expect(dead_code)]
    instance: TestInstance<'a>,
    past_attempts: Vec<ExecuteStatus>,
    req_tx: UnboundedSender<RunUnitRequest>,
}

impl ContextTestInstance<'_> {
    fn attempt_failed_will_retry(&mut self, run_status: ExecuteStatus) {
        self.past_attempts.push(run_status);
    }

    fn finish(self, last_run_status: ExecuteStatus) -> ExecutionStatuses {
        let mut attempts = self.past_attempts;
        attempts.push(last_run_status);
        ExecutionStatuses::new(attempts)
    }
}

#[derive(Debug)]
enum InternalEvent<'a> {
    Test(InternalTestEvent<'a>),
    Signal(SignalEvent),
    ReportCancel,
}

#[derive(Debug)]
enum InternalTestEvent<'a> {
    SetupScriptStarted {
        script_id: ScriptId,
        config: &'a ScriptConfig,
        index: usize,
        total: usize,
        // See the note in the `Started` variant.
        req_rx_tx: oneshot::Sender<UnboundedReceiver<RunUnitRequest>>,
    },
    SetupScriptSlow {
        script_id: ScriptId,
        config: &'a ScriptConfig,
        elapsed: Duration,
        will_terminate: Option<Duration>,
    },
    SetupScriptFinished {
        script_id: ScriptId,
        config: &'a ScriptConfig,
        index: usize,
        total: usize,
        status: SetupScriptExecuteStatus,
    },
    Started {
        test_instance: TestInstance<'a>,
        // The channel over which to return the unit request.
        //
        // The callback context is solely responsible for coordinating the
        // creation of all channels, such that it acts as the source of truth
        // for which units to broadcast messages out to. This oneshot channel is
        // used to let each test instance know to go ahead and start running
        // tests.
        //
        // Why do we use unbounded channels? Mostly to make life simpler --
        // these are low-traffic channels that we don't expect to be backed up.
        req_rx_tx: oneshot::Sender<UnboundedReceiver<RunUnitRequest>>,
    },
    Slow {
        test_instance: TestInstance<'a>,
        retry_data: RetryData,
        elapsed: Duration,
        will_terminate: Option<Duration>,
    },
    AttemptFailedWillRetry {
        test_instance: TestInstance<'a>,
        failure_output: TestOutputDisplay,
        run_status: ExecuteStatus,
        delay_before_next_attempt: Duration,
    },
    RetryStarted {
        test_instance: TestInstance<'a>,
        retry_data: RetryData,
    },
    Finished {
        test_instance: TestInstance<'a>,
        success_output: TestOutputDisplay,
        failure_output: TestOutputDisplay,
        junit_store_success_output: bool,
        junit_store_failure_output: bool,
        last_run_status: ExecuteStatus,
    },
    Skipped {
        test_instance: TestInstance<'a>,
        reason: MismatchReason,
    },
}

#[derive(Debug)]
enum InternalCancel {
    Report,
    TestFailure,
    Signal(ShutdownRequest),
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
    WindowsNtStatus(windows_sys::Win32::Foundation::NTSTATUS),
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
    pub(super) use win32job::Job;
    use win32job::JobError;
    use windows_sys::Win32::{
        Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE},
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
            JobObjects::TerminateJobObject,
        },
    };

    pub(super) fn configure_handle_inheritance_impl(
        no_capture: bool,
    ) -> Result<(), ConfigureHandleInheritanceError> {
        unsafe fn set_handle_inherit(handle: u32, inherit: bool) -> std::io::Result<()> {
            let handle = GetStdHandle(handle);
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let flags = if inherit { HANDLE_FLAG_INHERIT } else { 0 };
            if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flags) == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        unsafe {
            // Never inherit stdin.
            set_handle_inherit(STD_INPUT_HANDLE, false)?;

            // Inherit stdout and stderr if and only if no_capture is true.
            set_handle_inherit(STD_OUTPUT_HANDLE, no_capture)?;
            set_handle_inherit(STD_ERROR_HANDLE, no_capture)?;
        }

        Ok(())
    }

    pub(super) fn set_process_group(_cmd: &mut std::process::Command) {
        // TODO: set process group on Windows for better ctrl-C handling.
    }

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

            job.assign_process(handle as isize)?;
        }

        Ok(())
    }

    pub(super) async fn terminate_child(
        child: &mut Child,
        _child_acc: &mut ChildAccumulator,
        mode: TerminateMode,
        _stopwatch: &mut StopwatchStart,
        _req_rx: &mut UnboundedReceiver<RunUnitRequest>,
        job: Option<&Job>,
        _grace_period: Duration,
    ) {
        // Ignore signal events since Windows propagates them to child processes (this may change if
        // we start assigning processes to groups on Windows).
        if !matches!(mode, TerminateMode::Timeout) {
            return;
        }
        if let Some(job) = job {
            let handle = job.handle();
            unsafe {
                // Ignore the error here -- it's likely due to the process exiting.
                // Note: 1 is the exit code returned by Windows.
                _ = TerminateJobObject(handle as _, 1);
            }
        }
        // Start killing the process directly for good measure.
        let _ = child.start_kill();
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use libc::{SIGCONT, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGSTOP, SIGTERM, SIGTSTP};
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
    pub(super) fn set_process_group(cmd: &mut std::process::Command) {
        cmd.process_group(0);
    }

    #[derive(Debug)]
    pub(super) struct Job(());

    impl Job {
        pub(super) fn create() -> Result<Self, Infallible> {
            Ok(Self(()))
        }
    }

    pub(super) fn assign_process_to_job(
        _child: &tokio::process::Child,
        _job: Option<&Job>,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    pub(super) fn job_control_child(child: &Child, event: JobControlEvent) {
        if let Some(pid) = child.id() {
            let pid = pid as i32;
            // Send the signal to the process group.
            let signal = match event {
                JobControlEvent::Stop => SIGTSTP,
                JobControlEvent::Continue => SIGCONT,
            };
            unsafe {
                // We set up a process group while starting the test -- now send a signal to that
                // group.
                libc::kill(-pid, signal);
            }
        } else {
            // The child exited already -- don't send a signal.
        }
    }

    // Note this is SIGSTOP rather than SIGTSTP to avoid triggering our signal handler.
    pub(super) fn raise_stop() {
        // This can never error out because SIGSTOP is a valid signal.
        unsafe { libc::raise(SIGSTOP) };
    }

    // TODO: should this indicate whether the process exited immediately? Could
    // do this with a non-async fn that optionally returns a future to await on.
    pub(super) async fn terminate_child(
        child: &mut Child,
        child_acc: &mut ChildAccumulator,
        mode: TerminateMode,
        stopwatch: &mut StopwatchStart,
        req_rx: &mut UnboundedReceiver<RunUnitRequest>,
        _job: Option<&Job>,
        grace_period: Duration,
    ) {
        if let Some(pid) = child.id() {
            let pid = pid as i32;
            let term_signal = match mode {
                _ if grace_period.is_zero() => SIGKILL,
                TerminateMode::Timeout => SIGTERM,
                TerminateMode::Signal(ShutdownRequest::Once(ShutdownEvent::Hangup)) => SIGHUP,
                TerminateMode::Signal(ShutdownRequest::Once(ShutdownEvent::Term)) => SIGTERM,
                TerminateMode::Signal(ShutdownRequest::Once(ShutdownEvent::Quit)) => SIGQUIT,
                TerminateMode::Signal(ShutdownRequest::Once(ShutdownEvent::Interrupt)) => SIGINT,
                TerminateMode::Signal(ShutdownRequest::Twice) => SIGKILL,
            };
            unsafe {
                // We set up a process group while starting the test -- now send a signal to that
                // group.
                libc::kill(-pid, term_signal)
            };

            if term_signal == SIGKILL {
                // SIGKILL guarantees the process group is dead.
                return;
            }

            let mut sleep = std::pin::pin!(crate::time::pausable_sleep(grace_period));

            loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    _ = child.wait() => {
                        // The process exited.
                        break;
                    }
                    recv = req_rx.recv() => {
                        // The sender stays open longer than the whole loop, and the buffer is big
                        // enough for all messages ever sent through this channel, so a RecvError
                        // should never happen.
                        let req = recv.expect("a RecvError should never happen here");

                        match req {
                            RunUnitRequest::Signal(SignalRequest::Stop(sender)) => {
                                stopwatch.pause();
                                sleep.as_mut().pause();
                                imp::job_control_child(child, JobControlEvent::Stop);
                                let _ = sender.send(());
                            }
                            RunUnitRequest::Signal(SignalRequest::Continue) => {
                                // Possible to receive a Continue at the beginning of execution.
                                if !sleep.is_paused() {
                                    stopwatch.resume();
                                    sleep.as_mut().resume();
                                }
                                imp::job_control_child(child, JobControlEvent::Continue);
                            }
                            RunUnitRequest::Signal(SignalRequest::Shutdown(_)) => {
                                // Receiving a shutdown signal while in this state always means kill
                                // immediately.
                                unsafe {
                                    // Send SIGKILL to the entire process group.
                                    libc::kill(-pid, SIGKILL);
                                }
                                break;
                            }
                        }
                    }
                    _ = &mut sleep => {
                        // The process didn't exit -- need to do a hard shutdown.
                        unsafe {
                            // Send SIGKILL to the entire process group.
                            libc::kill(-pid, SIGKILL);
                        }
                        break;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminateMode {
    Timeout,
    Signal(ShutdownRequest),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::NextestConfig, platform::BuildPlatforms};

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut builder = TestRunnerBuilder::default();
        builder
            .set_capture_strategy(CaptureStrategy::None)
            .set_test_threads(TestThreads::Count(20));
        let test_list = TestList::empty();
        let config = NextestConfig::default_config("/fake/dir");
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
        let handler_kind = SignalHandlerKind::Noop;
        let profile = profile.apply_build_platforms(&build_platforms);
        let runner = builder
            .build(
                &test_list,
                &profile,
                vec![],
                handler_kind,
                DoubleSpawnInfo::disabled(),
                TargetRunner::empty(),
            )
            .unwrap();
        assert_eq!(runner.inner.capture_strategy, CaptureStrategy::None);
        assert_eq!(runner.inner.test_threads, 1, "tests run serially");
    }

    #[test]
    fn test_is_success() {
        assert_eq!(
            RunStats::default().summarize_final(),
            FinalRunStats::NoTestsRun,
            "empty run => no tests run"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Success,
            "initial run count = final run count => success"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 41,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Cancelled(RunStatsFailureKind::Test {
                initial_run_count: 42,
                not_run: 1
            }),
            "initial run count > final run count => cancelled"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 42,
                not_run: 0
            }),
            "failed => failure"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                exec_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 42,
                not_run: 0
            }),
            "exec failed => failure"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                timed_out: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 42,
                not_run: 0
            }),
            "timed out => failure"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                skipped: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Success,
            "skipped => not considered a failure"
        );

        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Cancelled(RunStatsFailureKind::SetupScript),
            "setup script failed => failure"
        );

        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript),
            "setup script failed => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_exec_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript),
            "setup script exec failed => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_timed_out: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript),
            "setup script timed out => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_passed: 2,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::NoTestsRun,
            "setup scripts passed => success, but no tests run"
        );
    }
}
