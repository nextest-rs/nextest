// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::DispatcherContext;
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
    input::{InputHandler, InputHandlerKind, InputHandlerStatus},
    list::{TestExecuteContext, TestInstance, TestList},
    reporter::events::{
        AbortStatus, ExecutionResult, InfoResponse, RunStats, SetupScriptInfoResponse, TestEvent,
        TestInfoResponse, UnitKind, UnitState,
    },
    runner::{
        InternalExecuteStatus, InternalSetupScriptExecuteStatus, InternalTestEvent,
        UnitExecuteStatus,
    },
    signal::{ShutdownEvent, SignalHandler, SignalHandlerKind},
    target_runner::TargetRunner,
    test_command::{ChildAccumulator, ChildFds},
    test_output::{CaptureStrategy, ChildExecutionOutput, ChildOutput, ChildSplitOutput},
    time::{PausableSleep, StopwatchStart},
};
use async_scoped::TokioScope;
use future_queue::StreamExt;
use futures::prelude::*;
use nextest_metadata::FilterMatch;
use quick_junit::ReportUuid;
use rand::{distributions::OpenClosed01, thread_rng, Rng};
use std::{
    convert::Infallible,
    fmt,
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
use tracing::{debug, instrument, warn};

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
    max_fail: Option<usize>,
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

    /// Sets the max-fail value for this test runner.
    pub fn set_max_fail(&mut self, max_fail: usize) -> &mut Self {
        self.max_fail = Some(max_fail);
        self
    }

    /// Sets the number of tests to run simultaneously.
    pub fn set_test_threads(&mut self, test_threads: TestThreads) -> &mut Self {
        self.test_threads = Some(test_threads);
        self
    }

    /// Creates a new test runner.
    #[expect(clippy::too_many_arguments)]
    pub fn build<'a>(
        self,
        test_list: &'a TestList,
        profile: &'a EvaluatableProfile<'a>,
        cli_args: Vec<String>,
        signal_handler: SignalHandlerKind,
        input_handler: InputHandlerKind,
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
        let max_fail = self.max_fail.or_else(|| profile.fail_fast().then_some(1));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("nextest-runner-worker")
            .build()
            .map_err(TestRunnerBuildError::TokioRuntimeCreate)?;
        let _guard = runtime.enter();

        // signal_handler.build() must be called from within the guard.
        let signal_handler = signal_handler.build()?;

        let input_handler = input_handler.build();

        Ok(TestRunner {
            inner: TestRunnerInner {
                capture_strategy: self.capture_strategy,
                profile,
                cli_args,
                test_threads,
                force_retries: self.retries,
                max_fail,
                test_list,
                double_spawn,
                target_runner,
                runtime,
                run_id: ReportUuid::new_v4(),
            },
            signal_handler,
            input_handler,
        })
    }
}

/// Context for running tests.
///
/// Created using [`TestRunnerBuilder::build`].
#[derive(Debug)]
pub struct TestRunner<'a> {
    inner: TestRunnerInner<'a>,
    signal_handler: SignalHandler,
    input_handler: InputHandler,
}

impl<'a> TestRunner<'a> {
    /// Returns the status of the input handler.
    pub fn input_handler_status(&self) -> InputHandlerStatus {
        self.input_handler.status()
    }

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
        E: fmt::Debug + Send,
    {
        let (report_cancel_tx, report_cancel_rx) = oneshot::channel();

        // If report_cancel_tx is None, at least one error has occurred and the
        // runner has been instructed to shut down. first_error is also set to
        // Some in that case.
        let mut report_cancel_tx = Some(report_cancel_tx);
        let mut first_error = None;

        let res = self.inner.execute(
            &mut self.signal_handler,
            &mut self.input_handler,
            report_cancel_rx,
            |event| {
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
            },
        );

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
    max_fail: Option<usize>,
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
        input_handler: &mut InputHandler,
        report_cancel_rx: oneshot::Receiver<()>,
        callback: F,
    ) -> Result<RunStats, Vec<JoinError>>
    where
        F: FnMut(TestEvent<'a>) + Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        let cancelled = AtomicBool::new(false);
        let cancelled_ref = &cancelled;

        let mut dispatcher_cx = DispatcherContext::new(
            callback,
            self.run_id,
            self.profile.name(),
            self.cli_args.clone(),
            self.test_list.run_count(),
            self.max_fail,
        );

        // Send the initial event.
        // (Don't need to set the cancelled atomic if this fails because the run hasn't started
        // yet.)
        dispatcher_cx.run_started(self.test_list);

        let dispatcher_cx_mut = &mut dispatcher_cx;

        let _guard = self.runtime.enter();

        let ((), results) = TokioScope::scope_and_block(move |scope| {
            let (resp_tx, resp_rx) = unbounded_channel::<InternalTestEvent<'a>>();
            let (cancellation_sender, _cancel_receiver) = broadcast::channel(1);

            // Run the dispatcher to completion in a task.
            let dispatcher_fut = dispatcher_cx_mut.run(
                resp_rx,
                signal_handler,
                input_handler,
                report_cancel_rx,
                cancelled_ref,
                cancellation_sender.clone(),
            );
            scope.spawn_cancellable(dispatcher_fut, || ());

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

                        // Drain the request receiver, responding to any final
                        // requests that may have been sent.
                        drain_req_rx(req_rx, UnitExecuteStatus::SetupScript(&status));

                        let (status, env_map) = status.into_external();

                        let _ = this_resp_tx.send(InternalTestEvent::SetupScriptFinished {
                            script_id,
                            config,
                            index,
                            total,
                            status,
                        });

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
                        debug!(test_name = test_instance.name, "running test");

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

                                let run_status =
                                    self.run_test(packet, &this_resp_tx, &mut req_rx).await;

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

                                    let run_status = run_status.into_external();
                                    let previous_result = run_status.result;
                                    let previous_slow = run_status.is_slow;

                                    let _ = this_resp_tx.send(
                                        InternalTestEvent::AttemptFailedWillRetry {
                                            test_instance,
                                            failure_output: settings.failure_output(),
                                            run_status,
                                            delay_before_next_attempt: delay,
                                        },
                                    );

                                    handle_delay_between_attempts(
                                        packet,
                                        previous_result,
                                        previous_slow,
                                        delay,
                                        &mut cancel_receiver,
                                        &mut req_rx,
                                    )
                                    .await;
                                } else {
                                    // This test failed and is out of retries.
                                    break run_status;
                                }
                            };

                            drain_req_rx(req_rx, UnitExecuteStatus::Test(&last_run_status));

                            // At this point, either:
                            // * the test has succeeded, or
                            // * the test has failed and we've run out of retries.
                            // In either case, the test is finished.
                            let last_run_status = last_run_status.into_external();
                            let _ = this_resp_tx.send(InternalTestEvent::Finished {
                                test_instance,
                                success_output: settings.success_output(),
                                failure_output: settings.failure_output(),
                                junit_store_success_output: settings.junit_store_success_output(),
                                junit_store_failure_output: settings.junit_store_failure_output(),
                                last_run_status,
                            });
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

        dispatcher_cx.run_finished();

        // Were there any join errors?
        let join_errors = results
            .into_iter()
            .filter_map(|r| r.err())
            .collect::<Vec<_>>();
        if !join_errors.is_empty() {
            return Err(join_errors);
        }
        Ok(dispatcher_cx.run_stats())
    }

    // ---
    // Helper methods
    // ---

    /// Run an individual setup script in its own process.
    #[instrument(level = "debug", skip(self, resp_tx, req_rx))]
    async fn run_setup_script(
        &self,
        script: SetupScriptPacket<'a>,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    ) -> InternalSetupScriptExecuteStatus<'a> {
        let mut stopwatch = crate::time::stopwatch();

        match self
            .run_setup_script_inner(script.clone(), &mut stopwatch, resp_tx, req_rx)
            .await
        {
            Ok(status) => status,
            Err(error) => InternalSetupScriptExecuteStatus {
                script,
                slow_after: None,
                output: ChildExecutionOutput::StartError(error),
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.snapshot(),
                env_map: None,
            },
        }
    }

    #[instrument(level = "debug", skip(self, resp_tx, req_rx))]
    async fn run_setup_script_inner(
        &self,
        script: SetupScriptPacket<'a>,
        stopwatch: &mut StopwatchStart,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    ) -> Result<InternalSetupScriptExecuteStatus<'a>, ChildStartError> {
        let mut cmd = script.make_command(&self.double_spawn, self.test_list)?;
        let command_mut = cmd.command_mut();

        command_mut.env("NEXTEST_RUN_ID", format!("{}", self.run_id));
        command_mut.stdin(Stdio::null());
        super::os::set_process_group(command_mut);

        // If creating a job fails, we might be on an old system. Ignore this -- job objects are a
        // best-effort thing.
        let job = super::os::Job::create().ok();

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
        let child_pid = child
            .id()
            .expect("child has never been polled so must return a PID");

        // If assigning the child to the job fails, ignore this. This can happen if the process has
        // exited.
        let _ = super::os::assign_process_to_job(&child, job.as_ref());

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

        let mut interval_sleep = std::pin::pin!(crate::time::pausable_sleep(slow_timeout.period));

        let mut timeout_hit = 0;

        let child_fds = ChildFds::new_split(child.stdout.take(), child.stderr.take());
        let mut child_acc = ChildAccumulator::new(child_fds);

        let mut cx = UnitContext {
            packet: UnitPacket::SetupScript(script.clone()),
            slow_after: None,
        };

        let (res, leaked) = {
            let res = loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    res = child.wait() => {
                        // The setup script finished executing.
                        break res;
                    }
                    _ = &mut interval_sleep, if status.is_none() => {
                        // Mark the script as slow.
                        cx.slow_after = Some(slow_timeout.period);

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
                                will_terminate.then_some(slow_timeout.grace_period),
                            ));
                        }

                        if will_terminate {
                            // attempt to terminate the slow test.
                            // as there is a race between shutting down a slow test and its own completion
                            // we silently ignore errors to avoid printing false warnings.
                            super::os::terminate_child(
                                &cx,
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
                        // The sender stays open longer than the whole loop, and the buffer is big
                        // enough for all messages ever sent through this channel, so a RecvError
                        // should never happen.
                        let req = recv.expect("a RecvError should never happen here");

                        match req {
                            RunUnitRequest::Signal(req) => {
                                handle_signal_request(
                                    &cx,
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
                            RunUnitRequest::Query(RunUnitQuery::GetInfo(sender)) => {
                                _ = sender.send(script.info_response(
                                    UnitState::Running {
                                        pid: child_pid,
                                        time_taken:             stopwatch.snapshot().active,
                                        slow_after: cx.slow_after,
                                    },
                                    child_acc.snapshot_in_progress(UnitKind::WAITING_ON_SCRIPT_MESSAGE),
                                ));
                            }
                        }
                    }
                }
            };

            // Build a tentative status using status and the exit status.
            let tentative_status = status.or_else(|| {
                res.as_ref()
                    .ok()
                    .map(|res| create_execution_result(*res, &child_acc.errors, false))
            });

            let leaked = detect_fd_leaks(
                &cx,
                child_pid,
                &mut child_acc,
                tentative_status,
                leak_timeout,
                stopwatch,
                req_rx,
            )
            .await;

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

        let exec_result = status
            .unwrap_or_else(|| create_execution_result(exit_status, &child_acc.errors, leaked));

        // Read from the environment map. If there's an error here, add it to the list of child errors.
        let mut errors: Vec<_> = child_acc.errors.into_iter().map(ChildError::from).collect();
        let env_map = if exec_result.is_success() {
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
            script,
            slow_after: cx.slow_after,
            output: ChildExecutionOutput::Output {
                result: Some(exec_result),
                output: child_acc.output.freeze(),
                errors: ErrorList::new(UnitKind::WAITING_ON_SCRIPT_MESSAGE, errors),
            },
            result: exec_result,
            stopwatch_end: stopwatch.snapshot(),
            env_map,
        })
    }

    /// Run an individual test in its own process.
    #[instrument(level = "debug", skip(self, resp_tx, req_rx))]
    async fn run_test<'test>(
        &self,
        test: TestPacket<'a, 'test>,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    ) -> InternalExecuteStatus<'a, 'test> {
        let mut stopwatch = crate::time::stopwatch();
        let delay_before_start = test.delay_before_start;

        match self
            .run_test_inner(test, &mut stopwatch, resp_tx, req_rx)
            .await
        {
            Ok(run_status) => run_status,
            Err(error) => InternalExecuteStatus {
                test,
                slow_after: None,
                output: ChildExecutionOutput::StartError(error),
                result: ExecutionResult::ExecFail,
                stopwatch_end: stopwatch.snapshot(),
                delay_before_start,
            },
        }
    }

    #[instrument(level = "debug", skip(self, resp_tx, req_rx))]
    async fn run_test_inner<'test>(
        &self,
        test: TestPacket<'a, 'test>,
        stopwatch: &mut StopwatchStart,
        resp_tx: &UnboundedSender<InternalTestEvent<'a>>,
        req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    ) -> Result<InternalExecuteStatus<'a, 'test>, ChildStartError> {
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
        super::os::set_process_group(command_mut);

        // If creating a job fails, we might be on an old system. Ignore this -- job objects are a
        // best-effort thing.
        let job = super::os::Job::create().ok();

        let crate::test_command::Child {
            mut child,
            child_fds,
        } = cmd
            .spawn(self.capture_strategy)
            .map_err(|error| ChildStartError::Spawn(Arc::new(error)))?;

        // Note: The PID stored here must be used with care -- it might be
        // outdated and have been reused by the kernel in case the process
        // has exited. If using for any real logic (not just reporting) it
        // might be best to always check child.id().
        let child_pid = child
            .id()
            .expect("child has never been polled so must return a PID");

        // If assigning the child to the job fails, ignore this. This can happen if the process has
        // exited.
        let _ = super::os::assign_process_to_job(&child, job.as_ref());

        let mut child_acc = ChildAccumulator::new(child_fds);

        let mut status: Option<ExecutionResult> = None;
        let slow_timeout = test.settings.slow_timeout();
        let leak_timeout = test.settings.leak_timeout();

        // Use a pausable_sleep rather than an interval here because it's much
        // harder to pause and resume an interval.
        let mut interval_sleep = std::pin::pin!(crate::time::pausable_sleep(slow_timeout.period));

        let mut timeout_hit = 0;

        let mut cx = UnitContext {
            packet: UnitPacket::Test(test),
            slow_after: None,
        };

        let (res, leaked) = {
            let res = loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    res = child.wait() => {
                        // The test finished executing.
                        break res;
                    }
                    _ = &mut interval_sleep, if status.is_none() => {
                        // Mark the test as slow.
                        cx.slow_after = Some(slow_timeout.period);

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
                            super::os::terminate_child(
                                &cx,
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
                                    &cx,
                                    &mut child,
                                    &mut child_acc,
                                    req,
                                    stopwatch,
                                    interval_sleep.as_mut(),
                                    req_rx,
                                    job.as_ref(),
                                    slow_timeout.grace_period,
                                ).await;
                            }
                            RunUnitRequest::Query(RunUnitQuery::GetInfo(tx)) => {
                                _ = tx.send(test.info_response(
                                    UnitState::Running {
                                        pid: child_pid,
                                        time_taken: stopwatch.snapshot().active,
                                        slow_after: cx.slow_after,
                                    },
                                    child_acc.snapshot_in_progress(UnitKind::WAITING_ON_TEST_MESSAGE),
                                ));
                            }
                        }
                    }
                };
            };

            // Build a tentative status using status and the exit status.
            let tentative_status = status.or_else(|| {
                res.as_ref()
                    .ok()
                    .map(|res| create_execution_result(*res, &child_acc.errors, false))
            });

            let leaked = detect_fd_leaks(
                &cx,
                child_pid,
                &mut child_acc,
                tentative_status,
                leak_timeout,
                stopwatch,
                req_rx,
            )
            .await;

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
        let exec_result = status
            .unwrap_or_else(|| create_execution_result(exit_status, &child_acc.errors, leaked));

        Ok(InternalExecuteStatus {
            test,
            slow_after: cx.slow_after,
            output: ChildExecutionOutput::Output {
                result: Some(exec_result),
                output: child_acc.output.freeze(),
                errors: ErrorList::new(UnitKind::WAITING_ON_TEST_MESSAGE, child_acc.errors),
            },
            result: exec_result,
            stopwatch_end: stopwatch.snapshot(),
            delay_before_start: test.delay_before_start,
        })
    }
}

/// Drains the request receiver of any messages.
fn drain_req_rx<'a>(
    mut receiver: UnboundedReceiver<RunUnitRequest<'a>>,
    status: UnitExecuteStatus<'a, '_>,
) {
    // Mark the receiver closed so no further messages are sent.
    receiver.close();
    loop {
        // Receive anything that's left in the receiver.
        let message = receiver.try_recv();
        match message {
            Ok(message) => {
                message.drain(status);
            }
            Err(_) => {
                break;
            }
        }
    }
}

async fn handle_delay_between_attempts<'a>(
    packet: TestPacket<'a, '_>,
    previous_result: ExecutionResult,
    previous_slow: bool,
    delay: Duration,
    cancel_receiver: &mut broadcast::Receiver<()>,
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
) {
    let mut sleep = std::pin::pin!(crate::time::pausable_sleep(delay));
    #[cfg_attr(not(unix), expect(unused_mut))]
    let mut waiting_stopwatch = crate::time::stopwatch();

    loop {
        tokio::select! {
            _ = &mut sleep => {
                // The timer has expired.
                break;
            }
            _ = cancel_receiver.recv() => {
                // The cancel signal was received.
                break;
            }
            recv = req_rx.recv() => {
                let req = recv.expect("req_rx sender is open");

                match req {
                    #[cfg(unix)]
                    RunUnitRequest::Signal(SignalRequest::Stop(tx)) => {
                        sleep.as_mut().pause();
                        waiting_stopwatch.pause();
                        _ = tx.send(());
                    }
                    #[cfg(unix)]
                    RunUnitRequest::Signal(SignalRequest::Continue) => {
                        if sleep.is_paused() {
                            sleep.as_mut().resume();
                            waiting_stopwatch.resume();
                        }
                    }
                    RunUnitRequest::Signal(SignalRequest::Shutdown(_)) => {
                        // The run was cancelled, so go ahead and perform a
                        // shutdown.
                        break;
                    }
                    RunUnitRequest::Query(RunUnitQuery::GetInfo(tx)) => {
                        let waiting_snapshot = waiting_stopwatch.snapshot();
                        _ = tx.send(
                            packet.info_response(
                                UnitState::DelayBeforeNextAttempt {
                                    previous_result,
                                    previous_slow,
                                    waiting_duration: waiting_snapshot.active,
                                    remaining: delay
                                        .checked_sub(waiting_snapshot.active)
                                        .unwrap_or_default(),
                                },
                                // This field is ignored but our data model
                                // requires it.
                                ChildExecutionOutput::Output {
                                    result: None,
                                    output: ChildOutput::Split(ChildSplitOutput {
                                        stdout: None,
                                        stderr: None,
                                    }),
                                    errors: None,
                                },
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// After a child process has exited, detect if it leaked file handles by
/// leaving long-running grandchildren open.
///
/// This is done by waiting for a short period of time after the child has
/// exited, and checking if stdout and stderr are still open. In the future, we
/// could do more sophisticated checks around e.g. if any processes with the
/// same PGID are around.
async fn detect_fd_leaks<'a>(
    cx: &UnitContext<'a, '_>,
    child_pid: u32,
    child_acc: &mut ChildAccumulator,
    tentative_result: Option<ExecutionResult>,
    leak_timeout: Duration,
    stopwatch: &mut StopwatchStart,
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
) -> bool {
    loop {
        // Ignore stop and continue events here since the leak timeout should be very small.
        // TODO: we may want to consider them.
        let mut sleep = std::pin::pin!(tokio::time::sleep(leak_timeout));
        let waiting_stopwatch = crate::time::stopwatch();

        tokio::select! {
            // All of the branches here need to check for
            // `!child_acc.fds.is_done()`, because if child_fds is done we want
            // to hit the `else` block right away.
            () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
            () = &mut sleep, if !child_acc.fds.is_done() => {
                break true;
            }
            recv = req_rx.recv(), if !child_acc.fds.is_done() => {
                // The sender stays open longer than the whole loop, and the
                // buffer is big enough for all messages ever sent through this
                // channel, so a RecvError should never happen.
                let req = recv.expect("a RecvError should never happen here");

                match req {
                    RunUnitRequest::Signal(_) => {
                        // The process is done executing, so signals are moot.
                    }
                    RunUnitRequest::Query(RunUnitQuery::GetInfo(sender)) => {
                        let snapshot = waiting_stopwatch.snapshot();
                        let resp = cx.info_response(
                            UnitState::Exiting {
                                // Because we've polled that the child is done,
                                // child.id() will likely return None at this
                                // point. Use the cached PID since this is just
                                // for reporting.
                                pid: child_pid,
                                time_taken: stopwatch.snapshot().active,
                                slow_after: cx.slow_after,
                                tentative_result,
                                waiting_duration: snapshot.active,
                                remaining: leak_timeout
                                    .checked_sub(snapshot.active)
                                    .unwrap_or_default(),
                            },
                            child_acc.snapshot_in_progress(cx.packet.kind().waiting_on_message()),
                        );

                        _ = sender.send(resp);
                    }
                }
            }
            else => {
                break false;
            }
        }
    }
}

// It would be nice to fix this function to not have so many arguments, but this
// code is actively being refactored right now and imposing too much structure
// can cause more harm than good.
#[expect(clippy::too_many_arguments)]
async fn handle_signal_request<'a>(
    cx: &UnitContext<'a, '_>,
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
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    job: Option<&super::os::Job>,
    grace_period: Duration,
) {
    match req {
        #[cfg(unix)]
        SignalRequest::Stop(sender) => {
            // It isn't possible to receive a stop event twice since it gets
            // debounced in the main signal handler.
            stopwatch.pause();
            interval_sleep.as_mut().pause();
            super::os::job_control_child(child, crate::signal::JobControlEvent::Stop);
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
                super::os::job_control_child(child, crate::signal::JobControlEvent::Continue);
            }
        }
        SignalRequest::Shutdown(event) => {
            super::os::terminate_child(
                cx,
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
        ExecutionResult::Fail {
            abort_status: AbortStatus::extract(exit_status),
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

/// Events sent from the test runner to individual test (or setup script) execution tasks.
#[derive(Clone, Debug)]
pub(super) enum RunUnitRequest<'a> {
    Signal(SignalRequest),
    Query(RunUnitQuery<'a>),
}

impl<'a> RunUnitRequest<'a> {
    fn drain(self, status: UnitExecuteStatus<'a, '_>) {
        match self {
            #[cfg(unix)]
            Self::Signal(SignalRequest::Stop(sender)) => {
                // The receiver being dead isn't really important.
                let _ = sender.send(());
            }
            #[cfg(unix)]
            Self::Signal(SignalRequest::Continue) => {}
            Self::Signal(SignalRequest::Shutdown(_)) => {}
            Self::Query(RunUnitQuery::GetInfo(tx)) => {
                // The receiver being dead isn't really important.
                _ = tx.send(status.info_response());
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum SignalRequest {
    // The mpsc sender is used by each test to indicate that the stop signal has been sent.
    #[cfg(unix)]
    Stop(UnboundedSender<()>),
    #[cfg(unix)]
    Continue,
    Shutdown(ShutdownRequest),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum ShutdownRequest {
    Once(ShutdownEvent),
    Twice,
}

/// Either a test or a setup script, along with information about how long the
/// test took.
pub(super) struct UnitContext<'a, 'test> {
    packet: UnitPacket<'a, 'test>,
    // TODO: This is a bit of a mess. It isn't clear where this kind of state
    // should live -- many parts of the request-response system need various
    // pieces of this code.
    slow_after: Option<Duration>,
}

impl<'a, 'test> UnitContext<'a, 'test> {
    #[cfg_attr(not(unix), expect(dead_code))]
    pub(super) fn packet(&self) -> &UnitPacket<'a, 'test> {
        &self.packet
    }

    pub(super) fn info_response(
        &self,
        state: UnitState,
        output: ChildExecutionOutput,
    ) -> InfoResponse<'a> {
        match &self.packet {
            UnitPacket::SetupScript(packet) => packet.info_response(state, output),
            UnitPacket::Test(packet) => packet.info_response(state, output),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum UnitPacket<'a, 'test> {
    SetupScript(SetupScriptPacket<'a>),
    Test(TestPacket<'a, 'test>),
}

impl UnitPacket<'_, '_> {
    pub(super) fn kind(&self) -> UnitKind {
        match self {
            Self::SetupScript(_) => UnitKind::Script,
            Self::Test(_) => UnitKind::Test,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TestPacket<'a, 'test> {
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

    pub(super) fn retry_data(&self) -> RetryData {
        self.retry_data
    }

    pub(super) fn info_response(
        &self,
        state: UnitState,
        output: ChildExecutionOutput,
    ) -> InfoResponse<'a> {
        InfoResponse::Test(TestInfoResponse {
            test_instance: self.test_instance.id(),
            state,
            retry_data: self.retry_data,
            output,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct SetupScriptPacket<'a> {
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

    pub(super) fn info_response(
        &self,
        state: UnitState,
        output: ChildExecutionOutput,
    ) -> InfoResponse<'a> {
        InfoResponse::SetupScript(SetupScriptInfoResponse {
            script_id: self.script_id.clone(),
            command: self.config.program(),
            args: self.config.args(),
            state,
            output,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) enum RunUnitQuery<'a> {
    GetInfo(UnboundedSender<InfoResponse<'a>>),
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
    super::os::configure_handle_inheritance_impl(no_capture)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TerminateMode {
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
        let signal_handler = SignalHandlerKind::Noop;
        let input_handler = InputHandlerKind::Noop;
        let profile = profile.apply_build_platforms(&build_platforms);
        let runner = builder
            .build(
                &test_list,
                &profile,
                vec![],
                signal_handler,
                input_handler,
                DoubleSpawnInfo::disabled(),
                TargetRunner::empty(),
            )
            .unwrap();
        assert_eq!(runner.inner.capture_strategy, CaptureStrategy::None);
        assert_eq!(runner.inner.test_threads, 1, "tests run serially");
    }
}
