// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{fs::File, io, io::PipeReader, os::fd::OwnedFd, process};
use tokio::process::{ChildStderr, ChildStdout};

pub(super) fn pipe_reader_to_file(rx: PipeReader) -> File {
    File::from(OwnedFd::from(rx))
}

pub(super) fn pipe_reader_to_child_stdout(rx: PipeReader) -> io::Result<ChildStdout> {
    ChildStdout::from_std(process::ChildStdout::from(OwnedFd::from(rx)))
}

pub(super) fn pipe_reader_to_child_stderr(rx: PipeReader) -> io::Result<ChildStderr> {
    ChildStderr::from_std(process::ChildStderr::from(OwnedFd::from(rx)))
}
