// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Internal events used between the runner components.
//!
//! These events often mirror those in [`crate::reporter::events`], but are used
//! within the runner. They'll often carry additional information that the
//! reporter doesn't need to know about.

use super::{RetryData, SetupScriptPacket, TestPacket};
use crate::{
    config::{ScriptConfig, ScriptId, SetupScriptEnvMap},
    input::InputEvent,
    list::TestInstance,
    reporter::{
        events::{
            ExecuteStatus, ExecutionResult, InfoResponse, SetupScriptExecuteStatus, UnitState,
        },
        TestOutputDisplay,
    },
    signal::{ShutdownEvent, SignalEvent},
    test_output::ChildExecutionOutput,
    time::StopwatchSnapshot,
};
use nextest_metadata::MismatchReason;
use std::time::Duration;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};

/// An internal event.
///
/// These events are sent by the executor (the part that actually runs
/// executables) to the dispatcher (the part of the runner that coordinates with
/// the external world).
#[derive(Debug)]
pub(super) enum InternalEvent<'a> {
    Test(InternalTestEvent<'a>),
    Signal(SignalEvent),
    Input(InputEvent),
    ReportCancel,
}

/// An internal version of `TestEvent`.
#[derive(Debug)]
pub(super) enum InternalTestEvent<'a> {
    SetupScriptStarted {
        script_id: ScriptId,
        config: &'a ScriptConfig,
        index: usize,
        total: usize,
        // See the note in the `Started` variant.
        req_rx_tx: oneshot::Sender<UnboundedReceiver<RunUnitRequest<'a>>>,
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
        req_rx_tx: oneshot::Sender<UnboundedReceiver<RunUnitRequest<'a>>>,
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
pub(super) enum InternalCancel {
    Report,
    TestFailure,
    Signal(ShutdownRequest),
}

#[derive(Clone, Copy)]
pub(super) enum UnitExecuteStatus<'a, 'test> {
    Test(&'test InternalExecuteStatus<'a, 'test>),
    SetupScript(&'test InternalSetupScriptExecuteStatus<'a>),
}

impl<'a> UnitExecuteStatus<'a, '_> {
    pub(super) fn info_response(&self) -> InfoResponse<'a> {
        match self {
            Self::Test(status) => status.test.info_response(
                UnitState::Exited {
                    result: status.result,
                    time_taken: status.stopwatch_end.active,
                    slow_after: status.slow_after,
                },
                status.output.clone(),
            ),
            Self::SetupScript(status) => status.script.info_response(
                UnitState::Exited {
                    result: status.result,
                    time_taken: status.stopwatch_end.active,
                    slow_after: status.slow_after,
                },
                status.output.clone(),
            ),
        }
    }
}

pub(super) struct InternalExecuteStatus<'a, 'test> {
    pub(super) test: TestPacket<'a, 'test>,
    pub(super) slow_after: Option<Duration>,
    pub(super) output: ChildExecutionOutput,
    pub(super) result: ExecutionResult,
    pub(super) stopwatch_end: StopwatchSnapshot,
    pub(super) delay_before_start: Duration,
}

impl InternalExecuteStatus<'_, '_> {
    pub(super) fn into_external(self) -> ExecuteStatus {
        ExecuteStatus {
            retry_data: self.test.retry_data(),
            output: self.output,
            result: self.result,
            start_time: self.stopwatch_end.start_time.fixed_offset(),
            time_taken: self.stopwatch_end.active,
            is_slow: self.slow_after.is_some(),
            delay_before_start: self.delay_before_start,
        }
    }
}

pub(super) struct InternalSetupScriptExecuteStatus<'a> {
    pub(super) script: SetupScriptPacket<'a>,
    pub(super) slow_after: Option<Duration>,
    pub(super) output: ChildExecutionOutput,
    pub(super) result: ExecutionResult,
    pub(super) stopwatch_end: StopwatchSnapshot,
    pub(super) env_map: Option<SetupScriptEnvMap>,
}

impl InternalSetupScriptExecuteStatus<'_> {
    pub(super) fn into_external(self) -> (SetupScriptExecuteStatus, Option<SetupScriptEnvMap>) {
        let env_count = self.env_map.as_ref().map(|map| map.len());
        (
            SetupScriptExecuteStatus {
                output: self.output,
                result: self.result,
                start_time: self.stopwatch_end.start_time.fixed_offset(),
                time_taken: self.stopwatch_end.active,
                is_slow: self.slow_after.is_some(),
                env_count,
            },
            self.env_map,
        )
    }
}

/// Events sent from the dispatcher to individual unit execution tasks.
#[derive(Clone, Debug)]
pub(super) enum RunUnitRequest<'a> {
    Signal(SignalRequest),
    Query(RunUnitQuery<'a>),
}

impl<'a> RunUnitRequest<'a> {
    pub(super) fn drain(self, status: UnitExecuteStatus<'a, '_>) {
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

#[derive(Clone, Debug)]
pub(super) enum RunUnitQuery<'a> {
    GetInfo(UnboundedSender<InfoResponse<'a>>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TerminateMode {
    Timeout,
    Signal(ShutdownRequest),
}
