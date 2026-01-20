// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The controller for the test runner.
//!
//! This module interfaces with the external world and the test executor. It
//! receives events from the executor and from other inputs (e.g. signal and
//! input handling), and sends events to the reporter.

use super::{RunUnitRequest, RunnerTaskState, ShutdownRequest};
use crate::{
    config::{
        elements::{MaxFail, TerminateMode},
        scripts::{ScriptId, SetupScriptConfig},
    },
    input::{InputEvent, InputHandler},
    list::{OwnedTestInstanceId, TestInstance, TestInstanceId, TestInstanceIdKey, TestList},
    reporter::events::{
        CancelReason, ChildExecutionOutputDescription, ExecuteStatus, ExecutionResultDescription,
        ExecutionStatuses, FailureDescription, FinalRunStats, InfoResponse, ReporterEvent,
        RunFinishedStats, RunStats, StressIndex, StressProgress, StressRunStats, TestEvent,
        TestEventKind, TestsNotSeen,
    },
    runner::{ExecutorEvent, RunUnitQuery, SignalRequest, StressCondition, StressCount},
    signal::{
        JobControlEvent, ShutdownEvent, ShutdownSignalEvent, SignalEvent, SignalHandler,
        SignalInfoEvent,
    },
    test_output::ChildSingleOutput,
    time::StopwatchStart,
};
use chrono::Local;
use debug_ignore::DebugIgnore;
use futures::future::{Fuse, FusedFuture};
use nextest_metadata::MismatchReason;
use quick_junit::ReportUuid;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, mem,
    pin::Pin,
    time::Duration,
};
use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
        oneshot,
    },
    time::MissedTickBehavior,
};
use tracing::debug;

/// Context for the dispatcher.
///
/// This struct is responsible for coordinating events from the outside world
/// and communicating with the executor.
#[derive(Clone)]
#[derive_where::derive_where(Debug)]
pub(super) struct DispatcherContext<'a, F> {
    callback: DebugIgnore<F>,
    run_id: ReportUuid,
    profile_name: String,
    cli_args: Vec<String>,
    stopwatch: StopwatchStart,
    run_stats: RunStats,
    max_fail: MaxFail,
    global_timeout: Duration,
    running_setup_script: Option<ContextSetupScript<'a>>,
    running_tests: BTreeMap<TestInstanceId<'a>, ContextTestInstance<'a>>,
    signal_count: Option<SignalCount>,
    stress_cx: DispatcherStressContext,
    tick_interval: Duration,
    rerun_cx: DispatcherRerunContext,
    #[cfg(test)]
    disable_signal_3_times_panic: bool,
}

impl<'a, F> DispatcherContext<'a, F>
where
    F: FnMut(ReporterEvent<'a>) + Send,
{
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new(
        callback: F,
        run_id: ReportUuid,
        profile_name: &str,
        cli_args: Vec<String>,
        initial_run_count: usize,
        max_fail: MaxFail,
        global_timeout: Duration,
        stress_condition: Option<StressCondition>,
        expected_outstanding: Option<BTreeSet<OwnedTestInstanceId>>,
    ) -> Self {
        // Tick every 50ms by default.
        let tick_interval_ms = env::var("NEXTEST_PROGRESS_TICK_INTERVAL_MS")
            .ok()
            .and_then(|interval| interval.parse::<u64>().ok())
            .unwrap_or(50);
        Self {
            callback: DebugIgnore(callback),
            run_id,
            stopwatch: crate::time::stopwatch(),
            profile_name: profile_name.to_owned(),
            cli_args,
            run_stats: RunStats {
                initial_run_count,
                ..RunStats::default()
            },
            max_fail,
            global_timeout,
            running_setup_script: None,
            running_tests: BTreeMap::new(),
            signal_count: None,
            stress_cx: DispatcherStressContext::new(stress_condition),
            tick_interval: Duration::from_millis(tick_interval_ms),
            rerun_cx: DispatcherRerunContext::new(expected_outstanding),
            #[cfg(test)]
            disable_signal_3_times_panic: false,
        }
    }

    /// Runs the dispatcher to completion, until `resp_rx` is closed.
    ///
    /// `executor_rx` is the main communication channel between the dispatcher
    /// and the executor. It receives events, but some of those events also
    /// include senders for the dispatcher to communicate back to the executor.
    ///
    /// This is expected to be spawned as a task via [`async_scoped`].
    pub(super) async fn run(
        &mut self,
        mut executor_rx: UnboundedReceiver<ExecutorEvent<'a>>,
        signal_handler: &mut SignalHandler,
        input_handler: &mut InputHandler,
        mut report_cancel_rx: Pin<&mut Fuse<oneshot::Receiver<()>>>,
    ) -> RunnerTaskState {
        let mut signals_done = false;
        let mut inputs_done = false;
        // For stress tests, this function is called for each sub-run -- in
        // other words, we reinitialize the global timeout for each sub-run.
        let mut global_timeout_sleep =
            std::pin::pin!(crate::time::pausable_sleep(self.global_timeout));

        // This is the interval at which tick events are sent to the reporter.
        let mut tick_interval = tokio::time::interval(self.tick_interval);
        tick_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            let internal_event = tokio::select! {
                _ = &mut global_timeout_sleep => {
                    InternalEvent::GlobalTimeout
                },
                _ = tick_interval.tick() => {
                    InternalEvent::Tick
                },
                internal_event = executor_rx.recv() => {
                    match internal_event {
                        Some(event) => InternalEvent::Executor(event),
                        None => {
                            // All runs have been completed.
                            break RunnerTaskState::finished_no_children();
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
                internal_event = input_handler.recv(), if !inputs_done => {
                    match internal_event {
                        Some(event) => InternalEvent::Input(event),
                        None => {
                            inputs_done = true;
                            continue;
                        }
                    }
                }
                res = &mut report_cancel_rx, if !report_cancel_rx.as_ref().is_terminated() => {
                    match res {
                        Ok(()) => {
                            InternalEvent::ReportCancel
                        }
                        Err(_) => {
                            // In normal operation, the sender is kept alive
                            // until the end of the run, so this should never
                            // fail. However there are circumstances around
                            // shutdown where it may be possible that the sender
                            // isn't kept alive. In those cases, we just ignore
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

            match self.handle_event(internal_event) {
                #[cfg(unix)]
                HandleEventResponse::JobControl(JobControlEvent::Stop) => {
                    // This is in reality bounded by the number of tests
                    // currently running.
                    let (status_tx, mut status_rx) = unbounded_channel();
                    self.broadcast_request(RunUnitRequest::Signal(SignalRequest::Stop(status_tx)));

                    debug!(
                        remaining = status_rx.sender_strong_count(),
                        "stopping tests"
                    );

                    // There's a possibility of a race condition between a test
                    // exiting and sending the message to the receiver. For that
                    // reason, don't wait more than 100ms on children to stop.
                    let mut sleep = std::pin::pin!(tokio::time::sleep(Duration::from_millis(100)));

                    loop {
                        tokio::select! {
                            res = status_rx.recv() => {
                                debug!(
                                    res = ?res,
                                    remaining = status_rx.sender_strong_count(),
                                    "test stopped",
                                );
                                if res.is_none() {
                                    // No remaining message in the channel's
                                    // buffer.
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

                    // Restore the terminal state.
                    input_handler.suspend();

                    // Pause the global timeout while suspended.
                    global_timeout_sleep.as_mut().pause();

                    // Also pause the stress stopwatch while suspended.
                    self.stress_cx.pause_stopwatch();

                    // Now stop nextest itself.
                    super::os::raise_stop();
                }
                #[cfg(unix)]
                HandleEventResponse::JobControl(JobControlEvent::Continue) => {
                    // Nextest has been resumed. Resume the input handler, as well as all the tests.
                    input_handler.resume();

                    // Resume the global timeout.
                    global_timeout_sleep.as_mut().resume();

                    // Also resume the stress stopwatch.
                    self.stress_cx.resume_stopwatch();

                    self.broadcast_request(RunUnitRequest::Signal(SignalRequest::Continue));
                }
                #[cfg(not(unix))]
                HandleEventResponse::JobControl(e) => {
                    // On platforms other than Unix this enum is expected to be
                    // empty; we can check this assumption at compile time like
                    // so.
                    //
                    // Rust 1.82 handles empty enums better, and this won't be
                    // required after we bump the MSRV to that.
                    match e {}
                }
                HandleEventResponse::Info(_) => {
                    // In reality, this is bounded by the number of
                    // tests running at the same time.
                    let (sender, mut receiver) = unbounded_channel();
                    let total = self
                        .broadcast_request(RunUnitRequest::Query(RunUnitQuery::GetInfo(sender)));

                    let mut index = 0;

                    self.info_started(total);
                    debug!(expected = total, "waiting for info responses");

                    loop {
                        // Don't wait too long for tasks to respond, to avoid a
                        // hung unit task.
                        let sleep = tokio::time::sleep(Duration::from_millis(100));
                        tokio::select! {
                            res = receiver.recv() => {
                                if let Some(info) = res {
                                    debug!(
                                        index,
                                        expected = total,
                                        remaining = total.saturating_sub(index + 1),
                                        sender_strong_count = receiver.sender_strong_count(),
                                        "received info response",
                                    );

                                    self.info_response(
                                        index,
                                        total,
                                        info,
                                    );
                                    index += 1;
                                } else {
                                    // All senders have been dropped.
                                    break;
                                }
                            }
                            _ = sleep => {
                                debug!(
                                    remaining = total.saturating_sub(index + 1),
                                    sender_strong_count = receiver.sender_strong_count(),
                                    "timeout waiting for tests to stop, ignoring",
                                );
                                break;
                            }
                        };
                    }

                    self.info_finished(total.saturating_sub(index + 1));
                }
                HandleEventResponse::Cancel(cancel) => {
                    // A cancellation notice was received.
                    match cancel {
                        // Some of the branches here don't do anything, but are specified
                        // for readability.
                        CancelEvent::Report => {
                            // An error was produced by the reporter, and cancellation has
                            // begun.
                            self.broadcast_request(RunUnitRequest::OtherCancel);
                        }
                        CancelEvent::TestFailure => {
                            // A test failure has caused cancellation to begin.
                            self.broadcast_request(RunUnitRequest::OtherCancel);
                        }
                        CancelEvent::GlobalTimeout => {
                            // The global timeout has expired, causing cancellation to begin.
                            self.broadcast_request(RunUnitRequest::Signal(
                                SignalRequest::Shutdown(ShutdownRequest::Once(
                                    ShutdownEvent::TERMINATE,
                                )),
                            ));
                        }
                        CancelEvent::Signal(req) => {
                            // A signal has caused cancellation to begin. Let all the child
                            // processes know about the signal, and continue to handle
                            // events.
                            //
                            // Ignore errors here: if there are no receivers to cancel, so
                            // be it. Also note the ordering here: cancelled_ref is set
                            // *before* this is sent.
                            self.broadcast_request(RunUnitRequest::Signal(
                                SignalRequest::Shutdown(req),
                            ));
                        }
                    }
                }
                HandleEventResponse::None => {}
            }
        }
    }

    pub(super) fn run_started(&mut self, test_list: &'a TestList, test_threads: usize) {
        let (stress_count, stress_infinite, stress_duration_nanos) =
            match self.stress_cx.condition() {
                Some(StressCondition::Count(StressCount::Count { count })) => {
                    (Some(count.get()), false, None)
                }
                Some(StressCondition::Count(StressCount::Infinite)) => (None, true, None),
                Some(StressCondition::Duration(duration)) => {
                    (None, false, Some(duration.as_nanos() as u64))
                }
                None => (None, false, None),
            };
        crate::fire_usdt!(UsdtRunStart {
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            total_tests: test_list.test_count(),
            filter_count: test_list.run_count(),
            test_threads,
            stress_count,
            stress_infinite,
            stress_duration_nanos,
        });

        self.basic_callback(TestEventKind::RunStarted {
            test_list,
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            cli_args: self.cli_args.clone(),
            stress_condition: self.stress_cx.condition(),
        })
    }

    pub(super) fn stress_sub_run_started(&mut self, progress: StressProgress) {
        // Reset run stats since we're starting over. Do this here rather than
        // in stress_sub_run_finished because we sometimes fetch run_stats after
        // stress_sub_run_finished and want it to be accurate until the next
        // sub-run starts.
        let sub_stats = self.run_stats;
        self.run_stats = RunStats {
            initial_run_count: sub_stats.initial_run_count,
            ..Default::default()
        };

        // Fire the USDT probe for stress sub-run start.
        let (stress_current, stress_total) = match &progress {
            StressProgress::Count {
                total,
                completed,
                elapsed: _,
            } => {
                let total = match total {
                    StressCount::Count { count } => Some(count.get()),
                    StressCount::Infinite => None,
                };
                (*completed, total)
            }
            StressProgress::Time {
                total: _,
                elapsed: _,
                completed,
            } => (*completed, None),
        };
        crate::fire_usdt!(UsdtStressSubRunStart {
            stress_sub_run_id: progress.unique_id(self.run_id),
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            stress_current,
            stress_total,
            elapsed_nanos: self.stopwatch.snapshot().active.as_nanos() as u64,
        });

        self.basic_callback(TestEventKind::StressSubRunStarted { progress })
    }

    pub(super) fn stress_sub_run_finished(&mut self) {
        let sub_elapsed = self
            .stress_cx
            .mark_completed(self.run_stats.summarize_final());
        let progress = self
            .stress_progress()
            .expect("stress_sub_run_finished called in non-stress test context");

        // Fire the USDT probe for stress sub-run done.
        let (stress_current, stress_total) = match &progress {
            StressProgress::Count {
                total,
                completed,
                elapsed: _,
            } => {
                let total = match total {
                    StressCount::Count { count } => Some(count.get()),
                    StressCount::Infinite => None,
                };
                (*completed - 1, total)
            }
            StressProgress::Time {
                total: _,
                elapsed: _,
                completed,
            } => (*completed - 1, None),
        };
        crate::fire_usdt!(UsdtStressSubRunDone {
            stress_sub_run_id: progress.unique_id(self.run_id),
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            stress_current,
            stress_total,
            elapsed_nanos: self.stopwatch.snapshot().active.as_nanos() as u64,
            sub_run_duration_nanos: sub_elapsed.as_nanos() as u64,
            total_tests: self.run_stats.initial_run_count,
            passed: self.run_stats.passed,
            failed: self.run_stats.failed_count(),
            skipped: self.run_stats.skipped,
        });

        self.basic_callback(TestEventKind::StressSubRunFinished {
            progress,
            sub_elapsed,
            sub_stats: self.run_stats,
        })
    }

    pub(super) fn stress_index(&self) -> Option<StressIndex> {
        self.stress_cx.stress_index()
    }

    pub(super) fn stress_progress(&self) -> Option<StressProgress> {
        self.stress_cx.progress(self.stopwatch.snapshot().active)
    }

    /// Returns the reason for cancellation, or `None` if the run is not cancelled.
    pub(super) fn cancel_reason(&self) -> Option<CancelReason> {
        self.run_stats.cancel_reason
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
        (self.callback)(ReporterEvent::Test(Box::new(event)))
    }

    #[inline]
    fn callback_none_response(&mut self, kind: TestEventKind<'a>) -> HandleEventResponse {
        self.basic_callback(kind);
        HandleEventResponse::None
    }

    fn handle_event(&mut self, event: InternalEvent<'a>) -> HandleEventResponse {
        match event {
            InternalEvent::Tick => {
                (self.callback)(ReporterEvent::Tick);
                HandleEventResponse::None
            }
            InternalEvent::Executor(ExecutorEvent::SetupScriptStarted {
                stress_index,
                script_id,
                config,
                program,
                index,
                total,
                req_rx_tx,
            }) => {
                if self.run_stats.cancel_reason.is_some() {
                    // The run has been cancelled: don't start any new units.
                    return HandleEventResponse::None;
                }

                let (req_tx, req_rx) = unbounded_channel();
                match req_rx_tx.send(req_rx) {
                    Ok(_) => {}
                    Err(_) => {
                        // The test task died?
                        debug!(?script_id, "test task died, ignoring");
                        return HandleEventResponse::None;
                    }
                }
                self.new_setup_script(script_id.clone(), config, index, total, req_tx);

                self.callback_none_response(TestEventKind::SetupScriptStarted {
                    stress_index,
                    index,
                    total,
                    script_id,
                    program,
                    args: config.command.args.clone(),
                    no_capture: config.no_capture(),
                })
            }
            InternalEvent::Executor(ExecutorEvent::SetupScriptSlow {
                stress_index,
                script_id,
                config,
                program,
                elapsed,
                will_terminate,
            }) => {
                // Fire the USDT probe for setup script slow.
                crate::fire_usdt!(UsdtSetupScriptSlow {
                    id: script_id.unique_id(self.run_id, stress_index.map(|s| s.current)),
                    run_id: self.run_id,
                    script_id: script_id.to_string(),
                    program: program.clone(),
                    args: config.command.args.clone(),
                    elapsed_nanos: elapsed.as_nanos() as u64,
                    will_terminate: will_terminate.is_some(),
                    stress_current: stress_index.map(|s| s.current),
                    stress_total: stress_index.and_then(|s| s.total.map(|t| t.get())),
                });

                self.callback_none_response(TestEventKind::SetupScriptSlow {
                    stress_index,
                    script_id,
                    program,
                    args: config.command.args.clone(),
                    elapsed,
                    will_terminate: will_terminate.is_some(),
                })
            }
            InternalEvent::Executor(ExecutorEvent::SetupScriptFinished {
                stress_index,
                script_id,
                config,
                program,
                index,
                total,
                status,
            }) => {
                self.finish_setup_script();
                self.run_stats.on_setup_script_finished(&status);

                // Fire the setup-script-done probe, extracting the exit code
                // from the result if available.
                let exit_code = match &status.result {
                    ExecutionResultDescription::Fail {
                        failure: FailureDescription::ExitCode { code },
                        ..
                    } => Some(*code),
                    _ => None,
                };

                // Extract stdout and stderr lengths from the output.
                let (stdout_len, stderr_len) = match &status.output {
                    ChildExecutionOutputDescription::Output { output, .. } => {
                        output.stdout_stderr_len()
                    }
                    ChildExecutionOutputDescription::StartError(_) => (None, None),
                };

                crate::fire_usdt!(UsdtSetupScriptDone {
                    id: script_id.unique_id(self.run_id, stress_index.map(|s| s.current)),
                    run_id: self.run_id,
                    script_id: script_id.to_string(),
                    program: program.clone(),
                    args: config.command.args.clone(),
                    result: status.result.as_static_str(),
                    exit_code,
                    duration_nanos: status.time_taken.as_nanos() as u64,
                    stress_current: stress_index.map(|s| s.current),
                    stress_total: stress_index.and_then(|s| s.total.map(|t| t.get())),
                    stdout_len,
                    stderr_len,
                });

                // Setup scripts failing always cause the entire test run to be cancelled
                // (--no-fail-fast is ignored).
                let fail_cancel = !status.result.is_success();

                self.basic_callback(TestEventKind::SetupScriptFinished {
                    stress_index,
                    index,
                    total,
                    script_id,
                    program,
                    args: config.command.args.clone(),
                    no_capture: config.no_capture(),
                    junit_store_success_output: config.junit.store_success_output,
                    junit_store_failure_output: config.junit.store_failure_output,
                    run_status: status,
                });

                if fail_cancel {
                    self.begin_cancel(CancelReason::SetupScriptFailure, CancelEvent::TestFailure)
                } else {
                    HandleEventResponse::None
                }
            }
            InternalEvent::Executor(ExecutorEvent::Started {
                stress_index,
                test_instance,
                command_line,
                req_rx_tx,
            }) => {
                if self.run_stats.cancel_reason.is_some() {
                    // The run has been cancelled: don't start any new units.
                    return HandleEventResponse::None;
                }

                let (req_tx, req_rx) = unbounded_channel();
                match req_rx_tx.send(req_rx) {
                    Ok(_) => {}
                    Err(_) => {
                        // The test task died?
                        debug!(test = ?test_instance.id(), "test task died, ignoring");
                        return HandleEventResponse::None;
                    }
                }
                self.new_test(test_instance, req_tx);
                self.callback_none_response(TestEventKind::TestStarted {
                    stress_index,
                    test_instance: test_instance.id(),
                    current_stats: self.run_stats,
                    running: self.running_tests.len(),
                    command_line,
                })
            }
            InternalEvent::Executor(ExecutorEvent::Slow {
                stress_index,
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            }) => {
                // Fire the test-slow probe.
                crate::fire_usdt!(UsdtTestAttemptSlow {
                    attempt_id: test_instance.id().attempt_id(
                        self.run_id,
                        stress_index.map(|s| s.current),
                        retry_data.attempt,
                    ),
                    run_id: self.run_id,
                    binary_id: test_instance.suite_info.binary_id.clone(),
                    test_name: test_instance.name.to_owned(),
                    attempt: retry_data.attempt,
                    total_attempts: retry_data.total_attempts,
                    elapsed_nanos: elapsed.as_nanos() as u64,
                    will_terminate: will_terminate.is_some(),
                    stress_current: stress_index.map(|s| s.current),
                    stress_total: stress_index.and_then(|s| s.total.map(|t| t.get())),
                });

                self.callback_none_response(TestEventKind::TestSlow {
                    stress_index,
                    test_instance: test_instance.id(),
                    retry_data,
                    elapsed,
                    will_terminate: will_terminate.is_some(),
                })
            }
            InternalEvent::Executor(ExecutorEvent::AttemptFailedWillRetry {
                stress_index,
                test_instance,
                failure_output,
                run_status,
                delay_before_next_attempt,
            }) => {
                let instance = self.existing_test(test_instance.id());
                instance.attempt_failed_will_retry(run_status.clone());
                self.callback_none_response(TestEventKind::TestAttemptFailedWillRetry {
                    stress_index,
                    test_instance: test_instance.id(),
                    failure_output,
                    run_status,
                    delay_before_next_attempt,
                    running: self.running_tests.len(),
                })
            }
            InternalEvent::Executor(ExecutorEvent::RetryStarted {
                stress_index,
                test_instance,
                retry_data,
                command_line,
                tx,
            }) => {
                if self.run_stats.cancel_reason.is_some() {
                    // The run has been cancelled: don't send a message over the tx and don't start
                    // any new units.
                    return HandleEventResponse::None;
                }

                match tx.send(()) {
                    Ok(_) => {}
                    Err(_) => {
                        // The test task died?
                        debug!(test = ?test_instance.id(), "test task died, ignoring");
                        return HandleEventResponse::None;
                    }
                }

                self.callback_none_response(TestEventKind::TestRetryStarted {
                    stress_index,
                    test_instance: test_instance.id(),
                    retry_data,
                    running: self.running_tests.len(),
                    command_line,
                })
            }
            InternalEvent::Executor(ExecutorEvent::Finished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                last_run_status,
            }) => {
                let run_statuses = self.finish_test(test_instance.id(), last_run_status);
                self.run_stats.on_test_finished(&run_statuses);

                // Check if this run should be cancelled because of a failure.
                // is_exceeded returns Some(terminate_mode) if max-fail is exceeded.
                let terminate_mode = self.max_fail.is_exceeded(self.run_stats.failed_count());

                self.basic_callback(TestEventKind::TestFinished {
                    stress_index,
                    test_instance: test_instance.id(),
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats: self.run_stats,
                    running: self.running(),
                });

                if let Some(terminate_mode) = terminate_mode {
                    // A test failed: start cancellation if required.
                    // Check if we should terminate immediately or wait for running tests.
                    match terminate_mode {
                        TerminateMode::Immediate => {
                            // Terminate running tests immediately.
                            self.broadcast_request(RunUnitRequest::Signal(
                                SignalRequest::Shutdown(ShutdownRequest::Once(
                                    ShutdownEvent::TestFailureImmediate,
                                )),
                            ));
                            self.begin_cancel(
                                CancelReason::TestFailureImmediate,
                                CancelEvent::Signal(ShutdownRequest::Once(
                                    ShutdownEvent::TestFailureImmediate,
                                )),
                            )
                        }
                        TerminateMode::Wait => {
                            self.begin_cancel(CancelReason::TestFailure, CancelEvent::TestFailure)
                        }
                    }
                } else {
                    HandleEventResponse::None
                }
            }
            InternalEvent::Executor(ExecutorEvent::Skipped {
                stress_index,
                test_instance,
                reason,
            }) => {
                // If the mismatch reason is that this test isn't a benchmark,
                // we don't display it in the skip counts (but still keep track
                // of it internally).
                if !matches!(reason, MismatchReason::NotBenchmark) {
                    self.run_stats.skipped += 1;
                }
                self.callback_none_response(TestEventKind::TestSkipped {
                    stress_index,
                    test_instance: test_instance.id(),
                    reason,
                })
            }
            InternalEvent::Signal(event) => self.handle_signal_event(event),
            InternalEvent::GlobalTimeout => {
                self.begin_cancel(CancelReason::GlobalTimeout, CancelEvent::GlobalTimeout)
            }
            InternalEvent::Input(InputEvent::Info) => {
                // Print current statistics.
                HandleEventResponse::Info(InfoEvent::Input)
            }
            InternalEvent::Input(InputEvent::Enter) => {
                self.callback_none_response(TestEventKind::InputEnter {
                    current_stats: self.run_stats,
                    running: self.running(),
                })
            }
            InternalEvent::ReportCancel => {
                self.begin_cancel(CancelReason::ReportError, CancelEvent::Report)
            }
        }
    }

    fn new_setup_script(
        &mut self,
        id: ScriptId,
        config: &'a SetupScriptConfig,
        index: usize,
        total: usize,
        req_tx: UnboundedSender<RunUnitRequest<'a>>,
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

    fn new_test(
        &mut self,
        instance: TestInstance<'a>,
        req_tx: UnboundedSender<RunUnitRequest<'a>>,
    ) {
        // Track this test as seen for rerun tracking.
        self.rerun_cx.mark_seen(instance.id());

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
        last_run_status: ExecuteStatus<ChildSingleOutput>,
    ) -> ExecutionStatuses<ChildSingleOutput> {
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

    /// Returns the number of units the request was broadcast to.
    fn broadcast_request(&self, req: RunUnitRequest<'a>) -> usize {
        let mut count = 0;

        if let Some(setup_script) = &self.running_setup_script {
            if setup_script.req_tx.send(req.clone()).is_err() {
                // The most likely reason for this error is that the setup
                // script has been marked as closed but we haven't processed the
                // exit event yet.
                debug!(?setup_script.id, "failed to send request to setup script (likely closed)");
            } else {
                count += 1;
            }
        }

        for (key, instance) in &self.running_tests {
            if instance.req_tx.send(req.clone()).is_err() {
                // The most likely reason for this error is that the test
                // instance has been marked as closed but we haven't processed
                // the exit event yet.
                debug!(
                    ?key,
                    "failed to send request to test instance (likely closed)"
                );
            } else {
                count += 1;
            }
        }

        count
    }

    fn handle_signal_event(&mut self, event: SignalEvent) -> HandleEventResponse {
        match event {
            SignalEvent::Shutdown(event) => {
                // TestFailureImmediate doesn't participate in signal count escalation.
                // It can only happen once and doesn't escalate to Twice on repetition.
                let req = match event {
                    ShutdownEvent::TestFailureImmediate => ShutdownRequest::Once(event),
                    ShutdownEvent::Signal(_) => {
                        let signal_count = self.increment_signal_count();
                        signal_count.to_request(event)
                    }
                };
                let cancel_reason = event_to_cancel_reason(event);

                self.begin_cancel(cancel_reason, CancelEvent::Signal(req))
            }
            #[cfg(unix)]
            SignalEvent::JobControl(JobControlEvent::Stop) => {
                // Debounce stop signals.
                if !self.stopwatch.is_paused() {
                    self.basic_callback(TestEventKind::RunPaused {
                        setup_scripts_running: self.setup_scripts_running(),
                        running: self.running(),
                    });
                    self.stopwatch.pause();
                    HandleEventResponse::JobControl(JobControlEvent::Stop)
                } else {
                    HandleEventResponse::None
                }
            }
            #[cfg(unix)]
            SignalEvent::JobControl(JobControlEvent::Continue) => {
                // Debounce continue signals.
                if self.stopwatch.is_paused() {
                    self.stopwatch.resume();
                    self.basic_callback(TestEventKind::RunContinued {
                        setup_scripts_running: self.setup_scripts_running(),
                        running: self.running(),
                    });
                    HandleEventResponse::JobControl(JobControlEvent::Continue)
                } else {
                    HandleEventResponse::None
                }
            }
            SignalEvent::Info(event) => HandleEventResponse::Info(InfoEvent::Signal(event)),
        }
    }

    fn info_started(&mut self, total: usize) {
        self.basic_callback(TestEventKind::InfoStarted {
            // Due to a race between units exiting and the info request being
            // broadcast, we rely on the info event's receiver count to
            // determine how many responses we're expecting. We expect every
            // unit that gets a request to return a response.
            total,
            run_stats: self.run_stats,
        });
    }

    fn info_response(&mut self, index: usize, total: usize, response: InfoResponse<'a>) {
        self.basic_callback(TestEventKind::InfoResponse {
            index,
            total,
            response,
        });
    }

    fn info_finished(&mut self, missing: usize) {
        self.basic_callback(TestEventKind::InfoFinished { missing });
    }

    fn increment_signal_count(&mut self) -> SignalCount {
        let new_count = match self.signal_count {
            None => SignalCount::Once,
            Some(SignalCount::Once) => SignalCount::Twice,
            Some(SignalCount::Twice) => {
                // The process was signaled 3 times. Time to panic.
                #[cfg(test)]
                {
                    if self.disable_signal_3_times_panic {
                        SignalCount::Twice
                    } else {
                        // TODO: a panic here won't currently lead to other
                        // tasks being cancelled. This should be fixed.
                        panic!("Signaled 3 times, exiting immediately");
                    }
                }
                #[cfg(not(test))]
                panic!("Signaled 3 times, exiting immediately");
            }
        };
        self.signal_count = Some(new_count);
        new_count
    }

    /// Begin cancellation of a test run. Report it if the current cancel state
    /// is less than the required one.
    ///
    /// Returns the corresponding `HandleEventResponse`.
    fn begin_cancel(&mut self, reason: CancelReason, event: CancelEvent) -> HandleEventResponse {
        // TODO: combine reason and event? The Twice block ignoring the event
        // seems to indicate a data modeling issue.
        if event == CancelEvent::Signal(ShutdownRequest::Twice) {
            // Forcibly kill child processes in the case of a second shutdown
            // signal.
            self.run_stats.cancel_reason = Some(CancelReason::SecondSignal);
            self.basic_callback(TestEventKind::RunBeginKill {
                setup_scripts_running: self.setup_scripts_running(),
                current_stats: self.run_stats,
                running: self.running(),
            });
            HandleEventResponse::Cancel(event)
        } else if self.run_stats.cancel_reason < Some(reason) {
            self.run_stats.cancel_reason = Some(reason);
            self.basic_callback(TestEventKind::RunBeginCancel {
                setup_scripts_running: self.setup_scripts_running(),
                current_stats: self.run_stats,
                running: self.running(),
            });
            HandleEventResponse::Cancel(event)
        } else {
            HandleEventResponse::None
        }
    }

    pub(super) fn run_finished(&mut self) {
        let stopwatch_end = self.stopwatch.snapshot();

        let stress_stats = self.stress_cx.run_stats(self.run_stats.summarize_final());
        let (stress_completed, stress_success, stress_failed) = match &stress_stats {
            Some(stats) => (
                Some(stats.completed.current),
                Some(stats.success_count),
                Some(stats.failed_count),
            ),
            None => (None, None, None),
        };

        crate::fire_usdt!(UsdtRunDone {
            run_id: self.run_id,
            profile_name: self.profile_name.clone(),
            total_tests: self.run_stats.initial_run_count,
            passed: self.run_stats.passed,
            failed: self.run_stats.failed_count(),
            skipped: self.run_stats.skipped,
            duration_nanos: stopwatch_end.active.as_nanos() as u64,
            paused_nanos: stopwatch_end.paused.as_nanos() as u64,
            stress_completed,
            stress_success,
            stress_failed,
        });

        let rerun_cx = mem::replace(&mut self.rerun_cx, DispatcherRerunContext::InitialRun);
        let tests_not_seen = rerun_cx.into_tests_not_seen();

        self.basic_callback(TestEventKind::RunFinished {
            start_time: stopwatch_end.start_time.fixed_offset(),
            run_id: self.run_id,
            elapsed: stopwatch_end.active,
            run_stats: stress_stats.map_or_else(
                || RunFinishedStats::Single(self.run_stats),
                RunFinishedStats::Stress,
            ),
            outstanding_not_seen: tests_not_seen,
        });
    }

    pub(super) fn run_stats(&self) -> RunStats {
        self.run_stats
    }
}

#[derive(Clone, Debug)]
enum DispatcherStressContext {
    None,
    Stress {
        condition: StressCondition,
        sub_stopwatch: StopwatchStart,
        completed: u32,
        failed: u32,
        cancelled: bool,
    },
}

/// Context for rerun tracking.
///
/// This enum tracks whether the current run is an initial run or a rerun, and
/// maintains the necessary state for computing which expected tests were not
/// seen during a rerun.
#[derive(Clone, Debug)]
enum DispatcherRerunContext {
    /// This is an initial run, not a rerun. No tracking needed.
    InitialRun,
    /// This is a rerun of a previous run.
    ///
    /// Contains the set of tests expected to run. As tests are seen, they are
    /// removed from this set. At the end of the run, any remaining tests are
    /// reported as "not seen".
    Rerun(BTreeSet<OwnedTestInstanceId>),
}

impl DispatcherRerunContext {
    fn new(expected_outstanding: Option<BTreeSet<OwnedTestInstanceId>>) -> Self {
        match expected_outstanding {
            Some(expected) => Self::Rerun(expected),
            None => Self::InitialRun,
        }
    }

    /// Marks a test as seen during this run.
    fn mark_seen(&mut self, id: TestInstanceId<'_>) {
        if let Self::Rerun(expected) = self {
            expected.remove(&id as &dyn TestInstanceIdKey);
        }
    }

    /// Returns tests not seen, if this is a rerun and some tests were not seen.
    ///
    /// Returns `None` if this is an initial run.
    fn into_tests_not_seen(self) -> Option<TestsNotSeen> {
        let Self::Rerun(not_seen) = self else {
            return None;
        };

        let total_not_seen = not_seen.len();
        const MAX_DISPLAY: usize = 8;
        let sample: Vec<_> = not_seen.into_iter().take(MAX_DISPLAY).collect();
        Some(TestsNotSeen {
            not_seen: sample,
            total_not_seen,
        })
    }
}

impl DispatcherStressContext {
    fn new(condition: Option<StressCondition>) -> Self {
        if let Some(condition) = condition {
            Self::Stress {
                condition,
                sub_stopwatch: crate::time::stopwatch(),
                completed: 0,
                failed: 0,
                cancelled: false,
            }
        } else {
            Self::None
        }
    }

    fn condition(&self) -> Option<StressCondition> {
        match self {
            Self::None => None,
            Self::Stress { condition, .. } => Some(condition.clone()),
        }
    }

    fn progress(&self, total_elapsed: Duration) -> Option<StressProgress> {
        match self {
            Self::None => None,
            Self::Stress {
                condition,
                sub_stopwatch: _,
                completed,
                failed: _,
                cancelled: _,
            } => match condition {
                StressCondition::Count(total) => Some(StressProgress::Count {
                    total: *total,
                    elapsed: total_elapsed,
                    completed: *completed,
                }),
                StressCondition::Duration(total) => Some(StressProgress::Time {
                    total: *total,
                    elapsed: total_elapsed,
                    completed: *completed,
                }),
            },
        }
    }

    #[inline]
    fn stress_index(&self) -> Option<StressIndex> {
        match self {
            Self::None => None,
            Self::Stress {
                condition,
                completed,
                ..
            } => {
                // The index starts from 0 so it is the same as the number of
                // completed runs.
                let current = *completed;
                let total = match condition {
                    StressCondition::Count(StressCount::Count { count }) => Some(*count),
                    StressCondition::Count(StressCount::Infinite)
                    | StressCondition::Duration(_) => None,
                };
                Some(StressIndex { current, total })
            }
        }
    }

    fn mark_completed(&mut self, summary: FinalRunStats) -> Duration {
        match self {
            Self::None => {
                panic!("mark_completed called in a non-stress test context");
            }
            Self::Stress {
                condition: _,
                sub_stopwatch,
                completed,
                failed,
                cancelled,
            } => {
                *completed += 1;
                match summary {
                    FinalRunStats::Success => {}
                    FinalRunStats::NoTestsRun => {
                        // TODO: We should figure out whether to terminate the
                        // test run based on this.
                    }
                    FinalRunStats::Failed { .. } => {
                        *failed += 1;
                    }
                    FinalRunStats::Cancelled { .. } => {
                        // In this case, we don't add to the failed count. The
                        // displayer will take care of displaying the
                        // cancellation message properly.
                        *cancelled = true;
                    }
                }
                let duration = sub_stopwatch.snapshot().active;
                *sub_stopwatch = crate::time::stopwatch();
                duration
            }
        }
    }

    fn run_stats(&self, last_final_stats: FinalRunStats) -> Option<StressRunStats> {
        match self {
            Self::None => None,
            Self::Stress {
                condition: _,
                sub_stopwatch: _,
                completed,
                failed,
                cancelled,
            } => {
                let mut success_count = completed.saturating_sub(*failed);
                // If the run is cancelled, there's one less success than we
                // thought above.
                if *cancelled {
                    success_count = success_count.saturating_sub(1);
                }
                Some(StressRunStats {
                    completed: self.stress_index().expect("we're in the Self::Stress case"),
                    success_count,
                    failed_count: *failed,
                    last_final_stats,
                })
            }
        }
    }

    #[cfg(unix)]
    fn pause_stopwatch(&mut self) {
        match self {
            Self::None => {}
            Self::Stress { sub_stopwatch, .. } => {
                sub_stopwatch.pause();
            }
        }
    }

    #[cfg(unix)]
    fn resume_stopwatch(&mut self) {
        match self {
            Self::None => {}
            Self::Stress { sub_stopwatch, .. } => {
                sub_stopwatch.resume();
            }
        }
    }
}

fn event_to_cancel_reason(event: ShutdownEvent) -> CancelReason {
    match event {
        ShutdownEvent::Signal(sig) => match sig {
            #[cfg(unix)]
            ShutdownSignalEvent::Hangup | ShutdownSignalEvent::Term | ShutdownSignalEvent::Quit => {
                CancelReason::Signal
            }
            ShutdownSignalEvent::Interrupt => CancelReason::Interrupt,
        },
        ShutdownEvent::TestFailureImmediate => CancelReason::TestFailureImmediate,
    }
}

#[derive(Clone, Debug)]
struct ContextSetupScript<'a> {
    id: ScriptId,
    // Store these details primarily for debugging.
    #[expect(dead_code)]
    config: &'a SetupScriptConfig,
    #[expect(dead_code)]
    index: usize,
    #[expect(dead_code)]
    total: usize,
    req_tx: UnboundedSender<RunUnitRequest<'a>>,
}

#[derive(Clone, Debug)]
struct ContextTestInstance<'a> {
    // Store the instance primarily for debugging.
    #[expect(dead_code)]
    instance: TestInstance<'a>,
    past_attempts: Vec<ExecuteStatus<ChildSingleOutput>>,
    req_tx: UnboundedSender<RunUnitRequest<'a>>,
}

impl ContextTestInstance<'_> {
    fn attempt_failed_will_retry(&mut self, run_status: ExecuteStatus<ChildSingleOutput>) {
        self.past_attempts.push(run_status);
    }

    fn finish(
        self,
        last_run_status: ExecuteStatus<ChildSingleOutput>,
    ) -> ExecutionStatuses<ChildSingleOutput> {
        let mut attempts = self.past_attempts;
        attempts.push(last_run_status);
        ExecutionStatuses::new(attempts)
    }
}

// Almost all events are executor events, which is much larger than the others,
// so it doesn't make sense to optimize for the rare signal and input events.
#[expect(clippy::large_enum_variant)]
#[derive(Debug)]
enum InternalEvent<'a> {
    Tick,
    Executor(ExecutorEvent<'a>),
    Signal(SignalEvent),
    Input(InputEvent),
    ReportCancel,
    GlobalTimeout,
}

/// The return result of `handle_event`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "this enum should not be dropped on the floor"]
enum HandleEventResponse {
    /// Stop or continue the run.
    #[cfg_attr(not(unix), expect(dead_code))]
    JobControl(JobControlEvent),

    /// Request information from running units.
    Info(InfoEvent),

    /// Cancel the run.
    Cancel(CancelEvent),

    /// No response.
    ///
    /// We use `None` here rather than `Option` because we've found that
    /// `Option` enables using `?`, which can lead to incorrect results.
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InfoEvent {
    Signal(SignalInfoEvent),
    Input,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancelEvent {
    Report,
    TestFailure,
    GlobalTimeout,
    Signal(ShutdownRequest),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn begin_cancel_report_signal_interrupt() {
        // TODO: also test TestFinished and SetupScriptFinished events.
        let events = Mutex::new(Vec::new());
        let mut cx = DispatcherContext::new(
            |event| match event {
                ReporterEvent::Test(event) => {
                    events.lock().unwrap().push(event);
                }
                ReporterEvent::Tick => {
                    // Ignore tick events here.
                }
            },
            ReportUuid::new_v4(),
            "default",
            vec![],
            0,
            MaxFail::All,
            crate::time::far_future_duration(),
            None, // stress_condition
            None, // expected_outstanding
        );

        cx.disable_signal_3_times_panic = true;

        // Begin cancellation with a report error.
        let response = cx.handle_event(InternalEvent::ReportCancel);
        assert_eq!(
            response,
            HandleEventResponse::Cancel(CancelEvent::Report),
            "expected report"
        );
        {
            let mut events = events.lock().unwrap();
            assert_eq!(events.len(), 1, "expected 1 event");
            let event = events.pop().unwrap();
            let TestEventKind::RunBeginCancel {
                setup_scripts_running,
                current_stats,
                running,
            } = event.kind
            else {
                panic!("expected RunBeginCancel event, found {:?}", event.kind);
            };
            assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
            assert_eq!(running, 0, "expected 0 tests running");
            assert_eq!(
                current_stats.cancel_reason,
                Some(CancelReason::ReportError),
                "expected report error"
            );
        }

        // Send another report error, ensuring it's ignored.
        let response = cx.handle_event(InternalEvent::ReportCancel);
        assert_noop(response, &events);

        // Save a copy before TestFailureImmediate for later tests.
        let cx_before_test_failure = cx.clone();

        // Test TestFailureImmediate after ReportCancel - should upgrade.
        let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
            ShutdownEvent::TestFailureImmediate,
        )));
        assert_eq!(
            response,
            HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                ShutdownEvent::TestFailureImmediate
            ))),
            "expected TestFailureImmediate"
        );
        {
            let mut events = events.lock().unwrap();
            assert_eq!(events.len(), 1, "expected 1 event");
            let event = events.pop().unwrap();
            let TestEventKind::RunBeginCancel {
                setup_scripts_running,
                current_stats,
                running,
            } = event.kind
            else {
                panic!("expected RunBeginCancel event, found {:?}", event.kind);
            };
            assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
            assert_eq!(running, 0, "expected 0 tests running");
            assert_eq!(
                current_stats.cancel_reason,
                Some(CancelReason::TestFailureImmediate),
                "expected test failure immediate"
            );
        }

        // Send another TestFailureImmediate, ensuring it's ignored (no escalation like signals).
        let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
            ShutdownEvent::TestFailureImmediate,
        )));
        assert_noop(response, &events);

        // Send a report error after TestFailureImmediate, ensuring it's ignored.
        let response = cx.handle_event(InternalEvent::ReportCancel);
        assert_noop(response, &events);

        // The rules:
        // * Any one signal will cause that signal.
        // * Any two signals received will cause a SIGKILL.
        // * After a signal is received, any less-important cancel-worthy events
        //   are ignored.
        // * TestFailureImmediate acts like a signal but doesn't escalate on repetition.
        //
        // Interestingly, this state machine appears to function on Windows too
        // (though of course the only variant is an Interrupt so this only runs
        // one iteration.) Should it be different? No compelling reason to be
        // yet.
        for sig1 in ShutdownSignalEvent::ALL_VARIANTS {
            for sig2 in ShutdownSignalEvent::ALL_VARIANTS {
                eprintln!("** testing {sig1:?} -> {sig2:?}");
                // Separate test for each signal to avoid mixing up state.
                let mut cx = cx.clone();

                // First signal.
                let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                    ShutdownEvent::Signal(*sig1),
                )));
                assert_eq!(
                    response,
                    HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                        ShutdownEvent::Signal(*sig1)
                    ))),
                    "expected Once"
                );
                {
                    let mut events = events.lock().unwrap();
                    assert_eq!(events.len(), 1, "expected 1 event");
                    let event = events.pop().unwrap();
                    let TestEventKind::RunBeginCancel {
                        setup_scripts_running,
                        current_stats,
                        running,
                    } = event.kind
                    else {
                        panic!("expected RunBeginCancel event, found {:?}", event.kind);
                    };
                    assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
                    assert_eq!(running, 0, "expected 0 tests running");
                    assert_eq!(
                        current_stats.cancel_reason,
                        Some(event_to_cancel_reason(ShutdownEvent::Signal(*sig1))),
                        "expected signal"
                    );
                }

                // Another report error, ensuring it's ignored.
                let response = cx.handle_event(InternalEvent::ReportCancel);
                assert_noop(response, &events);

                // Second signal.
                let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                    ShutdownEvent::Signal(*sig2),
                )));
                assert_eq!(
                    response,
                    HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Twice)),
                    "expected kill"
                );
                {
                    let mut events = events.lock().unwrap();
                    assert_eq!(events.len(), 1, "expected 1 events");
                    let event = events.pop().unwrap();
                    let TestEventKind::RunBeginKill {
                        setup_scripts_running,
                        current_stats,
                        running,
                    } = event.kind
                    else {
                        panic!("expected RunBeginKill event, found {:?}", event.kind);
                    };
                    assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
                    assert_eq!(running, 0, "expected 0 tests running");
                    assert_eq!(
                        current_stats.cancel_reason,
                        Some(CancelReason::SecondSignal),
                        "expected second signal"
                    );
                }

                // Another report error, ensuring it's ignored.
                let response = cx.handle_event(InternalEvent::ReportCancel);
                assert_noop(response, &events);

                // TestFailureImmediate after signal should be ignored (signal is more severe).
                let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                    ShutdownEvent::TestFailureImmediate,
                )));
                assert_noop(response, &events);
            }
        }

        // Test that signals upgrade from TestFailureImmediate.
        for sig in ShutdownSignalEvent::ALL_VARIANTS {
            eprintln!("** testing TestFailureImmediate -> {sig:?}");
            // Separate test for each signal to avoid mixing up state.
            // Clone from before TestFailureImmediate was sent.
            let mut cx = cx_before_test_failure.clone();

            // First, send TestFailureImmediate.
            let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                ShutdownEvent::TestFailureImmediate,
            )));
            assert_eq!(
                response,
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                    ShutdownEvent::TestFailureImmediate
                ))),
                "expected TestFailureImmediate"
            );
            {
                let mut events = events.lock().unwrap();
                assert_eq!(events.len(), 1, "expected 1 event");
                let event = events.pop().unwrap();
                let TestEventKind::RunBeginCancel {
                    setup_scripts_running,
                    current_stats,
                    running,
                } = event.kind
                else {
                    panic!("expected RunBeginCancel event, found {:?}", event.kind);
                };
                assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
                assert_eq!(running, 0, "expected 0 tests running");
                assert_eq!(
                    current_stats.cancel_reason,
                    Some(CancelReason::TestFailureImmediate),
                    "expected test failure immediate"
                );
            }

            // Now send a signal - should upgrade.
            let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                ShutdownEvent::Signal(*sig),
            )));
            assert_eq!(
                response,
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                    ShutdownEvent::Signal(*sig)
                ))),
                "expected signal upgrade"
            );
            {
                let mut events = events.lock().unwrap();
                assert_eq!(events.len(), 1, "expected 1 event");
                let event = events.pop().unwrap();
                let TestEventKind::RunBeginCancel {
                    setup_scripts_running,
                    current_stats,
                    running,
                } = event.kind
                else {
                    panic!("expected RunBeginCancel event, found {:?}", event.kind);
                };
                assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
                assert_eq!(running, 0, "expected 0 tests running");
                assert_eq!(
                    current_stats.cancel_reason,
                    Some(event_to_cancel_reason(ShutdownEvent::Signal(*sig))),
                    "expected signal cancel reason"
                );
            }

            // A second signal should cause a kill.
            let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                ShutdownEvent::Signal(*sig),
            )));
            assert_eq!(
                response,
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Twice)),
                "expected kill"
            );
            {
                let mut events = events.lock().unwrap();
                assert_eq!(events.len(), 1, "expected 1 event");
                let event = events.pop().unwrap();
                let TestEventKind::RunBeginKill {
                    setup_scripts_running,
                    current_stats,
                    running,
                } = event.kind
                else {
                    panic!("expected RunBeginKill event, found {:?}", event.kind);
                };
                assert_eq!(setup_scripts_running, 0, "expected 0 setup scripts running");
                assert_eq!(running, 0, "expected 0 tests running");
                assert_eq!(
                    current_stats.cancel_reason,
                    Some(CancelReason::SecondSignal),
                    "expected second signal"
                );
            }
        }
    }

    #[track_caller]
    fn assert_noop(response: HandleEventResponse, events: &Mutex<Vec<Box<TestEvent<'_>>>>) {
        assert_eq!(response, HandleEventResponse::None, "expected no response");
        assert_eq!(events.lock().unwrap().len(), 0, "expected no new events");
    }
}
