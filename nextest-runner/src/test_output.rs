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

    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        let start = self.acc.buf.len();

        let mut len = 0;
        for buf in bufs {
            self.acc.buf.extend_from_slice(buf);
            len += buf.len();
        }

        self.acc.chunks.push(OutputChunk {
            range: start..self.acc.buf.len(),
            timestamp: Instant::now(),
            stdout: self.stdout,
        });

        Ok(len)
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

/// Iterator over the lines for a [`TestOutput`]
pub struct LinesIterator<'acc> {
    acc: &'acc TestOutput,
    cur_chunk: usize,
    chunk_iter: Option<ChunkIterator<'acc>>,
    stdout_newline: bool,
    stderr_newline: bool,
}

impl<'acc> LinesIterator<'acc> {
    fn new(acc: &'acc TestOutput) -> Self {
        let mut this = Self {
            acc,
            cur_chunk: 0,
            chunk_iter: None,
            stdout_newline: true,
            stderr_newline: true,
        };

        this.advance(0);
        this
    }

    fn advance(&mut self, chunk_ind: usize) {
        let Some(chunk) = self.acc.chunks.get(chunk_ind) else {
            self.chunk_iter = None;
            return;
        };

        let ewnl = if chunk.stdout {
            &mut self.stdout_newline
        } else {
            &mut self.stderr_newline
        };
        self.chunk_iter = Some(ChunkIterator::new(&self.acc.buf, chunk, !*ewnl));
        self.cur_chunk = chunk_ind;
        *ewnl = self.acc.buf[chunk.range.clone()].ends_with(&[LF]);
    }
}

/// The [`Line`] kind, which can help consumers processing lines
#[derive(Copy, Clone, PartialEq)]
#[cfg_attr(test, derive(Debug))]
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

/// A single line of output for a test. Note that the linefeed (`\n`) is present
/// in the raw data
pub struct Line<'acc> {
    /// The parent chunk which this line is a subslice of
    pub chunk: &'acc OutputChunk,
    /// The raw data for this line entry
    pub raw: &'acc [u8],
    /// The line kind
    pub kind: LineKind,
}

impl<'acc> Line<'acc> {
    /// Gets the lossy string for the raw data
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

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_str_eq;
    use std::fmt::Write as _;

    macro_rules! wb {
        ($o:expr, $b:expr) => {
            $o.write($b).unwrap();
        };
    }

    /// Basic test for getting the combined, stream specific, and individual lines
    /// from a [`TestOutput`]
    #[test]
    fn normal_failure_output() {
        let mut acc = TestOutputAccumulator::new();

        wb!(acc.stdout(), b"\nrunning 1 test\n");

        const TEST_OUTPUT: &[&str] = &[
            "thread 'normal_failing' panicked at tests/path.rs:44:10:",
            "called `Result::unwrap()` on an `Err` value: oops",
            "stack bactrace:",
            "   0: rust_begin_unwind",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/std/src/panicking.rs:597:5",
            "   1: core::panicking::panic_fmt",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/panicking.rs:72:14",
            "   2: core::result::unwrap_failed",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/result.rs:1652:5",
            "   3: core::result::Result<T,E>::unwrap",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/result.rs:1077:23",
            "   4: path::load",
            "             at ./tests/path.rs:39:9",
            "   5: path::normal_failing",
            "             at ./tests/path.rs:224:35",
            "   6: path::normal_failing::{{closure}}",
            "             at ./tests/path.rs:223:30",
            "   7: core::ops::function::FnOnce::call_once",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/ops/function.rs:250:5",
            "   8: core::ops::function::FnOnce::call_once",
            "             at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/ops/function.rs:250:5",
            "note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.",
        ];

        {
            let err = &mut acc.stderr();
            for line in &TEST_OUTPUT[..2] {
                err.write_vectored(&[
                    std::io::IoSlice::new(line.as_bytes()),
                    std::io::IoSlice::new(b"\n"),
                ])
                .unwrap();
            }

            let mut backtrace_chunk = String::new();
            for line in &TEST_OUTPUT[2..TEST_OUTPUT.len() - 1] {
                backtrace_chunk.push_str(line);
                backtrace_chunk.push('\n');
            }

            err.write(backtrace_chunk.as_bytes()).unwrap();
            err.write_vectored(&[
                std::io::IoSlice::new(TEST_OUTPUT[TEST_OUTPUT.len() - 1].as_bytes()),
                std::io::IoSlice::new(b"\n"),
            ])
            .unwrap();
        }

        wb!(acc.stdout(), b"test normal_failing ... FAILED\n");

        let test_output = acc.freeze();

        assert_str_eq!(
            test_output.stdout_lossy(),
            "\nrunning 1 test\ntest normal_failing ... FAILED\n"
        );
        assert_str_eq!(test_output.stderr_lossy(), {
            let mut to = TEST_OUTPUT.join("\n");
            to.push('\n');
            to
        });

        {
            let mut combined = String::new();
            writeln!(&mut combined, "\nrunning 1 test").unwrap();

            for line in TEST_OUTPUT {
                combined.push_str(line);
                combined.push('\n');
            }

            writeln!(&mut combined, "test normal_failing ... FAILED").unwrap();

            assert_str_eq!(combined, test_output.lossy());
        }

        let mut lines = test_output.lines();

        assert_str_eq!(lines.next().unwrap().lossy(), "\n");
        assert_str_eq!(lines.next().unwrap().lossy(), "running 1 test\n");

        for expected in TEST_OUTPUT {
            let actual = lines.next().unwrap();

            assert_str_eq!(*expected, {
                let mut lossy = actual.lossy().to_string();
                lossy.pop();
                lossy
            });

            assert_eq!(actual.kind, LineKind::Complete);
            assert!(!actual.chunk.stdout);
        }

        assert_str_eq!(
            lines.next().unwrap().lossy(),
            "test normal_failing ... FAILED\n"
        );
        assert!(lines.next().is_none());
    }

    /// Tests that "split output" ie, output that is either excessively long and
    /// could not be written in an individual write syscall, or even "non-typical"
    /// user code that did unbuffered writes without flushing, possibly from multiple
    /// threads, causing stdout and stderr output to be mixed together
    #[test]
    fn split_output() {
        let mut acc = TestOutputAccumulator::new();

        const CHUNKS: &[(bool, LineKind, &str)] = &[
            // Normal writes
            (true, LineKind::Complete, "stdout line\n"),
            (false, LineKind::Complete, "stderr line\n"),
            // Writes that are split over multiple writes, but still represent a
            // contiguous stream
            (true, LineKind::Begin, "stdout begin..."),
            (true, LineKind::None, "..."),
            (true, LineKind::End, "...stdout end\n"),
            (false, LineKind::Begin, "stderr begin..."),
            (false, LineKind::None, "..."),
            (false, LineKind::End, "...stderr end\n"),
            // Writes that are split over multiple writes, but interspersed
            (true, LineKind::Begin, "stdout begin..."),
            (false, LineKind::Begin, "stderr begin..."),
            (false, LineKind::None, "..."),
            (true, LineKind::None, "..."),
            (true, LineKind::None, "...\n..."),
            (false, LineKind::None, "...\n..."),
            (false, LineKind::End, "...stderr end\n"),
            (true, LineKind::End, "...stdout end\n"),
            // Normal writes
            (true, LineKind::Complete, "stdout boop\nstdout end\n"),
            (false, LineKind::Complete, "stderr boop\nstderr end\n"),
        ];

        for (stdout, _, chunk) in CHUNKS {
            push_chunk(&mut acc, chunk.as_bytes(), *stdout);
        }

        let to = acc.freeze();

        {
            let mut combined = String::new();
            for (_, _, chunk) in CHUNKS {
                combined.push_str(chunk);
            }

            assert_str_eq!(combined, to.lossy());
        }

        let mut lines = to.lines();

        let timestamp = Instant::now();

        for (stdout, kind, data) in CHUNKS {
            let chunk = OutputChunk {
                stdout: *stdout,
                range: 0..data.len(),
                timestamp,
            };

            for expected in ChunkIterator::new(
                data.as_bytes(),
                &chunk,
                matches!(kind, LineKind::None | LineKind::End),
            ) {
                let line = lines.next().unwrap();
                assert_str_eq!(line.lossy(), expected.lossy());
                assert_eq!(line.kind, expected.kind, "{data}");
                assert_eq!(line.chunk.stdout, *stdout, "{data}");
            }
        }
    }
}
