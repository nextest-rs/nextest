// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{ChildFdError, ErrorList},
    test_command::{create_pipe, spawn_piped, spawn_process},
    test_output::{CaptureStrategy, ChildExecutionOutput, ChildOutput, ChildSplitOutput},
};
use bytes::BytesMut;
use std::{
    io::{self, PipeReader},
    process::Stdio,
    sync::Arc,
};
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

pub(super) fn attach_capture_readers(
    child: &mut TokioChild,
    stdout_rx: Option<PipeReader>,
    stderr_rx: Option<PipeReader>,
) -> io::Result<()> {
    child.stdout = stdout_rx.map(os::pipe_reader_to_child_stdout).transpose()?;
    child.stderr = stderr_rx.map(os::pipe_reader_to_child_stderr).transpose()?;
    Ok(())
}

/// A spawned child process along with its file descriptors.
pub(crate) struct Child {
    pub child: TokioChild,
    pub child_fds: ChildFds,
}

pub(super) fn spawn(
    mut cmd: std::process::Command,
    strategy: CaptureStrategy,
    stdin_passthrough: bool,
) -> std::io::Result<Child> {
    if stdin_passthrough {
        cmd.stdin(Stdio::inherit());
    } else {
        cmd.stdin(Stdio::null());
    }

    let (child, child_fds) = match strategy {
        CaptureStrategy::None => {
            let child = spawn_process(cmd)?;
            (child, ChildFds::new_split(None, None))
        }
        CaptureStrategy::Split => {
            let mut child = spawn_piped(cmd, true, true)?;
            let stdout = child.stdout.take().expect("stdout was set");
            let stderr = child.stderr.take().expect("stderr was set");
            (child, ChildFds::new_split(Some(stdout), Some(stderr)))
        }
        CaptureStrategy::Combined => {
            let (rx, tx) = create_pipe()?;
            cmd.stdout(tx.try_clone()?).stderr(tx);
            let child = spawn_process(cmd)?;
            let combined = os::pipe_reader_to_file(rx).into();
            (child, ChildFds::new_combined(combined))
        }
    };

    Ok(Child { child, child_fds })
}

/// The size of each buffered reader's buffer, and the size at which we grow the combined buffer.
///
/// This size is not totally arbitrary, but rather the (normal) page size on most systems.
const CHUNK_SIZE: usize = 4 * 1024;

/// A `BufReader` over an `AsyncRead` that tracks the state of the reader and
/// whether it is done.
pub(crate) struct FusedBufReader<R> {
    reader: BufReader<R>,
    done: bool,
}

impl<R: AsyncRead + Unpin> FusedBufReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::with_capacity(CHUNK_SIZE, reader),
            done: false,
        }
    }

    pub(crate) async fn fill_buf(&mut self, acc: &mut BytesMut) -> Result<(), io::Error> {
        if self.done {
            return Ok(());
        }

        let res = self.reader.fill_buf().await;
        match res {
            Ok(buf) => {
                acc.extend_from_slice(buf);
                if buf.is_empty() {
                    self.done = true;
                }
                let len = buf.len();
                self.reader.consume(len);
                Ok(())
            }
            Err(error) => {
                self.done = true;
                Err(error)
            }
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.done
    }
}

/// A version of [`FusedBufReader::fill_buf`] that works with an `Option<FusedBufReader>`.
async fn fill_buf_opt<R: AsyncRead + Unpin>(
    reader: Option<&mut FusedBufReader<R>>,
    acc: Option<&mut BytesMut>,
) -> Result<(), io::Error> {
    if let Some(reader) = reader {
        let acc = acc.expect("reader and acc must match");
        reader.fill_buf(acc).await
    } else {
        Ok(())
    }
}

/// A version of [`FusedBufReader::is_done`] that works with an `Option<FusedBufReader>`.
fn is_done_opt<R: AsyncRead + Unpin>(reader: Option<&FusedBufReader<R>>) -> bool {
    reader.is_none_or(|r| r.is_done())
}

/// Output and result accumulator for a child process.
pub(crate) struct ChildAccumulator {
    // TODO: it would be nice to also store the tokio::process::Child here, and
    // for `fill_buf` to select over it.
    pub(crate) fds: ChildFds,
    pub(crate) output: ChildOutputMut,
    pub(crate) errors: Vec<ChildFdError>,
}

impl ChildAccumulator {
    pub(crate) fn new(fds: ChildFds) -> Self {
        let output = fds.make_acc();
        Self {
            fds,
            output,
            errors: Vec::new(),
        }
    }

    pub(crate) async fn fill_buf(&mut self) {
        let res = self.fds.fill_buf(&mut self.output).await;
        if let Err(error) = res {
            self.errors.push(error);
        }
    }

    pub(crate) fn snapshot_in_progress(
        &self,
        error_description: &'static str,
    ) -> ChildExecutionOutput {
        ChildExecutionOutput::Output {
            result: None,
            output: self.output.snapshot(),
            errors: ErrorList::new(error_description, self.errors.clone()),
        }
    }
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

    pub(crate) fn new_combined(rx: File) -> Self {
        Self::Combined {
            combined: FusedBufReader::new(rx),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Split { stdout, stderr } => {
                is_done_opt(stdout.as_ref()) && is_done_opt(stderr.as_ref())
            }
            Self::Combined { combined } => combined.is_done(),
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
    pub(crate) async fn fill_buf(&mut self, acc: &mut ChildOutputMut) -> Result<(), ChildFdError> {
        match self {
            Self::Split { stdout, stderr } => {
                let (stdout_acc, stderr_acc) = acc.as_split_mut();
                // Wait until either of these make progress.
                tokio::select! {
                    res = fill_buf_opt(stdout.as_mut(), stdout_acc), if !is_done_opt(stdout.as_ref()) => {
                        res.map_err(|error| ChildFdError::ReadStdout(Arc::new(error)))
                    }
                    res = fill_buf_opt(stderr.as_mut(), stderr_acc), if !is_done_opt(stderr.as_ref()) => {
                        res.map_err(|error| ChildFdError::ReadStderr(Arc::new(error)))
                    }
                    // If both are done, do nothing.
                    else => {
                        Ok(())
                    }
                }
            }
            Self::Combined { combined } => {
                if !combined.is_done() {
                    combined
                        .fill_buf(acc.as_combined_mut())
                        .await
                        .map_err(|error| ChildFdError::ReadCombined(Arc::new(error)))
                } else {
                    Ok(())
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

    /// Makes a snapshot of the current output, returning a [`TestOutput`].
    ///
    /// This requires cloning the output so it's more expensive than [`Self::freeze`].
    pub(crate) fn snapshot(&self) -> ChildOutput {
        match self {
            Self::Split { stdout, stderr } => ChildOutput::Split(ChildSplitOutput {
                stdout: stdout.as_ref().map(|x| x.clone().freeze().into()),
                stderr: stderr.as_ref().map(|x| x.clone().freeze().into()),
            }),
            Self::Combined(combined) => ChildOutput::Combined {
                output: combined.clone().freeze().into(),
            },
        }
    }

    /// Marks the collection as done, returning a `TestOutput`.
    pub(crate) fn freeze(self) -> ChildOutput {
        match self {
            Self::Split { stdout, stderr } => ChildOutput::Split(ChildSplitOutput {
                stdout: stdout.map(|x| x.freeze().into()),
                stderr: stderr.map(|x| x.freeze().into()),
            }),
            Self::Combined(combined) => ChildOutput::Combined {
                output: combined.freeze().into(),
            },
        }
    }

    /// Returns the lengths of stdout and stderr in bytes.
    ///
    /// Returns `None` for each stream that wasn't captured.
    pub(crate) fn stdout_stderr_len(&self) -> (Option<u64>, Option<u64>) {
        match self {
            Self::Split { stdout, stderr } => (
                stdout.as_ref().map(|b| b.len() as u64),
                stderr.as_ref().map(|b| b.len() as u64),
            ),
            Self::Combined(combined) => (Some(combined.len() as u64), None),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{process::Command, sync::Barrier, thread, time::Duration};
    use test_case::test_case;

    // 32 threads oversubscribe the measured 3- and 10-core machines. Without
    // the lock, one sample first failed at round 16 (zero-based), so use 32
    // rounds to leave margin.
    const SPAWN_CONCURRENCY: usize = 32;
    const SPAWN_ROUNDS: usize = 32;
    const CAPTURE_MARKER: &str = "NEXTEST_CAPTURE_CLOSED";
    const EOF_TIMEOUT: Duration = Duration::from_secs(30);
    const CHILD_LIFETIME_SECS: u64 = 120;

    /// Keep children alive after closing stdout and stderr so an inherited
    /// writer in a sibling delays EOF.
    #[test_case(CaptureStrategy::Split, ChildProgram::Absolute; "split absolute")]
    #[test_case(CaptureStrategy::Combined, ChildProgram::Absolute; "combined absolute")]
    #[cfg_attr(
        target_vendor = "apple",
        test_case(CaptureStrategy::Split, ChildProgram::RelativeWithCwd; "split relative")
    )]
    #[cfg_attr(
        target_vendor = "apple",
        test_case(CaptureStrategy::Combined, ChildProgram::RelativeWithCwd; "combined relative")
    )]
    fn concurrent_spawns_do_not_inherit_capture_pipes(
        strategy: CaptureStrategy,
        program: ChildProgram,
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime starts");

        for round in 0..SPAWN_ROUNDS {
            let barrier = Barrier::new(SPAWN_CONCURRENCY);
            let children = thread::scope(|scope| {
                (0..SPAWN_CONCURRENCY)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            let _guard = runtime.enter();
                            spawn_lingering_child(strategy, program)
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().expect("spawn thread does not panic"))
                    .collect::<io::Result<Vec<_>>>()
                    .expect("children start")
            });

            let (fds, processes): (Vec<_>, Vec<_>) = children
                .into_iter()
                .map(|Child { child, child_fds }| (child_fds, child))
                .unzip();
            let mut lingering = LingeringChildren(processes);
            for (index, child_fds) in fds.into_iter().enumerate() {
                runtime.block_on(assert_capture_closes(child_fds, round, index));
            }
            runtime.block_on(lingering.kill_all());
        }
    }

    #[derive(Clone, Copy)]
    enum ChildProgram {
        Absolute,
        /// The cwd forces Apple's fork/exec fallback for a relative program.
        #[cfg(target_vendor = "apple")]
        RelativeWithCwd,
    }

    /// Kill on panic so inherited writers cannot delay runtime shutdown.
    struct LingeringChildren(Vec<TokioChild>);

    impl LingeringChildren {
        async fn kill_all(&mut self) {
            for child in &mut self.0 {
                child.kill().await.expect("lingering child is killed");
            }
        }
    }

    impl Drop for LingeringChildren {
        fn drop(&mut self) {
            for child in &mut self.0 {
                _ = child.start_kill();
            }
        }
    }

    fn spawn_lingering_child(
        strategy: CaptureStrategy,
        program: ChildProgram,
    ) -> io::Result<Child> {
        let mut command = match program {
            ChildProgram::Absolute => Command::new("/bin/sh"),
            #[cfg(target_vendor = "apple")]
            ChildProgram::RelativeWithCwd => {
                let mut command = Command::new("./sh");
                command.current_dir("/bin");
                command
            }
        };
        // Replace the shell so cleanup can kill the child without orphaning sleep.
        command.arg("-c").arg(format!(
            "echo {CAPTURE_MARKER}; exec >&- 2>&-; exec sleep {CHILD_LIFETIME_SECS}"
        ));
        spawn(command, strategy, false)
    }

    async fn assert_capture_closes(child_fds: ChildFds, round: usize, index: usize) {
        let mut accumulator = ChildAccumulator::new(child_fds);
        let drained = tokio::time::timeout(EOF_TIMEOUT, async {
            while !accumulator.fds.is_done() {
                accumulator.fill_buf().await;
            }
        })
        .await;
        assert!(
            drained.is_ok(),
            "child {index} in round {round} closed its capture pipes, but a sibling still holds them"
        );
        assert!(
            accumulator.errors.is_empty(),
            "capture reads succeed: {:?}",
            accumulator.errors
        );

        let stdout = match &accumulator.output {
            ChildOutputMut::Split { stdout, .. } => stdout.as_ref().expect("stdout is captured"),
            ChildOutputMut::Combined(output) => output,
        };
        assert!(
            std::str::from_utf8(stdout)
                .expect("child output is UTF-8")
                .contains(CAPTURE_MARKER),
            "child {index} in round {round} did not write the marker to its capture pipe"
        );
    }
}
