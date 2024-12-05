// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ShutdownRequest, TerminateMode, UnitContext};
use crate::{
    errors::ConfigureHandleInheritanceError,
    reporter::events::{
        UnitState, UnitTerminateMethod, UnitTerminateReason, UnitTerminateSignal,
        UnitTerminatingState,
    },
    runner::{RunUnitQuery, RunUnitRequest, SignalRequest},
    signal::{JobControlEvent, ShutdownEvent},
    test_command::ChildAccumulator,
    time::StopwatchStart,
};
use libc::{SIGCONT, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGSTOP, SIGTERM, SIGTSTP};
use std::{convert::Infallible, os::unix::process::CommandExt, time::Duration};
use tokio::{process::Child, sync::mpsc::UnboundedReceiver};

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
//
// TODO: it would be nice to find a way to gather data like job (only on
// Windows) or grace_period (only relevant on Unix) together.
#[expect(clippy::too_many_arguments)]
pub(super) async fn terminate_child<'a>(
    cx: &UnitContext<'a, '_>,
    child: &mut Child,
    child_acc: &mut ChildAccumulator,
    mode: TerminateMode,
    stopwatch: &mut StopwatchStart,
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    _job: Option<&Job>,
    grace_period: Duration,
) {
    if let Some(pid) = child.id() {
        let pid_i32 = pid as i32;
        let (term_reason, term_method) = to_terminate_reason_and_method(&mode, grace_period);

        // This is infallible in regular mode and fallible with cfg(test).
        #[allow(clippy::infallible_destructuring_match)]
        let term_signal = match term_method {
            UnitTerminateMethod::Signal(term_signal) => term_signal,
            #[cfg(test)]
            UnitTerminateMethod::Fake => {
                unreachable!("fake method is only used for reporter tests")
            }
        };

        unsafe {
            // We set up a process group while starting the test -- now send a signal to that
            // group.
            libc::kill(-pid_i32, term_signal.signal())
        };

        if term_signal == UnitTerminateSignal::Kill {
            // SIGKILL guarantees the process group is dead.
            return;
        }

        let mut sleep = std::pin::pin!(crate::time::pausable_sleep(grace_period));
        let mut waiting_stopwatch = crate::time::stopwatch();

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
                            waiting_stopwatch.pause();

                            job_control_child(child, JobControlEvent::Stop);
                            let _ = sender.send(());
                        }
                        RunUnitRequest::Signal(SignalRequest::Continue) => {
                            // Possible to receive a Continue at the beginning of execution.
                            if !sleep.is_paused() {
                                stopwatch.resume();
                                sleep.as_mut().resume();
                                waiting_stopwatch.resume();
                            }
                            job_control_child(child, JobControlEvent::Continue);
                        }
                        RunUnitRequest::Signal(SignalRequest::Shutdown(_)) => {
                            // Receiving a shutdown signal while in this state always means kill
                            // immediately.
                            unsafe {
                                // Send SIGKILL to the entire process group.
                                libc::kill(-pid_i32, SIGKILL);
                            }
                            break;
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
                    // The process didn't exit -- need to do a hard shutdown.
                    unsafe {
                        // Send SIGKILL to the entire process group.
                        libc::kill(-pid_i32, SIGKILL);
                    }
                    break;
                }
            }
        }
    }
}

fn to_terminate_reason_and_method(
    mode: &TerminateMode,
    grace_period: Duration,
) -> (UnitTerminateReason, UnitTerminateMethod) {
    match mode {
        TerminateMode::Timeout => (
            UnitTerminateReason::Timeout,
            timeout_terminate_method(grace_period),
        ),
        TerminateMode::Signal(req) => (
            UnitTerminateReason::Signal,
            shutdown_terminate_method(*req, grace_period),
        ),
    }
}

fn timeout_terminate_method(grace_period: Duration) -> UnitTerminateMethod {
    if grace_period.is_zero() {
        UnitTerminateMethod::Signal(UnitTerminateSignal::Kill)
    } else {
        UnitTerminateMethod::Signal(UnitTerminateSignal::Term)
    }
}

fn shutdown_terminate_method(req: ShutdownRequest, grace_period: Duration) -> UnitTerminateMethod {
    if grace_period.is_zero() {
        return UnitTerminateMethod::Signal(UnitTerminateSignal::Kill);
    }

    match req {
        ShutdownRequest::Once(ShutdownEvent::Hangup) => {
            UnitTerminateMethod::Signal(UnitTerminateSignal::Hangup)
        }
        ShutdownRequest::Once(ShutdownEvent::Term) => {
            UnitTerminateMethod::Signal(UnitTerminateSignal::Term)
        }
        ShutdownRequest::Once(ShutdownEvent::Quit) => {
            UnitTerminateMethod::Signal(UnitTerminateSignal::Quit)
        }
        ShutdownRequest::Once(ShutdownEvent::Interrupt) => {
            UnitTerminateMethod::Signal(UnitTerminateSignal::Interrupt)
        }
        ShutdownRequest::Twice => UnitTerminateMethod::Signal(UnitTerminateSignal::Kill),
    }
}

impl UnitTerminateSignal {
    fn signal(self) -> libc::c_int {
        match self {
            UnitTerminateSignal::Interrupt => SIGINT,
            UnitTerminateSignal::Term => SIGTERM,
            UnitTerminateSignal::Hangup => SIGHUP,
            UnitTerminateSignal::Quit => SIGQUIT,
            UnitTerminateSignal::Kill => SIGKILL,
        }
    }
}
