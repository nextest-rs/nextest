// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::ChildError,
    test_output::{CaptureStrategy, ChildSplitOutput, TestOutput},
};
use bytes::BytesMut;
use std::{io, process::Stdio};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child as TokioChild, ChildStderr, ChildStdout},
};

cfg_if::cfg_if! {
    if #[cfg(unix)] {
        #[path = "unix.rs"]
        mod unix;
        use unix as os;
    } else if #[cfg(windows)] {
        #[path = "windows.rs"]
        mod windows;
        use windows as os;
    } else {
        compile_error!("unsupported target platform");
    }
}

/// A spawned child process along with its file descriptors.
pub(crate) struct Child {
    pub child: TokioChild,
    pub child_fds: ChildFds,
}

pub(super) fn spawn(
    mut cmd: std::process::Command,
    strategy: CaptureStrategy,
) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null());

    let state: Option<os::State> = match strategy {
        CaptureStrategy::None => None,
        CaptureStrategy::Split => {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            None
        }
        CaptureStrategy::Combined => Some(os::setup_io(&mut cmd)?),
    };

    let mut cmd: tokio::process::Command = cmd.into();
    let mut child = cmd.spawn()?;

    let output = match strategy {
        CaptureStrategy::None => ChildFds::new_split(None, None),
        CaptureStrategy::Split => {
            let stdout = child.stdout.take().expect("stdout was set");
            let stderr = child.stderr.take().expect("stderr was set");

            ChildFds::new_split(Some(stdout), Some(stderr))
        }
        CaptureStrategy::Combined => {
            ChildFds::new_combined(std::fs::File::from(state.expect("state was set").ours).into())
        }
    };

    Ok(Child {
        child,
        child_fds: output,
    })
}

/// The size of each buffered reader's buffer, and the size at which we grow the combined buffer.
///
/// This size is not totally arbitrary, but rather the (normal) page size on most systems.
const CHUNK_SIZE: usize = 4 * 1024;

/// A `BufReader` over an `AsyncRead` that tracks the state of the reader and the result of the last
/// operation.
pub(crate) struct FusedBufReader<R> {
    reader: BufReader<R>,
    res: Option<Result<(), io::Error>>,
}

impl<R: AsyncRead + Unpin> FusedBufReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::with_capacity(CHUNK_SIZE, reader),
            res: None,
        }
    }

    pub(crate) async fn fill_buf(&mut self, acc: &mut BytesMut) {
        if !self.is_done() {
            let res = self.reader.fill_buf().await;
            match res {
                Ok(buf) => {
                    acc.extend_from_slice(buf);
                    if buf.is_empty() {
                        self.res = Some(Ok(()));
                    }
                    let len = buf.len();
                    self.reader.consume(len);
                }
                Err(error) => {
                    self.res = Some(Err(error));
                }
            }
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.res.is_some()
    }
}

/// A version of [`FusedBufReader::fill_buf`] that works with an `Option<FusedBufReader>`.
async fn fill_buf_opt<R: AsyncRead + Unpin>(
    reader: Option<&mut FusedBufReader<R>>,
    acc: Option<&mut BytesMut>,
) {
    if let Some(reader) = reader {
        let acc = acc.expect("reader and acc must match");
        reader.fill_buf(acc).await;
    }
}

/// A version of [`FusedBufReader::is_done`] that works with an `Option<FusedBufReader>`.
fn is_done_opt<R: AsyncRead + Unpin>(reader: &Option<FusedBufReader<R>>) -> bool {
    reader.as_ref().map_or(true, |r| r.is_done())
}

/// File descriptors (or Windows handles) for the child process.
pub(crate) enum ChildFds {
    /// Separate stdout and stderr, or they're not captured.
    Split {
        stdout: Option<FusedBufReader<ChildStdout>>,
        stderr: Option<FusedBufReader<ChildStderr>>,
    },

    /// Combined stdout and stderr.
    Combined { combined: FusedBufReader<File> },
}

impl ChildFds {
    pub(crate) fn new_split(stdout: Option<ChildStdout>, stderr: Option<ChildStderr>) -> Self {
        Self::Split {
            stdout: stdout.map(FusedBufReader::new),
            stderr: stderr.map(FusedBufReader::new),
        }
    }

    pub(crate) fn new_combined(file: File) -> Self {
        Self::Combined {
            combined: FusedBufReader::new(file),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Split { stdout, stderr } => is_done_opt(stdout) && is_done_opt(stderr),
            Self::Combined { combined } => combined.is_done(),
        }
    }

    /// Consumes `self`, returning any errors that occurred while collecting the output.
    pub(crate) fn into_errors(self) -> Vec<ChildError> {
        match self {
            Self::Split { stdout, stderr } => {
                let mut errors = Vec::new();
                if let Some(stdout) = stdout {
                    if let Some(Err(error)) = stdout.res {
                        errors.push(ChildError::ReadStdout(error));
                    }
                }
                if let Some(stderr) = stderr {
                    if let Some(Err(error)) = stderr.res {
                        errors.push(ChildError::ReadStderr(error));
                    }
                }
                errors
            }
            Self::Combined { combined } => {
                if let Some(Err(error)) = combined.res {
                    vec![ChildError::ReadCombined(error)]
                } else {
                    Vec::new()
                }
            }
        }
    }
}

impl ChildFds {
    /// Makes an empty `ChildOutput` with the appropriate buffers for this `ChildFds`.
    pub(crate) fn make_acc(&self) -> ChildOutputMut {
        match self {
            Self::Split { stdout, stderr } => ChildOutputMut::Split {
                stdout: stdout.as_ref().map(|_| BytesMut::with_capacity(CHUNK_SIZE)),
                stderr: stderr.as_ref().map(|_| BytesMut::with_capacity(CHUNK_SIZE)),
            },
            Self::Combined { .. } => ChildOutputMut::Combined(BytesMut::with_capacity(CHUNK_SIZE)),
        }
    }

    /// Fills one of the buffers in `acc` with available data from the child process.
    ///
    /// This is a single step in the process of collecting the output of a child process. This
    /// operation is cancel-safe, since the underlying [`AsyncBufReadExt::fill_buf`] operation is
    /// cancel-safe.
    ///
    /// We follow this "externalized progress" pattern rather than having the collect output futures
    /// own the data they're collecting, to enable future improvements where we can dump
    /// currently-captured output to the terminal.
    pub(crate) async fn fill_buf(&mut self, acc: &mut ChildOutputMut) {
        match self {
            Self::Split { stdout, stderr } => {
                let (stdout_acc, stderr_acc) = acc.as_split_mut();
                // Wait until either of these make progress.
                tokio::select! {
                    _ = fill_buf_opt(stdout.as_mut(), stdout_acc), if !is_done_opt(stdout) => {}
                    _ = fill_buf_opt(stderr.as_mut(), stderr_acc), if !is_done_opt(stderr) => {}
                    // If both are done, do nothing.
                    else => {}
                }
            }
            Self::Combined { combined } => {
                if !combined.is_done() {
                    combined.fill_buf(acc.as_combined_mut()).await;
                }
            }
        }
    }
}

/// The output of a child process that's currently being collected.
pub(crate) enum ChildOutputMut {
    /// Separate stdout and stderr (`None` if not captured).
    Split {
        stdout: Option<BytesMut>,
        stderr: Option<BytesMut>,
    },
    /// Combined stdout and stderr.
    Combined(BytesMut),
}

impl ChildOutputMut {
    fn as_split_mut(&mut self) -> (Option<&mut BytesMut>, Option<&mut BytesMut>) {
        match self {
            Self::Split { stdout, stderr } => (stdout.as_mut(), stderr.as_mut()),
            _ => panic!("ChildOutput is not split"),
        }
    }

    fn as_combined_mut(&mut self) -> &mut BytesMut {
        match self {
            Self::Combined(combined) => combined,
            _ => panic!("ChildOutput is not combined"),
        }
    }

    /// Marks the collection as done, returning a `TestOutput`.
    pub(crate) fn freeze(self) -> TestOutput {
        match self {
            Self::Split { stdout, stderr } => TestOutput::Split(ChildSplitOutput {
                stdout: stdout.map(|x| x.freeze().into()),
                stderr: stderr.map(|x| x.freeze().into()),
            }),
            Self::Combined(combined) => TestOutput::Combined {
                output: combined.freeze().into(),
            },
        }
    }
}
