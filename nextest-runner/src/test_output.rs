//! Utilities for capture output from tests run in a child process

use bytes::{Bytes, BytesMut};
use std::{io::Write as _, time::Instant};
use tokio::io::AsyncBufReadExt;

/// A single chunk of captured output, this may represent 0 or more lines
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct OutputChunk {
    /// The byte range the chunk occupies in the buffer
    range: std::ops::Range<usize>,
    /// The timestamp the chunk was read
    timestamp: Instant,
    /// True if stdout, false if stderr
    stdout: bool,
}

/// The complete captured output of a child process
#[derive(Clone, Debug)]
pub struct TestOutput {
    /// The raw buffer of combined stdout and stderr
    pub buf: Bytes,
    /// Description of each individual chunk that was streamed from the test
    /// process
    pub chunks: Vec<OutputChunk>,
    /// The start of the beginning of the capture, so that each individual
    /// chunk can get an elapsed time if needed
    pub start: Instant,
}

impl TestOutput {
    /// Gets only stdout as a lossy utf-8 string
    #[inline]
    pub fn stdout_lossy(&self) -> String {
        self.as_string(true)
    }

    /// Gets only stdout as a lossy utf-8 string
    #[inline]
    pub fn stderr_lossy(&self) -> String {
        self.as_string(false)
    }

    /// Gets the combined stdout and stderr streams as a lossy utf-8 string
    #[inline]
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.buf)
    }

    fn as_string(&self, stdout: bool) -> String {
        // Presize the buffer, assuming that we'll have well formed utf8 in
        // almost all cases
        let count = self
            .chunks
            .iter()
            .filter_map(|oc| (oc.stdout == stdout).then_some(oc.range.len()))
            .sum();

        self.chunks
            .iter()
            .fold(String::with_capacity(count), |mut acc, oc| {
                if oc.stdout != stdout {
                    return acc;
                }

                // This is the lazy way to do this, but as stated, the normal case
                // should be utf-8 strings so not a big deal if we get the occasional
                // allocation
                let chunk = String::from_utf8_lossy(&self.buf[oc.range.clone()]);
                acc.push_str(&chunk);
                acc
            })
    }

    /// Gets the raw stdout buffer
    #[inline]
    pub fn stdout(&self) -> Bytes {
        self.as_buf(true)
    }

    /// Gets the raw stderr buffer
    #[inline]
    pub fn stderr(&self) -> Bytes {
        self.as_buf(false)
    }

    fn as_buf(&self, stdout: bool) -> Bytes {
        let count = self
            .chunks
            .iter()
            .filter_map(|oc| (oc.stdout == stdout).then_some(oc.range.len()))
            .sum();

        self.chunks
            .iter()
            .fold(bytes::BytesMut::with_capacity(count), |mut acc, oc| {
                if oc.stdout != stdout {
                    return acc;
                }

                acc.extend_from_slice(&self.buf[oc.range.clone()]);
                acc
            })
            .freeze()
    }
}

/// Captures the stdout and/or stderr streams into a buffer, indexed on each
/// chunk of output including the timestamp and which stream it came from
pub struct TestOutputAccumulator {
    buf: BytesMut,
    chunks: Vec<OutputChunk>,
    start: Instant,
}

impl TestOutputAccumulator {
    /// Creates a new test accumulator to capture output from a child process
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(4 * 1024),
            chunks: Vec::new(),
            start: Instant::now(),
        }
    }

    /// Similar to [`bytes::BytesMut::freeze`], this is called when output
    /// capturing is complete to create a [`TestOutput`] of the complete
    /// captured output
    pub fn freeze(self) -> TestOutput {
        TestOutput {
            buf: self.buf.freeze(),
            chunks: self.chunks,
            start: self.start,
        }
    }

    /// Gets a writer the can be used to write to the accumulator as if a child
    /// process was writing to stdout
    #[inline]
    pub fn stdout(&mut self) -> TestOutputWriter<'_> {
        TestOutputWriter {
            acc: self,
            stdout: true,
        }
    }

    /// Gets a writer the can be used to write to the accumulator as if a child
    /// process was writing to stderr
    #[inline]
    pub fn stderr(&mut self) -> TestOutputWriter<'_> {
        TestOutputWriter {
            acc: self,
            stdout: false,
        }
    }
}

/// Provides [`std::io::Write`] and [`std::fmt::Write`] implementations for a
/// single stream of a [`TestOutputAccumulator`]
pub struct TestOutputWriter<'acc> {
    acc: &'acc mut TestOutputAccumulator,
    stdout: bool,
}

impl<'acc> std::io::Write for TestOutputWriter<'acc> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let start = self.acc.buf.len();
        self.acc.buf.extend_from_slice(buf);
        self.acc.chunks.push(OutputChunk {
            range: start..self.acc.buf.len(),
            timestamp: Instant::now(),
            stdout: self.stdout,
        });

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'acc> std::fmt::Write for TestOutputWriter<'acc> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match self.write(s.as_bytes()) {
            Ok(_) => Ok(()),
            Err(_) => Err(std::fmt::Error),
        }
    }
}

/// The size of each buffered reader's buffer, and the size at which we grow
/// the interleaved buffer.
///
/// This size is not totally arbitrary, but rather the (normal) page size on
/// most linux, windows, and macos systems.
const CHUNK_SIZE: usize = 4 * 1024;

/// Collects the stdout and/or stderr streams into a single buffer
pub async fn collect_test_output(
    streams: Option<(tokio::process::ChildStdout, tokio::process::ChildStderr)>,
    acc: &mut TestOutputAccumulator,
) -> Result<(), crate::errors::CollectTestOutputError> {
    let Some((stdout, stderr)) = streams else {
        return Ok(());
    };

    let mut stdout = tokio::io::BufReader::with_capacity(CHUNK_SIZE, stdout);
    let mut stderr = tokio::io::BufReader::with_capacity(CHUNK_SIZE, stderr);

    let mut out_done = false;
    let mut err_done = false;

    while !out_done || !err_done {
        tokio::select! {
            res = stdout.fill_buf() => {
                let read = {
                    let buf = res.map_err(crate::errors::CollectTestOutputError::ReadStdout)?;
                    push_chunk(acc, buf, true);
                    buf.len()
                };

                stdout.consume(read);
                out_done = read == 0;
            }
            res = stderr.fill_buf() => {
                let read = {
                    let buf = res.map_err(crate::errors::CollectTestOutputError::ReadStderr)?;
                    push_chunk(acc, buf, false);
                    buf.len()
                };

                stderr.consume(read);
                err_done = read == 0;
            }
        };
    }

    Ok(())
}

#[inline]
fn push_chunk(acc: &mut TestOutputAccumulator, chunk: &[u8], stdout: bool) {
    let start = acc.buf.len();

    if acc.buf.capacity() - start < chunk.len() {
        acc.buf.reserve(CHUNK_SIZE);
    }

    acc.buf.extend_from_slice(chunk);
    acc.chunks.push(OutputChunk {
        range: start..start + chunk.len(),
        timestamp: Instant::now(),
        stdout,
    });
}
