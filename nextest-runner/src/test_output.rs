//! Utilities for capture output from tests run in a child process

use bytes::{Bytes, BytesMut};
use std::{io::Write as _, ops::Range, time::Instant};
use tokio::io::AsyncBufReadExt;

/// A single chunk of captured output, this may represent 0 or more lines
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct OutputChunk {
    /// The byte range the chunk occupies in the buffer
    range: Range<usize>,
    /// The timestamp the chunk was read
    pub timestamp: Instant,
    /// True if stdout, false if stderr
    pub stdout: bool,
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

    /// Retrieves an iterator over the lines in the output
    #[inline]
    pub fn lines(&self) -> LinesIterator<'_> {
        LinesIterator::new(self)
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

struct ChunkIterator<'acc> {
    chunk: &'acc OutputChunk,
    haystack: &'acc [u8],
    continues: bool,
}

impl<'acc> ChunkIterator<'acc> {
    fn new(buf: &'acc [u8], chunk: &'acc OutputChunk, continues: bool) -> Self {
        Self {
            chunk,
            haystack: &buf[chunk.range.clone()],
            continues,
        }
    }
}

const LF: u8 = b'\n';

impl<'acc> Iterator for ChunkIterator<'acc> {
    type Item = Line<'acc>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.haystack.is_empty() {
            return None;
        }

        let (ret, remaining, has) = match memchr::memchr(LF, self.haystack) {
            Some(pos) => (&self.haystack[..pos + 1], &self.haystack[pos + 1..], true),
            None => (self.haystack, &[][..], false),
        };
        self.haystack = remaining;

        let kind = if self.continues {
            self.continues = false;

            if has {
                LineKind::End
            } else {
                LineKind::None
            }
        } else if has {
            LineKind::Complete
        } else {
            LineKind::Begin
        };

        Some(Line {
            chunk: self.chunk,
            raw: ret,
            kind,
        })
    }
}

pub struct LinesIterator<'acc> {
    acc: &'acc TestOutput,
    cur_chunk: usize,
    chunk_iter: Option<ChunkIterator<'acc>>,
}

impl<'acc> LinesIterator<'acc> {
    fn new(acc: &'acc TestOutput) -> Self {
        let mut this = Self {
            acc,
            cur_chunk: 0,
            chunk_iter: None,
        };

        this.advance(0);
        this
    }

    fn advance(&mut self, chunk_ind: usize) {
        let Some(chunk) = self.acc.chunks.get(chunk_ind) else {
            self.chunk_iter = None;
            return;
        };

        let continues = self.chunk_iter.take().map_or(false, |ci| {
            !self.acc.buf[ci.chunk.range.clone()].ends_with(&[LF])
        });
        self.chunk_iter = Some(ChunkIterator::new(&self.acc.buf, chunk, continues));
        self.cur_chunk = chunk_ind;
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum LineKind {
    /// The raw data encompasses a complete line from beginning to end
    Complete,
    /// The raw data begins a line, but the output chunk ends before a newline
    Begin,
    /// The raw data ends a line that was started in a different chunk
    End,
    /// No line feeds were present in the chunk
    None,
}

pub struct Line<'acc> {
    /// The parent chunk which this line is a subslice of
    pub chunk: &'acc OutputChunk,
    /// The raw data for this line entry
    pub raw: &'acc [u8],
    pub kind: LineKind,
}

impl<'acc> Line<'acc> {
    #[inline]
    pub fn lossy(&self) -> std::borrow::Cow<'acc, str> {
        String::from_utf8_lossy(self.raw)
    }
}

impl<'acc> Iterator for LinesIterator<'acc> {
    type Item = Line<'acc>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            {
                let Some(chunk) = &mut self.chunk_iter else {
                    return None;
                };

                if let Some(line) = chunk.next() {
                    return Some(line);
                }
            }

            self.advance(self.cur_chunk + 1);
        }
    }
}
