// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{fs::File, io::PipeReader, os::windows::io::OwnedHandle};

pub(super) fn pipe_reader_to_file(rx: PipeReader) -> File {
    File::from(OwnedHandle::from(rx))
}
