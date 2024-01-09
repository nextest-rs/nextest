//! Utilities for capture output from tests run in a child process

use bstr::{ByteSlice, Lines};
use bytes::{Bytes, BytesMut};
use std::borrow::Cow;
use tokio::io::BufReader;

/// The strategy used to capture test executable output
#[derive(Copy, Clone, PartialEq, Default, Debug)]
pub enum CaptureStrategy {
    /// Captures `stdout` and `stderr` separately
    ///
    /// * pro: output from `stdout` and `stderr` can be identified and easily split
    /// * con: ordering between the streams cannot be guaranteed
    #[default]
    Split,
    /// Captures `stdout` and `stderr` in a single stream
    ///
    /// * pro: output is guaranteed to be ordered as it would in a terminal emulator
    /// * con: distinction between `stdout` and `stderr` is lost, all output is attributed to `stdout`
    Combined,
    /// Output is not captured
    ///
    /// This mode is used when using --no-capture, causing nextest to execute
    /// tests serially without capturing output
    None,
}

/// A single output for a test.
///
/// This is a wrapper around a [`Bytes`] that provides some convenience methods.
#[derive(Clone, Debug)]
pub struct TestSingleOutput {
    /// The raw output buffer
    pub buf: Bytes,
}

impl From<Bytes> for TestSingleOutput {
    #[inline]
    fn from(buf: Bytes) -> Self {
        Self { buf }
    }
}

impl TestSingleOutput {
    /// Gets this output as a lossy UTF-8 string.
    #[inline]
    pub fn to_str_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.buf)
    }

    /// Iterates over lines in this output.
    #[inline]
    pub fn lines(&self) -> Lines<'_> {
        self.buf.lines()
    }

    /// Returns true if the output is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// The complete captured output of a child process
#[derive(Clone, Debug)]
pub enum TestOutput {
    /// The output was split into stdout and stderr.
    Split {
        /// The captured stdout.
        stdout: TestSingleOutput,

        /// The captured stderr.
        stderr: TestSingleOutput,
    },

    /// The output was combined into stdout and stderr.
    Combined {
        /// The captured output.
        output: TestSingleOutput,
    },

    /// The output was an execution failure.
    ExecFail {
        /// A single-line message.
        message: String,

        /// The full description, including other errors, to print out.
        description: String,
    },
}

/// The size of each buffered reader's buffer, and the size at which we grow
/// the interleaved buffer.
///
/// This size is not totally arbitrary, but rather the (normal) page size on
/// most linux, windows, and macos systems.
const CHUNK_SIZE: usize = 4 * 1024;

use crate::errors::CollectTestOutputError as Err;

/// Collects the stdout and/or stderr streams into a single buffer
pub async fn collect_test_output(
    streams: Option<crate::test_command::Output>,
) -> Result<Option<TestOutput>, Err> {
    use tokio::io::AsyncBufReadExt as _;

    let Some(output) = streams else {
        return Ok(None);
    };

    match output {
        crate::test_command::Output::Split { stdout, stderr } => {
            let mut stdout = BufReader::with_capacity(CHUNK_SIZE, stdout);
            let mut stderr = BufReader::with_capacity(CHUNK_SIZE, stderr);

            let mut stdout_acc = BytesMut::with_capacity(CHUNK_SIZE);
            let mut stderr_acc = BytesMut::with_capacity(CHUNK_SIZE);

            let mut out_done = false;
            let mut err_done = false;

            loop {
                tokio::select! {
                    res = stdout.fill_buf(), if !out_done => {
                        let read = {
                            let buf = res.map_err(Err::ReadStdout)?;
                            stdout_acc.extend_from_slice(buf);
                            buf.len()
                        };

                        stdout.consume(read);
                        out_done = read == 0;
                    }
                    res = stderr.fill_buf(), if !err_done => {
                        let read = {
                            let buf = res.map_err(Err::ReadStderr)?;
                            stderr_acc.extend_from_slice(buf);
                            buf.len()
                        };

                        stderr.consume(read);
                        err_done = read == 0;
                    }
                    else => break,
                };
            }

            Ok(Some(TestOutput::Split {
                stdout: stdout_acc.freeze().into(),
                stderr: stderr_acc.freeze().into(),
            }))
        }
        crate::test_command::Output::Combined(output) => {
            let mut output = BufReader::with_capacity(CHUNK_SIZE, output);
            let mut acc = BytesMut::with_capacity(CHUNK_SIZE);

            loop {
                let read = {
                    let buf = output.fill_buf().await.map_err(Err::ReadStdout)?;
                    acc.extend_from_slice(buf);
                    buf.len()
                };

                output.consume(read);
                if read == 0 {
                    break;
                }
            }

            Ok(Some(TestOutput::Combined {
                output: acc.freeze().into(),
            }))
        }
    }
}
