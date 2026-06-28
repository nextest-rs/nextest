// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::config::elements::CpuPriorityLevel;
use std::{fs::File, io::PipeReader, os::windows::io::OwnedHandle};
use win32job::PriorityClass;

pub(super) fn pipe_reader_to_file(rx: PipeReader) -> File {
    File::from(OwnedHandle::from(rx))
}

pub(super) fn to_priority_class(priority: CpuPriorityLevel) -> PriorityClass {
    match priority {
        CpuPriorityLevel::High => PriorityClass::High,
        CpuPriorityLevel::AboveNormal => PriorityClass::AboveNormal,
        CpuPriorityLevel::Normal => PriorityClass::Normal,
        CpuPriorityLevel::BelowNormal => PriorityClass::BelowNormal,
        CpuPriorityLevel::Low => PriorityClass::Idle,
    }
}
