//! Utilities for capture output from tests run in a child process

use crate::{
    reporter::{heuristic_extract_description, DescriptionKind},
    runner::ExecutionResult,
};
use bstr::{ByteSlice, Lines};
use bytes::Bytes;
use std::{borrow::Cow, sync::OnceLock};

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

/// A single output for a test or setup script: standard output, standard error, or a combined
/// buffer.
///
/// This is a wrapper around a [`Bytes`] that provides some convenience methods.
#[derive(Clone, Debug)]
pub struct ChildSingleOutput {
    /// The raw output buffer
    pub buf: Bytes,

    /// A string representation of the output, computed on first access.
    ///
    /// `None` means the output is valid UTF-8.
    as_str: OnceLock<Option<Box<str>>>,
}

impl From<Bytes> for ChildSingleOutput {
    #[inline]
    fn from(buf: Bytes) -> Self {
        Self {
            buf,
            as_str: OnceLock::new(),
        }
    }
}

impl ChildSingleOutput {
    /// Gets this output as a lossy UTF-8 string.
    #[inline]
    pub fn as_str_lossy(&self) -> &str {
        let s = self
            .as_str
            .get_or_init(|| match String::from_utf8_lossy(&self.buf) {
                // A borrowed string from `from_utf8_lossy` is always valid UTF-8. We can't store
                // the `Cow` directly because that would be a self-referential struct. (Well, we
                // could via a library like ouroboros, but that's really unnecessary.)
                Cow::Borrowed(_) => None,
                Cow::Owned(s) => Some(s.into_boxed_str()),
            });

        match s {
            Some(s) => s,
            // SAFETY: Immediately above, we've established that `None` means `buf` is valid UTF-8.
            None => unsafe { std::str::from_utf8_unchecked(&self.buf) },
        }
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

/// The complete captured output of a child process.
#[derive(Clone, Debug)]
pub enum TestExecutionOutput {
    /// The process was run and the output was captured.
    Output(TestOutput),

    /// There was an execution failure.
    ExecFail {
        /// A single-line message.
        message: String,

        /// The full description, including other errors, to print out.
        description: String,
    },
}

/// The output of a test.
///
/// Part of [`TestExecutionOutput`].
#[derive(Clone, Debug)]
pub enum TestOutput {
    /// The output was split into stdout and stderr.
    Split(ChildSplitOutput),

    /// The output was combined into stdout and stderr.
    Combined {
        /// The captured output.
        output: ChildSingleOutput,
    },
}

/// The output of a child process (test or setup script) with split stdout and stderr.
///
/// Part of [`TestOutput`], and can be used independently as well.
#[derive(Clone, Debug)]
pub struct ChildSplitOutput {
    /// The captured stdout, or `None` if the output was not captured.
    pub stdout: Option<ChildSingleOutput>,

    /// The captured stderr, or `None` if the output was not captured.
    pub stderr: Option<ChildSingleOutput>,
}

impl TestOutput {
    /// Attempts to extract a description of a test failure from the output of the test.
    pub fn heuristic_extract_description(
        &self,
        exec_result: ExecutionResult,
    ) -> Option<DescriptionKind<'_>> {
        match self {
            Self::Split(split) => {
                if let Some(kind) = heuristic_extract_description(
                    exec_result,
                    split.stdout.as_ref().map(|x| x.buf.as_ref()),
                    split.stderr.as_ref().map(|x| x.buf.as_ref()),
                ) {
                    return Some(kind);
                }
            }
            Self::Combined { output } => {
                // Pass in the same buffer for both stdout and stderr.
                if let Some(kind) =
                    heuristic_extract_description(exec_result, Some(&output.buf), Some(&output.buf))
                {
                    return Some(kind);
                }
            }
        }

        None
    }
}
