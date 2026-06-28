// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    config::elements::CpuPriorityLevel,
    double_spawn::DoubleSpawnInfo,
    errors::ConfigureHandleInheritanceError,
    helpers::plural,
    reporter::events::{UnitState, UnitTerminateMethod, UnitTerminateReason, UnitTerminatingState},
    run_mode::NextestRunMode,
    runner::{
        ChildPid, Interceptor, InternalTerminateReason, RunUnitQuery, RunUnitRequest,
        ShutdownRequest, SignalRequest, TerminateChildResult, UnitContext,
    },
    signal::{ShutdownEvent, ShutdownSignalEvent},
    test_command::{ChildAccumulator, CpuPriorityRequest},
    time::StopwatchStart,
};
use std::{
    collections::BTreeMap,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::{
        Once,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use swrite::{SWrite, swrite};
use tokio::{process::Child, runtime::Runtime, sync::mpsc::UnboundedReceiver};
use tracing::{debug, warn};
pub(super) use win32job::Job;
use win32job::JobError;
use windows_sys::Win32::{
    Foundation::{
        ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        LUID, SetHandleInformation,
    },
    Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
        SE_INC_BASE_PRIORITY_NAME, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
    },
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        JobObjects::TerminateJobObject,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

/// Resolves the CPU priority for a test process on Windows.
///
/// This always uses a job object.
// TODO-RAINCLAUDE: unlike Unix, the interceptor and double-spawn don't matter here: the job's priority-class limit applies to every process assigned to the job, including an interceptor wrapper and its descendants, so there is no per-mechanism choice to make.
pub(super) fn resolve_cpu_priority(
    priority: CpuPriorityLevel,
    _interceptor: &Interceptor,
    _double_spawn: &DoubleSpawnInfo,
) -> CpuPriorityRequest {
    CpuPriorityRequest::JobObject(priority)
}

// TODO-RAINCLAUDE: bitsets of CpuPriorityLevel, keyed by `1 << rank()` (an encoding nextest owns, rather than borrowing win32job's repr). DENIED_LEVELS records levels whose job-limit creation was denied: token privileges don't change mid-run, so once a level is denied, skip the doomed create-with-limit on every subsequent spawn rather than repeating the failing kernel calls per test. WARNED_LEVELS records levels already warned about — by the run-start probe (which seeds both bitsets) or by the spawn-time backstop — so each denied level produces exactly one warning per process.
static DENIED_LEVELS: AtomicU8 = AtomicU8::new(0);
static WARNED_LEVELS: AtomicU8 = AtomicU8::new(0);

fn level_bit(level: CpuPriorityLevel) -> u8 {
    1 << level.rank()
}

pub(super) fn create_job(cpu_priority: Option<CpuPriorityRequest>) -> Result<Job, JobError> {
    if let Some(req) = cpu_priority {
        let CpuPriorityRequest::JobObject(level) = req;
        if DENIED_LEVELS.load(Ordering::Relaxed) & level_bit(level) == 0 {
            match create_job_with_limits(Some(req)) {
                Ok(job) => return Ok(job),
                Err(error) => {
                    // TODO-RAINCLAUDE: the priority-class limit can be rejected (high needs SeIncreaseBasePriorityPrivilege enabled). The job object is also nextest's vehicle for timeout kills and grandchild cleanup, so never let a priority failure drop it: record the denial, warn, and retry without the limit. The run-start probe warns with test counts and seeds the bitsets, so the warning here fires only on probe/spawn privilege drift.
                    DENIED_LEVELS.fetch_or(level_bit(level), Ordering::Relaxed);
                    warn_cpu_priority_job_failed(level, error);
                }
            }
        }
    }
    create_job_with_limits(None)
}

// TODO-RAINCLAUDE: creates a job object with nextest's standard limits, plus the priority-class limit if requested. Also used by the run-start probe, which must see the same kernel decision as a real test job. Setting the priority class to high requires SeIncreaseBasePriorityPrivilege *enabled* in the token (unlike SetPriorityClass, where high is unprivileged and only realtime is gated); the privilege is present but disabled in elevated admin tokens, so do a best-effort enable here, at the single point every high job goes through. MSDN documents the job priority-class privilege requirement without qualification; that only high is gated in practice is empirical — see probe_applies_unprivileged_priority_classes below.
fn create_job_with_limits(cpu_priority: Option<CpuPriorityRequest>) -> Result<Job, JobError> {
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_breakaway_ok();
    if let Some(req) = cpu_priority {
        let CpuPriorityRequest::JobObject(level) = req;
        if level == CpuPriorityLevel::High {
            ensure_increase_base_priority_privilege();
        }
        info.limit_priority_class(req.priority_class());
    }
    Job::create_with_limit_info(&mut info)
}

// TODO-RAINCLAUDE: best-effort, once per process; the caller's job creation measures the outcome either way.
fn ensure_increase_base_priority_privilege() {
    static ENABLE_ONCE: Once = Once::new();
    ENABLE_ONCE.call_once(|| {
        if let Err(error) = enable_increase_base_priority_privilege() {
            debug!("unable to enable SeIncreaseBasePriorityPrivilege: {error}");
        }
    });
}

// TODO-RAINCLAUDE: warns that a level's job-limit creation failed at spawn time, once per level per process, naming the level. The run-start probe seeds WARNED_LEVELS for the levels it reports, so this fires only for a level that failed at spawn despite the probe applying it.
fn warn_cpu_priority_job_failed(level: CpuPriorityLevel, error: JobError) {
    let bit = level_bit(level);
    if WARNED_LEVELS.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        warn!(
            "unable to apply the {} cpu-priority class to a job object ({}); \
             affected tests will run at their inherited priority class",
            level.as_str(),
            io::Error::from(error),
        );
    }
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

/// This is a no-op on Windows, and is here only to have a uniform interface
/// with Unix.
pub(super) fn set_cpu_priority_pre_exec(
    _cmd: &mut std::process::Command,
    _cpu_priority: Option<CpuPriorityRequest>,
) {
}

// TODO-RAINCLAUDE: probe whether the tallied CPU priority levels can be applied and warn once with affected test counts, mirroring the Unix probe. No subprocess is needed here: privilege checks happen at job-object creation, in-process, so a throwaway job with the same limits as a real test job measures the real kernel decision (including the best-effort SeIncreaseBasePriorityPrivilege enable that create_job_with_limits performs for high).
pub(super) fn maybe_warn_cpu_priority(
    level_counts: BTreeMap<CpuPriorityLevel, usize>,
    mode: NextestRunMode,
    _double_spawn: &DoubleSpawnInfo,
    _runtime: &Runtime,
) {
    // TODO-RAINCLAUDE: BTreeMap iteration is in rank order (highest priority first) since CpuPriorityLevel's Ord delegates to rank(). A denied level seeds DENIED_LEVELS (so per-spawn creation skips the doomed limit) and WARNED_LEVELS (so the spawn-time backstop stays silent and fires only on probe/spawn drift).
    let mut affected = Vec::new();
    for (&level, &test_count) in &level_counts {
        if let Err(error) = create_job_with_limits(Some(CpuPriorityRequest::JobObject(level))) {
            DENIED_LEVELS.fetch_or(level_bit(level), Ordering::Relaxed);
            WARNED_LEVELS.fetch_or(level_bit(level), Ordering::Relaxed);
            affected.push(AffectedLevel {
                level,
                test_count,
                error: io::Error::from(error),
            });
        }
    }
    if affected.is_empty() {
        return;
    }

    warn!("{}", render_cpu_priority_warning(&affected, mode));
}

// TODO-RAINCLAUDE: a CPU priority level whose job-creation probe failed, with how many tests requested it.
#[derive(Debug)]
struct AffectedLevel {
    level: CpuPriorityLevel,
    test_count: usize,
    error: io::Error,
}

// TODO-RAINCLAUDE: renders the user-facing warning for levels whose job-creation probe failed.
fn render_cpu_priority_warning(affected: &[AffectedLevel], mode: NextestRunMode) -> String {
    let total: usize = affected.iter().map(|a| a.test_count).sum();

    let mut msg = String::new();
    swrite!(
        msg,
        "the requested CPU priority could not be applied for {total} {tests}, \
         which will run at their inherited priority class instead:",
        tests = plural::tests_str(mode, total),
    );
    for a in affected {
        swrite!(
            msg,
            "\n  - {} ({}): {} {} ({})",
            a.level.as_str(),
            priority_class_name(a.level),
            a.test_count,
            plural::tests_str(mode, a.test_count),
            a.error,
        );
    }
    if affected.iter().any(|a| a.level == CpuPriorityLevel::High) {
        swrite!(
            msg,
            "\nsetting a job object's priority class to high requires the \
             SeIncreaseBasePriorityPrivilege privilege."
        );
    }

    msg
}

// TODO-RAINCLAUDE: the priority-class name shown in user-facing messages, matching the names in the config reference.
fn priority_class_name(level: CpuPriorityLevel) -> &'static str {
    match level {
        CpuPriorityLevel::High => "HIGH_PRIORITY_CLASS",
        CpuPriorityLevel::AboveNormal => "ABOVE_NORMAL_PRIORITY_CLASS",
        CpuPriorityLevel::Normal => "NORMAL_PRIORITY_CLASS",
        CpuPriorityLevel::BelowNormal => "BELOW_NORMAL_PRIORITY_CLASS",
        CpuPriorityLevel::Low => "IDLE_PRIORITY_CLASS",
    }
}

// TODO-RAINCLAUDE: enables SeIncreaseBasePriorityPrivilege in this process's token. The privilege is required to set a job object's priority class to high, and is present but disabled in elevated admin tokens. Child processes inherit the token's enabled state, but the privilege only matters at job creation, which happens in this process.
fn enable_increase_base_priority_privilege() -> io::Result<()> {
    let mut token: HANDLE = ptr::null_mut();
    // TODO-RAINCLAUDE: SAFETY — standard Win32 call; GetCurrentProcess returns a pseudo-handle that needs no cleanup, and token is written on success.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // TODO-RAINCLAUDE: SAFETY — token was returned by a successful OpenProcessToken; OwnedHandle closes it on drop.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };

    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    // TODO-RAINCLAUDE: SAFETY — standard Win32 call; the name is a valid static wide string and luid is written on success.
    if unsafe { LookupPrivilegeValueW(ptr::null(), SE_INC_BASE_PRIORITY_NAME, &mut luid) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // TODO-RAINCLAUDE: SAFETY — standard Win32 call over a valid token handle and a properly initialized TOKEN_PRIVILEGES.
    if unsafe {
        AdjustTokenPrivileges(
            token.as_raw_handle(),
            0,
            &privileges,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // TODO-RAINCLAUDE: AdjustTokenPrivileges returns success even when no privileges were assigned; ERROR_NOT_ALL_ASSIGNED must be checked explicitly.
    // TODO-RAINCLAUDE: SAFETY — GetLastError is always safe to call.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the token does not hold SeIncreaseBasePriorityPrivilege",
        ));
    }
    Ok(())
}

// TODO-RAINCLAUDE: the job object is the only vehicle for cpu-priority on Windows, so an assignment failure means the setting is silently dropped for this test; warn once. assign_process_to_job already returns Ok when the child has exited before assignment, so an Err here is a real failure, though a child that exits in the narrow window between the handle check and the assignment can still surface as a (spurious, once-only) warning.
pub(super) fn warn_on_cpu_priority_assign_error(
    cpu_priority: Option<CpuPriorityRequest>,
    error: JobError,
) {
    if cpu_priority.is_some() {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            warn!(
                "unable to assign a test process to its job object ({}); \
                 its cpu-priority setting may not be applied",
                io::Error::from(error),
            );
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_cpu_priority_warning_describes_affected_tests() {
        let affected = vec![
            AffectedLevel {
                level: CpuPriorityLevel::High,
                test_count: 3,
                error: io::Error::from_raw_os_error(1314),
            },
            AffectedLevel {
                level: CpuPriorityLevel::AboveNormal,
                test_count: 1,
                error: io::Error::from_raw_os_error(1314),
            },
        ];

        let msg = render_cpu_priority_warning(&affected, NextestRunMode::Test);
        assert!(
            msg.contains("for 4 tests"),
            "the total affected count is summed across levels: {msg}"
        );
        assert!(
            msg.contains("high (HIGH_PRIORITY_CLASS): 3 tests"),
            "the high level is broken out with its count: {msg}"
        );
        assert!(
            msg.contains("above-normal (ABOVE_NORMAL_PRIORITY_CLASS): 1 test"),
            "the above-normal level is broken out and singularized: {msg}"
        );
        assert!(
            msg.contains("SeIncreaseBasePriorityPrivilege"),
            "the message hints at the privilege requirement when high is affected: {msg}"
        );
    }

    #[test]
    fn render_cpu_priority_warning_omits_privilege_hint_without_high() {
        let affected = vec![AffectedLevel {
            level: CpuPriorityLevel::BelowNormal,
            test_count: 2,
            error: io::Error::from_raw_os_error(5),
        }];

        let msg = render_cpu_priority_warning(&affected, NextestRunMode::Test);
        assert!(
            msg.contains("below-normal (BELOW_NORMAL_PRIORITY_CLASS): 2 tests"),
            "{msg}"
        );
        assert!(
            !msg.contains("SeIncreaseBasePriorityPrivilege"),
            "the privilege hint is specific to the high class: {msg}"
        );
    }

    #[test]
    fn probe_applies_unprivileged_priority_classes() {
        for level in [
            CpuPriorityLevel::AboveNormal,
            CpuPriorityLevel::Normal,
            CpuPriorityLevel::BelowNormal,
            CpuPriorityLevel::Low,
        ] {
            let result = create_job_with_limits(Some(CpuPriorityRequest::JobObject(level)));
            assert!(
                result.is_ok(),
                "the {} class never requires privileges: {:?}",
                level.as_str(),
                result.err(),
            );
        }
    }

    #[test]
    fn create_job_never_fails_on_priority_denial() {
        // TODO-RAINCLAUDE: the job object is nextest's vehicle for timeout kills and grandchild cleanup, so a denied priority class must degrade to an unlimited job rather than an error. On an unprivileged runner this exercises the actual denial-and-fallback path (and the DENIED_LEVELS/WARNED_LEVELS bitsets) for high; on a privileged one every creation succeeds directly. Either way the invariant holds. Note create_job mutates the process-global bitsets, which is fine because nextest runs each test in its own process.
        for level in CpuPriorityLevel::ALL {
            // TODO-RAINCLAUDE: call twice per level to also exercise the denied-and-cached path on unprivileged runners.
            for _ in 0..2 {
                let result = create_job(Some(CpuPriorityRequest::JobObject(level)));
                assert!(
                    result.is_ok(),
                    "create_job falls back to an unlimited job for {}: {:?}",
                    level.as_str(),
                    result.err(),
                );
            }
        }
    }
}
