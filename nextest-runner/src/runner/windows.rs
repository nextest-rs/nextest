// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ConfigureHandleInheritanceError,
    runner::{InternalTerminateReason, RunUnitRequest, UnitContext},
    test_command::ChildAccumulator,
    time::StopwatchStart,
};
use std::time::Duration;
use tokio::{process::Child, sync::mpsc::UnboundedReceiver};
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

#[expect(clippy::too_many_arguments)]
pub(super) async fn terminate_child(
    _cx: &UnitContext<'_>,
    child: &mut Child,
    _child_acc: &mut ChildAccumulator,
    reason: InternalTerminateReason,
    _stopwatch: &mut StopwatchStart,
    _req_rx: &mut UnboundedReceiver<RunUnitRequest<'_>>,
    job: Option<&Job>,
    _grace_period: Duration,
) {
    // Ignore signal events since Windows propagates them to child processes (this may change if
    // we start assigning processes to groups on Windows).
    if !matches!(reason, InternalTerminateReason::Timeout) {
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
