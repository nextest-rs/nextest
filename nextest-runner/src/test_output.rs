//! Utilities for capture output from tests run in a child process

use bytes::{Bytes, BytesMut};
use std::io::Write as _;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt};

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

/// Collects the stdout and/or stderr streams into a single buffer
#[allow(clippy::needless_lifetimes, clippy::let_and_return)]
pub fn collect_test_output<'a>(
    mut stdout: Option<tokio::process::ChildStdout>,
    mut stderr: Option<tokio::process::ChildStderr>,
    acc: &'a mut TestOutputAccumulator,
) -> impl futures::Future<Output = Result<(), crate::errors::CollectTestOutputError>> + 'a {
    let read_loop = async move {
        loop {
            if let Some(so) = &mut stdout {
                if read_chunk(acc, so, true)
                    .await
                    .map_err(crate::errors::CollectTestOutputError::ReadStdout)?
                {
                    read_to_end(acc, so, true)
                        .await
                        .map_err(crate::errors::CollectTestOutputError::ReadStdout)?;
                    stdout.take();
                }
            }

            if let Some(se) = &mut stderr {
                if read_chunk(acc, se, false)
                    .await
                    .map_err(crate::errors::CollectTestOutputError::ReadStderr)?
                {
                    read_to_end(acc, se, false)
                        .await
                        .map_err(crate::errors::CollectTestOutputError::ReadStderr)?;
                    stderr.take();
                }
            }

            if stdout.is_none() && stderr.is_none() {
                break;
            } else {
                // We're polling to see if there is data and thus sidestepping
                // the async reads if there is none, which can cause tokio to
                // deadlock by not giving time to other async tasks, so do a
                // yield to keep it happy
                tokio::task::yield_now().await;
            }
        }

        Ok(())
    };

    read_loop
}

#[cfg(unix)]
async fn read_chunk<S>(
    acc: &mut TestOutputAccumulator,
    output: &mut S,
    stdout: bool,
) -> std::io::Result<bool>
where
    S: AsyncRead + std::os::fd::AsRawFd + Unpin + Send,
{
    let fd = output.as_raw_fd();

    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    // Poll the pipe to see if there is data to read, or the other end has closed
    // SAFETY: syscall
    let res = unsafe { libc::poll(&mut pfd, 1, 0) };

    match res {
        // Timed out before the pipe could be ready
        0 => Ok(false),
        1 => {
            if (pfd.revents & libc::POLLIN) != 0 {
                // Get the amount of data we can read from the pipe
                let mut num_bytes: u32 = 0; // note technically this is an i32, but...

                // NOTE: this might not work on some unixes as it's not, strictly speaking,
                // a standardized way of checking for pending data
                // SAFETY: syscall
                if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut num_bytes as *mut u32) } == 0 {
                    num_bytes
                } else {
                    let last = std::io::Error::last_os_error();
                    return if matches!(
                        last.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::BrokenPipe
                    ) {
                        Ok(true)
                    } else {
                        Err(last)
                    };
                };

                // Ignore spurious readiness
                if num_bytes == 0 {
                    return Ok(false);
                }

                do_read(acc, output, stdout, num_bytes as usize).await?;
            }

            // If write end of the pipe has closed, we can stop polling it
            Ok((pfd.revents & libc::POLLHUP) != 0)
        }
        _ => {
            let last = std::io::Error::last_os_error();
            if matches!(
                last.kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::BrokenPipe
            ) {
                Ok(true)
            } else {
                Err(last)
            }
        }
    }
}

#[cfg(windows)]
async fn read_chunk<S>(
    acc: &mut TestOutputAccumulator,
    output: &mut S,
    stdout: bool,
) -> std::io::Result<bool>
where
    S: AsyncRead + std::os::windows::io::AsRawHandle + Unpin + Send,
{
    // NOTE: we need to put this in a separate block since as_raw_handle()
    // returns a pointer and makes the .await at the bottom unable to be compiled
    // since they are non-Send
    let num_bytes = {
        let handle = output.as_raw_handle();

        // Check if the pipe has pending data
        let mut num_bytes = 0;

        // SAFETY: syscall
        if unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                handle as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut num_bytes,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let err = std::io::Error::last_os_error();

            return if err.kind() == std::io::ErrorKind::BrokenPipe {
                Ok(true)
            } else {
                Err(err)
            };
        }

        num_bytes
    };

    if num_bytes == 0 {
        return Ok(false);
    }

    do_read(acc, output, stdout, num_bytes as usize).await
}

/// Perform the actual read now that we know how much data is pending
async fn do_read<S>(
    acc: &mut TestOutputAccumulator,
    output: &mut S,
    stdout: bool,
    num_bytes: usize,
) -> std::io::Result<bool>
where
    S: AsyncRead + Unpin + Send,
{
    let start = acc.buf.len();

    // We allocate in 4k chunks, so avoid if we already have space
    if acc.buf.capacity() - start < num_bytes {
        const CHUNK_SIZE: usize = 4 * 1024;
        let chunk_size = if num_bytes > CHUNK_SIZE {
            CHUNK_SIZE * (num_bytes / CHUNK_SIZE + 1)
        } else {
            CHUNK_SIZE
        };

        acc.buf.reserve(chunk_size);
    }

    let timestamp = Instant::now();
    let read = output.read_buf(&mut acc.buf).await?;

    if read == 0 {
        return Ok(true);
    }

    let end = acc.buf.len();

    acc.chunks.push(OutputChunk {
        range: start..end,
        timestamp,
        stdout,
    });

    Ok(false)
}

async fn read_to_end<S>(
    acc: &mut TestOutputAccumulator,
    output: &mut S,
    stdout: bool,
) -> std::io::Result<()>
where
    S: AsyncRead + Unpin + Send,
{
    while !do_read(acc, output, stdout, 1024).await? {}

    Ok(())
}
