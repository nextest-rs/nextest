// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{
    ChildPid, InternalTerminateReason, ShutdownRequest, TerminateChildResult, UnitContext,
};
use crate::{
    config::elements::CpuPriorityLevel,
    double_spawn::DoubleSpawnInfo,
    errors::ConfigureHandleInheritanceError,
    reporter::events::{
        UnitState, UnitTerminateMethod, UnitTerminateReason, UnitTerminateSignal,
        UnitTerminatingState,
    },
    run_mode::NextestRunMode,
    runner::{Interceptor, RunUnitQuery, RunUnitRequest, SignalRequest},
    signal::{JobControlEvent, ShutdownEvent, ShutdownSignalEvent},
    test_command::{ChildAccumulator, CpuPriorityRequest, spawns_via_double_spawn},
    time::StopwatchStart,
};
use libc::{SIGCONT, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGSTOP, SIGTERM, SIGTSTP};
use std::{
    collections::BTreeMap, convert::Infallible, os::unix::process::CommandExt, time::Duration,
};
use tokio::{process::Child, runtime::Runtime, sync::mpsc::UnboundedReceiver};

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

/// Determines the manner in which CPU priority is applied.
pub(super) fn resolve_cpu_priority(
    priority: CpuPriorityLevel,
    interceptor: &Interceptor,
    double_spawn: &DoubleSpawnInfo,
) -> CpuPriorityRequest {
    if spawns_via_double_spawn(interceptor, double_spawn) {
        CpuPriorityRequest::DoubleSpawn(priority)
    } else {
        CpuPriorityRequest::PreExec(priority)
    }
}

/// Sets a `pre_exec` hook to set the CPU priority.
pub(super) fn set_cpu_priority_pre_exec(
    cmd: &mut std::process::Command,
    cpu_priority: Option<CpuPriorityRequest>,
) {
    let Some(nice) = cpu_priority.and_then(|req| req.pre_exec_nice()) else {
        return;
    };

    // TODO-RAINCLAUDE: SAFETY — the closure runs between fork and exec in a potentially multithreaded parent, so it must only do fork-safe work. try_set_nice is a bare setpriority syscall plus an errno read, with no locks or allocation (POSIX does not list setpriority as async-signal-safe, but on our target platforms it is a bare syscall), and the closure does nothing else.
    unsafe {
        cmd.pre_exec(move || {
            // TODO-RAINCLAUDE: failures such as EACCES are deliberately ignored here; the up-front probe warning covers them.
            let _ = crate::cpu_priority_probe::try_set_nice(nice);
            Ok(())
        });
    }
}

// TODO-RAINCLAUDE: probe whether the tallied CPU priority levels can be applied and warn once; delegates to the Unix probe module, keeping the call site free of cfg().
pub(super) fn maybe_warn_cpu_priority(
    level_counts: BTreeMap<CpuPriorityLevel, usize>,
    mode: NextestRunMode,
    double_spawn: &DoubleSpawnInfo,
    runtime: &Runtime,
) {
    crate::cpu_priority_probe::maybe_warn_cpu_priority(&level_counts, mode, double_spawn, runtime);
}

// TODO-RAINCLAUDE: no-op on Unix — assign_process_to_job is infallible here, so there is no assignment failure to surface. Present for a uniform interface with Windows, where the job object is the only vehicle for cpu-priority.
pub(super) fn warn_on_cpu_priority_assign_error(
    _cpu_priority: Option<CpuPriorityRequest>,
    _error: Infallible,
) {
}

#[derive(Debug)]
pub(super) struct Job(());

pub(super) fn create_job(_cpu_priority: Option<CpuPriorityRequest>) -> Result<Job, Infallible> {
    Ok(Job(()))
}

pub(super) fn assign_process_to_job(
    _child: &tokio::process::Child,
    _job: Option<&Job>,
) -> Result<(), Infallible> {
    Ok(())
}

pub(super) fn job_control_child(child: &Child, child_pid: ChildPid, event: JobControlEvent) {
    if child.id().is_some() {
        // Send the signal to the process or process group.
        let signal = match event {
            JobControlEvent::Stop => SIGTSTP,
            JobControlEvent::Continue => SIGCONT,
        };
        unsafe {
            libc::kill(child_pid.for_kill(), signal);
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
    cx: &UnitContext<'a>,
    child: &mut Child,
    child_acc: &mut ChildAccumulator,
    child_pid: ChildPid,
    reason: InternalTerminateReason,
    stopwatch: &mut StopwatchStart,
    req_rx: &mut UnboundedReceiver<RunUnitRequest<'a>>,
    _job: Option<&Job>,
    grace_period: Duration,
) -> TerminateChildResult {
    let Some(pid) = child.id() else {
        return TerminateChildResult::Exited;
    };

    let (term_reason, term_method) = to_terminate_reason_and_method(&reason, grace_period);

    // This is infallible in regular mode and fallible with cfg(test).
    #[cfg_attr(not(test), expect(clippy::infallible_destructuring_match))]
    let term_signal = match term_method {
        UnitTerminateMethod::Signal(term_signal) => term_signal,
        #[cfg(test)]
        UnitTerminateMethod::Fake => {
            unreachable!("fake method is only used for reporter tests")
        }
    };

    unsafe { libc::kill(child_pid.for_kill(), term_signal.signal()) };

    if term_signal == UnitTerminateSignal::Kill {
        // SIGKILL guarantees the process group is dead.
        return TerminateChildResult::Killed;
    }

    let mut sleep = std::pin::pin!(crate::time::pausable_sleep(grace_period));
    let mut waiting_stopwatch = crate::time::stopwatch();

    loop {
        tokio::select! {
            () = child_acc.fill_buf(), if !child_acc.fds.is_done() => {}
            _ = child.wait() => {
                // The process exited.
                break TerminateChildResult::Exited;
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

                        job_control_child(child, child_pid, JobControlEvent::Stop);
                        let _ = sender.send(());
                    }
                    RunUnitRequest::Signal(SignalRequest::Continue) => {
                        // Possible to receive a Continue at the beginning of execution.
                        if !sleep.is_paused() {
                            stopwatch.resume();
                            sleep.as_mut().resume();
                            waiting_stopwatch.resume();
                        }
                        job_control_child(child, child_pid, JobControlEvent::Continue);
                    }
                    RunUnitRequest::Signal(SignalRequest::Shutdown(_)) => {
                        // Receiving a shutdown signal while in this state always means kill
                        // immediately.
                        unsafe {
                            // Send SIGKILL to the process or process group.
                            libc::kill(child_pid.for_kill(), SIGKILL);
                        }
                        break TerminateChildResult::Killed;
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
                // The process didn't exit -- need to do a hard shutdown.
                unsafe {
                    // Send SIGKILL to the process or process group.
                    libc::kill(child_pid.for_kill(), SIGKILL);
                }
                break TerminateChildResult::Killed;
            }
        }
    }
}

fn to_terminate_reason_and_method(
    reason: &InternalTerminateReason,
    grace_period: Duration,
) -> (UnitTerminateReason, UnitTerminateMethod) {
    match reason {
        InternalTerminateReason::Timeout => (
            UnitTerminateReason::Timeout,
            timeout_terminate_method(grace_period),
        ),
        InternalTerminateReason::Signal(req) => (
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
        ShutdownRequest::Once(ShutdownEvent::Signal(sig)) => match sig {
            ShutdownSignalEvent::Hangup => UnitTerminateMethod::Signal(UnitTerminateSignal::Hangup),
            ShutdownSignalEvent::Term => UnitTerminateMethod::Signal(UnitTerminateSignal::Term),
            ShutdownSignalEvent::Quit => UnitTerminateMethod::Signal(UnitTerminateSignal::Quit),
            ShutdownSignalEvent::Interrupt => {
                UnitTerminateMethod::Signal(UnitTerminateSignal::Interrupt)
            }
        },
        ShutdownRequest::Once(ShutdownEvent::TestFailureImmediate) => {
            // Test failure with immediate mode: use SIGTERM like timeout
            UnitTerminateMethod::Signal(UnitTerminateSignal::Term)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::elements::CpuPriorityLevel, cpu_priority_probe::current_nice};
    use std::process::{Child, Command};

    fn child_nice(child: &Child) -> i32 {
        // SAFETY: getpriority is always safe to call. The child is alive (not
        // yet reaped), so PRIO_PROCESS/pid is expected to work.
        unsafe { libc::getpriority(libc::PRIO_PROCESS as _, child.id() as _) }
    }

    #[test]
    fn set_cpu_priority_pre_exec_applies_nice() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        set_cpu_priority_pre_exec(
            &mut cmd,
            Some(CpuPriorityRequest::PreExec(CpuPriorityLevel::Low)),
        );

        let mut child = cmd.spawn().expect("sleep spawns");
        let nice = child_nice(&child);
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(
            nice, 19,
            "low maps to setpriority nice 19 (max niceness when NZERO == 20)",
        );
    }

    #[test]
    fn set_cpu_priority_pre_exec_none_is_a_noop() {
        let parent_nice = current_nice();

        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        set_cpu_priority_pre_exec(&mut cmd, None);

        let mut child = cmd.spawn().expect("sleep spawns");
        let nice = child_nice(&child);
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(
            nice, parent_nice,
            "the child inherits the parent's nice value"
        );
    }
}
