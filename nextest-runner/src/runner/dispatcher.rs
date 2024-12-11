// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The controller for the test runner.
//!
//! This module interfaces with the external world and the test executor. It
//! receives events from the executor and from other inputs (e.g. signal and
//! input handling), and sends events to the reporter.

use super::{RunUnitRequest, ShutdownRequest};
use crate::{
    config::{MaxFail, ScriptConfig, ScriptId},
    input::{InputEvent, InputHandler},
    list::{TestInstance, TestInstanceId, TestList},
    reporter::events::{
        CancelReason, ExecuteStatus, ExecutionStatuses, InfoResponse, RunStats, TestEvent,
        TestEventKind,
    },
    runner::{ExecutorEvent, RunUnitQuery, SignalRequest},
    signal::{JobControlEvent, ShutdownEvent, SignalEvent, SignalHandler, SignalInfoEvent},
    time::StopwatchStart,
};
use chrono::Local;
use debug_ignore::DebugIgnore;
use quick_junit::ReportUuid;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::{
    broadcast,
    mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    oneshot,
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
    running_setup_script: Option<ContextSetupScript<'a>>,
    running_tests: BTreeMap<TestInstanceId<'a>, ContextTestInstance<'a>>,
    cancel_state: Option<CancelReason>,
    signal_count: Option<SignalCount>,
    #[cfg(test)]
    disable_signal_3_times_panic: bool,
}

impl<'a, F> DispatcherContext<'a, F>
where
    F: FnMut(TestEvent<'a>) + Send,
{
    pub(super) fn new(
        callback: F,
        run_id: ReportUuid,
        profile_name: &str,
        cli_args: Vec<String>,
        initial_run_count: usize,
        max_fail: MaxFail,
    ) -> Self {
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
            running_setup_script: None,
            running_tests: BTreeMap::new(),
            cancel_state: None,
            signal_count: None,
            #[cfg(test)]
            disable_signal_3_times_panic: false,
        }
    }

    /// Runs the dispatcher to completion, until `resp_rx` is closed.
    ///
    /// `resp_rx` is the main communication channel between the dispatcher and
    /// the executor. It receives events, but some of those events also include
    /// senders for the dispatcher to communicate back to the executor.
    ///
    /// This is expected to be spawned as a task via [`async_scoped`].
    pub(super) async fn run(
        &mut self,
        mut executor_rx: UnboundedReceiver<ExecutorEvent<'a>>,
        signal_handler: &mut SignalHandler,
        input_handler: &mut InputHandler,
        report_cancel_rx: oneshot::Receiver<()>,
        cancelled_ref: &AtomicBool,
        cancellation_sender: broadcast::Sender<()>,
    ) {
        let mut report_cancel_rx = std::pin::pin!(report_cancel_rx);

        let mut signals_done = false;
        let mut inputs_done = false;
        let mut report_cancel_rx_done = false;

        loop {
            let internal_event = tokio::select! {
                internal_event = executor_rx.recv() => {
                    match internal_event {
                        Some(event) => InternalEvent::Executor(event),
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
                internal_event = input_handler.recv(), if !inputs_done => {
                    match internal_event {
                        Some(event) => InternalEvent::Input(event),
                        None => {
                            inputs_done = true;
                            continue;
                        }
                    }
                }
                res = &mut report_cancel_rx, if !report_cancel_rx_done => {
                    report_cancel_rx_done = true;
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

                    // Now stop nextest itself.
                    super::os::raise_stop();
                }
                #[cfg(unix)]
                HandleEventResponse::JobControl(JobControlEvent::Continue) => {
                    // Nextest has been resumed. Resume all the tests as well.
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
                    // A cancellation notice was received. Note the ordering here:
                    // cancelled_ref is set *before* notifications are broadcast. This
                    // prevents race conditions.
                    cancelled_ref.store(true, Ordering::Release);
                    let _ = cancellation_sender.send(());
                    match cancel {
                        // Some of the branches here don't do anything, but are specified
                        // for readability.
                        CancelEvent::Report => {
                            // An error was produced by the reporter, and cancellation has
                            // begun.
                        }
                        CancelEvent::TestFailure => {
                            // A test failure has caused cancellation to begin. Nothing to
                            // do here.
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

    pub(super) fn run_started(&mut self, test_list: &'a TestList) {
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
    fn callback_none_response(&mut self, kind: TestEventKind<'a>) -> HandleEventResponse {
        self.basic_callback(kind);
        HandleEventResponse::None
    }

    fn handle_event(&mut self, event: InternalEvent<'a>) -> HandleEventResponse {
        match event {
            InternalEvent::Executor(ExecutorEvent::SetupScriptStarted {
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
                        return HandleEventResponse::None;
                    }
                }
                self.new_setup_script(script_id.clone(), config, index, total, req_tx);
                self.callback_none_response(TestEventKind::SetupScriptStarted {
                    index,
                    total,
                    script_id,
                    command: config.program(),
                    args: config.args(),
                    no_capture: config.no_capture(),
                })
            }
            InternalEvent::Executor(ExecutorEvent::SetupScriptSlow {
                script_id,
                config,
                elapsed,
                will_terminate,
            }) => self.callback_none_response(TestEventKind::SetupScriptSlow {
                script_id,
                command: config.program(),
                args: config.args(),
                elapsed,
                will_terminate: will_terminate.is_some(),
            }),
            InternalEvent::Executor(ExecutorEvent::SetupScriptFinished {
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

                self.basic_callback(TestEventKind::SetupScriptFinished {
                    index,
                    total,
                    script_id,
                    command: config.program(),
                    args: config.args(),
                    no_capture: config.no_capture(),
                    run_status: status,
                });

                if fail_cancel {
                    self.begin_cancel(CancelReason::SetupScriptFailure, CancelEvent::TestFailure)
                } else {
                    HandleEventResponse::None
                }
            }
            InternalEvent::Executor(ExecutorEvent::Started {
                test_instance,
                req_rx_tx,
            }) => {
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
                    test_instance,
                    current_stats: self.run_stats,
                    running: self.running_tests.len(),
                    cancel_state: self.cancel_state,
                })
            }
            InternalEvent::Executor(ExecutorEvent::Slow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            }) => self.callback_none_response(TestEventKind::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate: will_terminate.is_some(),
            }),
            InternalEvent::Executor(ExecutorEvent::AttemptFailedWillRetry {
                test_instance,
                failure_output,
                run_status,
                delay_before_next_attempt,
            }) => {
                let instance = self.existing_test(test_instance.id());
                instance.attempt_failed_will_retry(run_status.clone());
                self.callback_none_response(TestEventKind::TestAttemptFailedWillRetry {
                    test_instance,
                    failure_output,
                    run_status,
                    delay_before_next_attempt,
                })
            }
            InternalEvent::Executor(ExecutorEvent::RetryStarted {
                test_instance,
                retry_data,
            }) => self.callback_none_response(TestEventKind::TestRetryStarted {
                test_instance,
                retry_data,
            }),
            InternalEvent::Executor(ExecutorEvent::Finished {
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
                let fail_cancel = self.max_fail.is_exceeded(self.run_stats.failed_count());

                self.basic_callback(TestEventKind::TestFinished {
                    test_instance,
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats: self.run_stats,
                    running: self.running(),
                    cancel_state: self.cancel_state,
                });

                if fail_cancel {
                    // A test failed: start cancellation if required.
                    self.begin_cancel(CancelReason::TestFailure, CancelEvent::TestFailure)
                } else {
                    HandleEventResponse::None
                }
            }
            InternalEvent::Executor(ExecutorEvent::Skipped {
                test_instance,
                reason,
            }) => {
                self.run_stats.skipped += 1;
                self.callback_none_response(TestEventKind::TestSkipped {
                    test_instance,
                    reason,
                })
            }
            InternalEvent::Signal(event) => self.handle_signal_event(event),
            InternalEvent::Input(InputEvent::Info) => {
                // Print current statistics.
                HandleEventResponse::Info(InfoEvent::Input)
            }
            InternalEvent::ReportCancel => {
                self.begin_cancel(CancelReason::ReportError, CancelEvent::Report)
            }
        }
    }

    fn new_setup_script(
        &mut self,
        id: ScriptId,
        config: &'a ScriptConfig,
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
                let signal_count = self.increment_signal_count();
                let req = signal_count.to_request(event);

                let cancel_reason = match event {
                    #[cfg(unix)]
                    ShutdownEvent::Hangup | ShutdownEvent::Term | ShutdownEvent::Quit => {
                        CancelReason::Signal
                    }
                    ShutdownEvent::Interrupt => CancelReason::Interrupt,
                };

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
        // TODO: combine reason and event?
        if self.cancel_state < Some(reason) {
            self.cancel_state = Some(reason);
            self.basic_callback(TestEventKind::RunBeginCancel {
                setup_scripts_running: self.setup_scripts_running(),
                running: self.running(),
                reason,
            });
            HandleEventResponse::Cancel(event)
        } else {
            HandleEventResponse::None
        }
    }

    pub(super) fn run_finished(&mut self) {
        let stopwatch_end = self.stopwatch.snapshot();
        self.basic_callback(TestEventKind::RunFinished {
            start_time: stopwatch_end.start_time.fixed_offset(),
            run_id: self.run_id,
            elapsed: stopwatch_end.active,
            run_stats: self.run_stats,
        })
    }

    pub(super) fn run_stats(&self) -> RunStats {
        self.run_stats
    }
}

#[derive(Clone, Debug)]
struct ContextSetupScript<'a> {
    id: ScriptId,
    // Store these details primarily for debugging.
    #[expect(dead_code)]
    config: &'a ScriptConfig,
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
    past_attempts: Vec<ExecuteStatus>,
    req_tx: UnboundedSender<RunUnitRequest<'a>>,
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
    Executor(ExecutorEvent<'a>),
    Signal(SignalEvent),
    Input(InputEvent),
    ReportCancel,
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
            |event| {
                events.lock().unwrap().push(event);
            },
            ReportUuid::new_v4(),
            "default",
            vec![],
            0,
            MaxFail::All,
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
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1, "expected 1 event");
            let TestEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } = &events[0].kind
            else {
                panic!("expected RunBeginCancel event, found {:?}", events[0].kind);
            };
            assert_eq!(
                *setup_scripts_running, 0,
                "expected 0 setup scripts running"
            );
            assert_eq!(*running, 0, "expected 0 tests running");
            assert_eq!(*reason, CancelReason::ReportError, "expected report error");
        }

        // Send another report error, ensuring it's ignored.
        let response = cx.handle_event(InternalEvent::ReportCancel);
        assert_noop(response, &events, 1);

        // Cancel with a signal.
        #[cfg(unix)]
        let signal_event_count = {
            let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                ShutdownEvent::Hangup,
            )));
            assert_eq!(
                response,
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                    ShutdownEvent::Hangup
                ))),
                "expected cancellation"
            );
            {
                let events = events.lock().unwrap();
                assert_eq!(events.len(), 2, "expected 2 events");
                let TestEventKind::RunBeginCancel {
                    setup_scripts_running,
                    running,
                    reason,
                } = &events[1].kind
                else {
                    panic!("expected RunBeginCancel event, found {:?}", events[1].kind);
                };
                assert_eq!(
                    *setup_scripts_running, 0,
                    "expected 0 setup scripts running"
                );
                assert_eq!(*running, 0, "expected 0 tests running");
                assert_eq!(*reason, CancelReason::Signal, "expected signal");
            }

            // Send another signal, ensuring it's ignored.
            let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
                ShutdownEvent::Hangup,
            )));
            assert_noop(response, &events, 2);

            // Send a report error, ensuring it's ignored.
            let response = cx.handle_event(InternalEvent::ReportCancel);
            assert_noop(response, &events, 2);

            1
        };
        #[cfg(not(unix))]
        let signal_event_count = 0;

        // Cancel with an interrupt.
        let response = cx.handle_event(InternalEvent::Signal(SignalEvent::Shutdown(
            ShutdownEvent::Interrupt,
        )));
        assert_eq!(
            response,
            if signal_event_count == 0 {
                // On Windows, this is the first signal.
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Once(
                    ShutdownEvent::Interrupt,
                )))
            } else {
                // On Unix, this is the second signal.
                HandleEventResponse::Cancel(CancelEvent::Signal(ShutdownRequest::Twice))
            },
            "expected cancellation"
        );
        {
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2 + signal_event_count, "expected event count");
            let TestEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } = &events[1 + signal_event_count].kind
            else {
                panic!(
                    "expected RunBeginCancel event, found {:?}",
                    events[1 + signal_event_count].kind
                );
            };
            assert_eq!(
                *setup_scripts_running, 0,
                "expected 0 setup scripts running"
            );
            assert_eq!(*running, 0, "expected 0 tests running");
            assert_eq!(*reason, CancelReason::Interrupt, "expected interrupt");
        }
    }

    #[track_caller]
    fn assert_noop(
        response: HandleEventResponse,
        events: &Mutex<Vec<TestEvent<'_>>>,
        event_count: usize,
    ) {
        assert_eq!(response, HandleEventResponse::None, "expected no response");
        let events = events.lock().unwrap();
        assert_eq!(events.len(), event_count, "expected no new events");
    }
}
