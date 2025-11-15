// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ConfigureHandleInheritanceError,
    reporter::events::{UnitState, UnitTerminateMethod, UnitTerminateReason, UnitTerminatingState},
    runner::{
        ChildPid, InternalTerminateReason, RunUnitQuery, RunUnitRequest, ShutdownRequest,
        SignalRequest, TerminateChildResult, UnitContext,
    },
    signal::{ShutdownEvent, ShutdownSignalEvent},
    test_command::ChildAccumulator,
    time::StopwatchStart,
};
use std::time::Duration;
use tokio::{process::Child, sync::mpsc::UnboundedReceiver};
pub(super) use win32job::Job;
use win32job::JobError;
use windows_sys::Win32::{
    Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        JobObjects::TerminateJobObject,
    },
};

pub(super) fn create_job() -> Result<Job, JobError> {
    Job::create_with_limit_info(win32job::ExtendedLimitInfo::new().limit_breakaway_ok())
}

pub(super) fn configure_handle_inheritance_impl(
    no_capture: bool,
) -> Result<(), ConfigureHandleInheritanceError> {
    unsafe fn set_handle_inherit(handle: u32, inherit: bool) -> std::io::Result<()> {
        // SAFETY: Win32 call, handle is assumed to be valid
        let handle = unsafe { GetStdHandle(handle) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let flags = if inherit { HANDLE_FLAG_INHERIT } else { 0 };
        // SAFETY: Win32 call, handle is assumed to be valid
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flags) } == 0 {
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

#[expect(clippy::too_many_arguments)]
pub(super) async fn terminate_child<'a>(
    cx: &UnitContext<'a>,
    child: &mut Child,
    child_acc: &mut ChildAccumulator,
    _child_pid: ChildPid,
    reason: InternalTerminateReason,
    stopwatch: &mut StopwatchStart,
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    job: Option<&Job>,
    grace_period: Duration,
) -> TerminateChildResult {
    let Some(pid) = child.id() else {
        return TerminateChildResult::Exited;
    };
    let (term_reason, term_method) = to_terminate_reason_and_method(&reason, grace_period);

    let child_exited = match term_method {
        UnitTerminateMethod::Wait => {
            // Unlike Unix, this doesn't need to be a pausable sleep -- we don't
            // have a notion of pausing nextest on Windows at the moment.
            let mut sleep = std::pin::pin!(tokio::time::sleep(grace_period));
            let waiting_stopwatch = crate::time::stopwatch();

            loop {
                tokio::select! {
                    () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
                    _ = child.wait() => {
                        // The process exited.
                        break true;
                    }
                    recv = req_rx.recv() => {
                        // The sender stays open longer than the whole loop, and
                        // the buffer is big enough for all messages ever sent
                        // through this channel, so a RecvError should never
                        // happen.
                        let req = recv.expect("a RecvError should never happen here");

                        match req {
                            RunUnitRequest::Signal(SignalRequest::Shutdown(_)) => {
                                // Receiving a shutdown signal while waiting for
                                // the process to exit always means kill
                                // immediately -- go to the next step.
                                break false;
                            }
                            RunUnitRequest::OtherCancel => {
                                // Ignore non-signal cancellation requests (most
                                // likely another test failed). Let the unit finish.
                            }
                            RunUnitRequest::Query(RunUnitQuery::GetInfo(sender)) => {
                                let waiting_snapshot = waiting_stopwatch.snapshot();
                                _ = sender.send(
                                    cx.info_response(
                                        UnitState::Terminating(UnitTerminatingState {
                                            pid,
                                            time_taken: stopwatch.snapshot().active,
                                            reason: term_reason,
                                            method: term_method,
                                            waiting_duration: waiting_snapshot.active,
                                            remaining: grace_period
                                                .checked_sub(waiting_snapshot.active)
                                                .unwrap_or_default(),
                                        }),
                                        child_acc.snapshot_in_progress(cx.packet().kind().waiting_on_message()),
                                    )
                                );
                            }
                        }
                    }
                    _ = &mut sleep => {
                        // The grace period has elapsed.
                        break false;
                    }
                }
            }
        }
        UnitTerminateMethod::JobObject => {
            // The process is killed by the job object.
            false
        }
        #[cfg(test)]
        UnitTerminateMethod::Fake => {
            unreachable!("fake method is only used for reporter tests");
        }
    };

    // In any case, always call TerminateJobObject to ensure other processes
    // spawned by the child are killed.
    if let Some(job) = job {
        let handle = job.handle();
        unsafe {
            // Ignore the error here -- it's likely due to the process exiting.
            // Note: 1 is the exit code returned by Windows.
            _ = TerminateJobObject(handle as _, 1);
        }
    }

    // Start killing the process directly for good measure.
    if child_exited {
        TerminateChildResult::Exited
    } else {
        let _ = child.start_kill();
        TerminateChildResult::Killed
    }
}

fn to_terminate_reason_and_method(
    reason: &InternalTerminateReason,
    grace_period: Duration,
) -> (UnitTerminateReason, UnitTerminateMethod) {
    match reason {
        InternalTerminateReason::Timeout => (
            UnitTerminateReason::Timeout,
            // The grace period is currently ignored for timeouts --
            // TerminateJobObject is immediately called.
            UnitTerminateMethod::JobObject,
        ),
        InternalTerminateReason::Signal(req) => (
            // The only signals we support on Windows are interrupts.
            UnitTerminateReason::Interrupt,
            shutdown_terminate_method(*req, grace_period),
        ),
    }
}

fn shutdown_terminate_method(req: ShutdownRequest, grace_period: Duration) -> UnitTerminateMethod {
    if grace_period.is_zero() {
        return UnitTerminateMethod::JobObject;
    }

    match req {
        // In case of interrupt events or test failure, wait for the grace period to elapse
        // before terminating the job. We're assuming that if nextest got an
        // interrupt, child processes did as well.
        ShutdownRequest::Once(ShutdownEvent::Signal(ShutdownSignalEvent::Interrupt))
        | ShutdownRequest::Once(ShutdownEvent::TestFailureImmediate) => UnitTerminateMethod::Wait,
        ShutdownRequest::Twice => UnitTerminateMethod::JobObject,
    }
}
