// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::runner::{AbortStatus, ExecutionResult};
use once_cell::sync::Lazy;
use quick_junit::Output;
use regex::{Regex, RegexBuilder};
use std::fmt;

// This regex works for the default panic handler for Rust -- other panic handlers may not work,
// which is why this is heuristic.
static PANICKED_AT_REGEX_STR: &str = "^thread '([^']+)' panicked at ";
static PANICKED_AT_REGEX: Lazy<Regex> = Lazy::new(|| {
    let mut builder = RegexBuilder::new(PANICKED_AT_REGEX_STR);
    builder.multi_line(true);
    builder.build().unwrap()
});

static ERROR_REGEX_STR: &str = "^Error: ";
static ERROR_REGEX: Lazy<Regex> = Lazy::new(|| {
    let mut builder = RegexBuilder::new(ERROR_REGEX_STR);
    builder.multi_line(true);
    builder.build().unwrap()
});

/// The return result of [`heuristic_extract_description`].
#[derive(Clone, Copy, Debug)]
pub enum DescriptionKind<'a> {
    /// This was some kind of abort.
    Abort {
        /// The reason for the abort.
        status: AbortStatus,
        /// Whether the test leaked handles.
        leaked: bool,
    },

    /// A stack trace was found in the output.
    ///
    /// The output is borrowed from standard error.
    StackTrace {
        /// The stack trace as a substring of the standard error.
        stderr_output: &'a str,
    },

    /// An error string was found in the output.
    ///
    /// The output is borrowed from standard error.
    ErrorStr {
        /// The error string as a substring of the standard error.
        stderr_output: &'a str,
    },

    /// A should-panic test did not panic.
    ///
    /// The output is borrowed from standard output.
    ShouldPanic {
        /// The should-panic of the test as a substring of the standard output.
        stdout_output: &'a str,
    },
}

impl DescriptionKind<'_> {
    /// Displays the description as a user-friendly string.
    pub fn display_human(&self) -> DescriptionKindDisplay<'_> {
        DescriptionKindDisplay(*self)
    }
}

/// A display wrapper for [`DescriptionKind`].
#[derive(Clone, Copy, Debug)]
pub struct DescriptionKindDisplay<'a>(DescriptionKind<'a>);

impl<'a> DescriptionKindDisplay<'a> {
    /// Returns the displayer in a JUnit-compatible format.
    ///
    /// This format filters out invalid XML characters.
    pub fn to_junit_output(self) -> Output {
        Output::new(self.to_string())
    }
}

impl fmt::Display for DescriptionKindDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DescriptionKind::Abort { status, leaked } => {
                write!(f, "Test aborted")?;
                match status {
                    #[cfg(unix)]
                    AbortStatus::UnixSignal(sig) => {
                        let signal_str = crate::helpers::signal_str(sig)
                            .map(|signal_str| format!("SIG{}", signal_str))
                            .unwrap_or_else(|| sig.to_string());
                        write!(f, " with signal {}", signal_str)?;
                    }
                    #[cfg(windows)]
                    AbortStatus::WindowsNtStatus(exception) => {
                        write!(
                            f,
                            " with code {}",
                            crate::helpers::display_nt_status(exception)
                        )?;
                    }
                }
                if leaked {
                    write!(f, ", and also leaked handles")?;
                }
                Ok(())
            }
            DescriptionKind::StackTrace { stderr_output } => {
                write!(f, "{}", stderr_output)
            }
            DescriptionKind::ErrorStr { stderr_output } => {
                write!(f, "{}", stderr_output)
            }
            DescriptionKind::ShouldPanic { stdout_output } => {
                write!(f, "{}", stdout_output)
            }
        }
    }
}

/// Attempts to heuristically extract a description of the test failure from the output of the test.
pub fn heuristic_extract_description<'a>(
    exec_result: ExecutionResult,
    stdout: &'a str,
    stderr: &'a str,
) -> Option<DescriptionKind<'a>> {
    // If the test crashed with a signal, use that.
    if let ExecutionResult::Fail {
        abort_status: Some(status),
        leaked,
    } = exec_result
    {
        return Some(DescriptionKind::Abort { status, leaked });
    }

    // Try the heuristic stack trace extraction first to try and grab more information first.
    if let Some(stderr_output) = heuristic_stack_trace(stderr) {
        return Some(DescriptionKind::StackTrace { stderr_output });
    }
    if let Some(stderr_output) = heuristic_error_str(stderr) {
        return Some(DescriptionKind::ErrorStr { stderr_output });
    }
    if let Some(stdout_output) = heuristic_should_panic(stdout) {
        return Some(DescriptionKind::ShouldPanic { stdout_output });
    }

    None
}

fn heuristic_should_panic(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .find(|line| line.contains("note: test did not panic as expected"))
}

fn heuristic_stack_trace(stderr: &str) -> Option<&str> {
    let panicked_at_match = PANICKED_AT_REGEX.find(stderr)?;
    // If the previous line starts with "Error: ", grab it as well -- it contains the error with
    // result-based test failures.
    let mut start = panicked_at_match.start();
    let prefix = stderr[..start].trim_end_matches('\n');
    if let Some(prev_line_start) = prefix.rfind('\n') {
        if prefix[prev_line_start..].starts_with("\nError:") {
            start = prev_line_start + 1;
        }
    }

    Some(stderr[start..].trim_end())
}

fn heuristic_error_str(stderr: &str) -> Option<&str> {
    // Starting Rust 1.66, Result-based errors simply print out "Error: ".
    let error_match = ERROR_REGEX.find(stderr)?;
    let start = error_match.start();
    Some(stderr[start..].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_extract_description() {
        let tests: &[(&str, &str)] = &[(
            "running 1 test
test test_failure_should_panic - should panic ... FAILED

failures:

---- test_failure_should_panic stdout ----
note: test did not panic as expected

failures:
    test_failure_should_panic

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 13 filtered out; finished in 0.00s",
            "note: test did not panic as expected",
        )];

        for (input, output) in tests {
            assert_eq!(heuristic_should_panic(input), Some(*output));
        }
    }

    #[test]
    fn test_heuristic_stack_trace() {
        let tests: &[(&str, &str)] = &[
            (
                "thread 'main' panicked at 'foo', src/lib.rs:1\n",
                "thread 'main' panicked at 'foo', src/lib.rs:1",
            ),
            (
                "foobar\n\
            thread 'main' panicked at 'foo', src/lib.rs:1\n\n",
                "thread 'main' panicked at 'foo', src/lib.rs:1",
            ),
            (
                r#"
text: foo
Error: Custom { kind: InvalidData, error: "this is an error" }
thread 'test_result_failure' panicked at 'assertion failed: `(left == right)`
  left: `1`,
 right: `0`: the test returned a termination value with a non-zero status code (1) which indicates a failure', /rustc/fe5b13d681f25ee6474be29d748c65adcd91f69e/library/test/src/lib.rs:186:5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
            "#,
                r#"Error: Custom { kind: InvalidData, error: "this is an error" }
thread 'test_result_failure' panicked at 'assertion failed: `(left == right)`
  left: `1`,
 right: `0`: the test returned a termination value with a non-zero status code (1) which indicates a failure', /rustc/fe5b13d681f25ee6474be29d748c65adcd91f69e/library/test/src/lib.rs:186:5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"#,
            ),
        ];

        for (input, output) in tests {
            assert_eq!(heuristic_stack_trace(input), Some(*output));
        }
    }

    #[test]
    fn test_heuristic_error_str() {
        let tests: &[(&str, &str)] = &[(
            "foobar\nError: \"this is an error\"\n",
            "Error: \"this is an error\"",
        )];

        for (input, output) in tests {
            assert_eq!(heuristic_error_str(input), Some(*output));
        }
    }
}
