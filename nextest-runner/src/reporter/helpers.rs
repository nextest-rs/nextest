// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::runner::{AbortStatus, ExecutionResult};
use bstr::ByteSlice;
use once_cell::sync::Lazy;
use regex::bytes::{Regex, RegexBuilder};
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

    /// A panic message was found in the output.
    ///
    /// The output is borrowed from standard error.
    PanicMessage {
        /// The subslice of standard error that contains the stack trace.
        stderr_subslice: ByteSubslice<'a>,
    },

    /// An error string was found in the output.
    ///
    /// The output is borrowed from standard error.
    ErrorStr {
        /// The subslice of standard error that contains the stack trace.
        stderr_subslice: ByteSubslice<'a>,
    },

    /// A should-panic test did not panic.
    ///
    /// The output is borrowed from standard output.
    ShouldPanic {
        /// The subslice of standard output that contains the should-panic message.
        stdout_subslice: ByteSubslice<'a>,
    },
}

impl<'a> DescriptionKind<'a> {
    /// Returns the subslice of standard error that contains the description.
    pub fn stderr_subslice(&self) -> Option<ByteSubslice<'a>> {
        match self {
            DescriptionKind::Abort { .. } => None,
            DescriptionKind::PanicMessage { stderr_subslice }
            | DescriptionKind::ErrorStr {
                stderr_subslice, ..
            } => Some(*stderr_subslice),
            DescriptionKind::ShouldPanic { .. } => None,
        }
    }

    /// Returns the subslice of standard output that contains the description.
    pub fn stdout_subslice(&self) -> Option<ByteSubslice<'a>> {
        match self {
            DescriptionKind::Abort { .. } => None,
            DescriptionKind::PanicMessage { .. } => None,
            DescriptionKind::ErrorStr { .. } => None,
            DescriptionKind::ShouldPanic {
                stdout_subslice, ..
            } => Some(*stdout_subslice),
        }
    }

    /// Returns the subslice of combined output (either stdout or stderr) that contains the description.
    pub fn combined_subslice(&self) -> Option<ByteSubslice<'a>> {
        match self {
            DescriptionKind::Abort { .. } => None,
            DescriptionKind::PanicMessage { stderr_subslice }
            | DescriptionKind::ErrorStr {
                stderr_subslice, ..
            } => Some(*stderr_subslice),
            DescriptionKind::ShouldPanic {
                stdout_subslice, ..
            } => Some(*stdout_subslice),
        }
    }

    /// Displays the description as a user-friendly string.
    pub fn display_human(&self) -> DescriptionKindDisplay<'_> {
        DescriptionKindDisplay(*self)
    }
}

/// A subslice of a byte slice.
///
/// This type tracks the start index of the subslice from the parent slice.
#[derive(Clone, Copy, Debug)]
pub struct ByteSubslice<'a> {
    /// The slice.
    pub slice: &'a [u8],

    /// The start index of the subslice from the parent slice.
    pub start: usize,
}

/// A display wrapper for [`DescriptionKind`].
#[derive(Clone, Copy, Debug)]
pub struct DescriptionKindDisplay<'a>(DescriptionKind<'a>);

impl fmt::Display for DescriptionKindDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DescriptionKind::Abort { status, leaked } => {
                write!(f, "Test aborted")?;
                match status {
                    #[cfg(unix)]
                    AbortStatus::UnixSignal(sig) => {
                        let signal_str = crate::helpers::signal_str(sig)
                            .map(|signal_str| format!("SIG{signal_str}"))
                            .unwrap_or_else(|| sig.to_string());
                        write!(f, " with signal {signal_str}")?;
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
            DescriptionKind::PanicMessage { stderr_subslice } => {
                // Strip invalid XML characters.
                write!(f, "{}", String::from_utf8_lossy(stderr_subslice.slice))
            }
            DescriptionKind::ErrorStr { stderr_subslice } => {
                write!(f, "{}", String::from_utf8_lossy(stderr_subslice.slice))
            }
            DescriptionKind::ShouldPanic { stdout_subslice } => {
                write!(f, "{}", String::from_utf8_lossy(stdout_subslice.slice))
            }
        }
    }
}

/// Attempts to heuristically extract a description of the test failure from the output of the test.
pub fn heuristic_extract_description<'a>(
    exec_result: ExecutionResult,
    stdout: &'a [u8],
    stderr: &'a [u8],
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
    if let Some(stderr_subslice) = heuristic_panic_message(stderr) {
        return Some(DescriptionKind::PanicMessage { stderr_subslice });
    }
    if let Some(stderr_subslice) = heuristic_error_str(stderr) {
        return Some(DescriptionKind::ErrorStr { stderr_subslice });
    }
    if let Some(stdout_subslice) = heuristic_should_panic(stdout) {
        return Some(DescriptionKind::ShouldPanic { stdout_subslice });
    }

    None
}

fn heuristic_should_panic(stdout: &[u8]) -> Option<ByteSubslice<'_>> {
    let line = stdout
        .lines()
        .find(|line| line.contains_str("note: test did not panic as expected"))?;

    // SAFETY: line is a subslice of stdout.
    let start = unsafe { line.as_ptr().offset_from(stdout.as_ptr()) };

    let start = usize::try_from(start).unwrap_or_else(|error| {
        panic!(
            "negative offset from stdout.as_ptr() ({:x}) to line.as_ptr() ({:x}): {}",
            stdout.as_ptr() as usize,
            line.as_ptr() as usize,
            error
        )
    });
    Some(ByteSubslice { slice: line, start })
}

fn heuristic_panic_message(stderr: &[u8]) -> Option<ByteSubslice<'_>> {
    // Look for the last instance to handle situations like proptest which repeatedly print out
    // `panicked at ...` messages.
    let panicked_at_match = PANICKED_AT_REGEX.find_iter(stderr).last()?;
    // If the previous line starts with "Error: ", grab it as well -- it contains the error with
    // result-based test failures.
    let mut start = panicked_at_match.start();
    let prefix = stderr[..start].trim_end_with(|c| c == '\n' || c == '\r');
    if let Some(prev_line_start) = prefix.rfind("\n") {
        if prefix[prev_line_start..].starts_with_str("\nError:") {
            start = prev_line_start + 1;
        }
    }

    // TODO: this grabs too much -- it is possible that destructors print out further messages so we
    // should be more careful. But it's hard to tell what's printed by the panic and what's printed
    // by destructors, so we lean on the side of caution.
    Some(ByteSubslice {
        slice: stderr[start..].trim_end_with(|c| c.is_whitespace()),
        start,
    })
}

fn heuristic_error_str(stderr: &[u8]) -> Option<ByteSubslice<'_>> {
    // Starting Rust 1.66, Result-based errors simply print out "Error: ".
    let error_match = ERROR_REGEX.find(stderr)?;
    let start = error_match.start();

    // TODO: this grabs too much -- it is possible that destructors print out further messages so we
    // should be more careful. But it's hard to tell what's printed by the error and what's printed
    // by destructors, so we lean on the side of caution.
    Some(ByteSubslice {
        slice: stderr[start..].trim_end_with(|c| c.is_whitespace()),
        start,
    })
}

/// Given a slice, find the index of the point at which highlighting should end.
///
/// Returns a value in the range [0, slice.len()].
pub fn highlight_end(slice: &[u8]) -> usize {
    // We want to highlight the first two lines of the output.
    let mut iter = slice.find_iter(b"\n");
    match iter.next() {
        Some(_) => {
            match iter.next() {
                Some(second) => second,
                // No second newline found, so highlight the entire slice.
                None => slice.len(),
            }
        }
        // No newline found, so highlight the entire slice.
        None => slice.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_should_panic() {
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
            let extracted = heuristic_should_panic(input.as_bytes())
                .expect("should-panic message should have been found");
            assert_eq!(
                DisplayWrapper(extracted.slice),
                DisplayWrapper(output.as_bytes())
            );
            assert_eq!(
                extracted.start,
                extracted.slice.as_ptr() as usize - input.as_bytes().as_ptr() as usize
            );
        }
    }

    #[test]
    fn test_heuristic_panic_message() {
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
more text at the end, followed by some newlines


            "#,
                r#"Error: Custom { kind: InvalidData, error: "this is an error" }
thread 'test_result_failure' panicked at 'assertion failed: `(left == right)`
  left: `1`,
 right: `0`: the test returned a termination value with a non-zero status code (1) which indicates a failure', /rustc/fe5b13d681f25ee6474be29d748c65adcd91f69e/library/test/src/lib.rs:186:5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
more text at the end, followed by some newlines"#,
            ),
            // Multiple panics: only the last one should be extracted.
            (
                r#"
thread 'main' panicked at src/lib.rs:1:
foo
thread 'main' panicked at src/lib.rs:2:
bar
"#,
                r#"thread 'main' panicked at src/lib.rs:2:
bar"#,
            ), // With RUST_BACKTRACE=1
            (
                r#"
some initial text
line 2
line 3
thread 'reporter::helpers::tests::test_heuristic_stack_trace' panicked at nextest-runner/src/reporter/helpers.rs:237:9:
test
stack backtrace:
   0: rust_begin_unwind
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/panicking.rs:652:5
   1: core::panicking::panic_fmt
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/panicking.rs:72:14
   2: nextest_runner::reporter::helpers::tests::test_heuristic_stack_trace
             at ./src/reporter/helpers.rs:237:9
   3: nextest_runner::reporter::helpers::tests::test_heuristic_stack_trace::{{closure}}
             at ./src/reporter/helpers.rs:236:36
   4: core::ops::function::FnOnce::call_once
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/ops/function.rs:250:5
   5: core::ops::function::FnOnce::call_once
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/ops/function.rs:250:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
more text at the end, followed by some newlines


"#,
                r#"thread 'reporter::helpers::tests::test_heuristic_stack_trace' panicked at nextest-runner/src/reporter/helpers.rs:237:9:
test
stack backtrace:
   0: rust_begin_unwind
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/panicking.rs:652:5
   1: core::panicking::panic_fmt
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/panicking.rs:72:14
   2: nextest_runner::reporter::helpers::tests::test_heuristic_stack_trace
             at ./src/reporter/helpers.rs:237:9
   3: nextest_runner::reporter::helpers::tests::test_heuristic_stack_trace::{{closure}}
             at ./src/reporter/helpers.rs:236:36
   4: core::ops::function::FnOnce::call_once
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/ops/function.rs:250:5
   5: core::ops::function::FnOnce::call_once
             at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/ops/function.rs:250:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
more text at the end, followed by some newlines"#,
            ),
            // RUST_BACKTRACE=full
            (
                r#"
some initial text
thread 'reporter::helpers::tests::test_heuristic_stack_trace' panicked at nextest-runner/src/reporter/helpers.rs:237:9:
test
stack backtrace:
   0:     0x61e6da135fe5 - std::backtrace_rs::backtrace::libunwind::trace::h23054e327d0d4b55
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/../../backtrace/src/backtrace/libunwind.rs:116:5
   1:     0x61e6da135fe5 - std::backtrace_rs::backtrace::trace_unsynchronized::h0cc587407d7f7f64
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/../../backtrace/src/backtrace/mod.rs:66:5
   2:     0x61e6da135fe5 - std::sys_common::backtrace::_print_fmt::h4feeb59774730d6b
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/sys_common/backtrace.rs:68:5
   3:     0x61e6da135fe5 - <std::sys_common::backtrace::_print::DisplayBacktrace as core::fmt::Display>::fmt::hd736fd5964392270
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/sys_common/backtrace.rs:44:22
   4:     0x61e6da16433b - core::fmt::rt::Argument::fmt::h105051d8ea1ade1e
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/fmt/rt.rs:165:63
   5:     0x61e6da16433b - core::fmt::write::hc6043626647b98ea
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/fmt/mod.rs:1168:21
some more text at the end, followed by some newlines


"#,
                r#"thread 'reporter::helpers::tests::test_heuristic_stack_trace' panicked at nextest-runner/src/reporter/helpers.rs:237:9:
test
stack backtrace:
   0:     0x61e6da135fe5 - std::backtrace_rs::backtrace::libunwind::trace::h23054e327d0d4b55
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/../../backtrace/src/backtrace/libunwind.rs:116:5
   1:     0x61e6da135fe5 - std::backtrace_rs::backtrace::trace_unsynchronized::h0cc587407d7f7f64
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/../../backtrace/src/backtrace/mod.rs:66:5
   2:     0x61e6da135fe5 - std::sys_common::backtrace::_print_fmt::h4feeb59774730d6b
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/sys_common/backtrace.rs:68:5
   3:     0x61e6da135fe5 - <std::sys_common::backtrace::_print::DisplayBacktrace as core::fmt::Display>::fmt::hd736fd5964392270
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/std/src/sys_common/backtrace.rs:44:22
   4:     0x61e6da16433b - core::fmt::rt::Argument::fmt::h105051d8ea1ade1e
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/fmt/rt.rs:165:63
   5:     0x61e6da16433b - core::fmt::write::hc6043626647b98ea
                               at /rustc/3f5fd8dd41153bc5fdca9427e9e05be2c767ba23/library/core/src/fmt/mod.rs:1168:21
some more text at the end, followed by some newlines"#,
            ),
        ];

        for (input, output) in tests {
            let extracted = heuristic_panic_message(input.as_bytes())
                .expect("stack trace should have been found");
            assert_eq!(
                DisplayWrapper(extracted.slice),
                DisplayWrapper(output.as_bytes())
            );
            assert_eq!(
                extracted.start,
                extracted.slice.as_ptr() as usize - input.as_bytes().as_ptr() as usize
            );
        }
    }

    #[test]
    fn test_heuristic_error_str() {
        let tests: &[(&str, &str)] = &[(
            "foobar\nError: \"this is an error\"\n",
            "Error: \"this is an error\"",
        )];

        for (input, output) in tests {
            let extracted =
                heuristic_error_str(input.as_bytes()).expect("error string should have been found");
            assert_eq!(
                DisplayWrapper(extracted.slice),
                DisplayWrapper(output.as_bytes())
            );
            assert_eq!(
                extracted.start,
                extracted.slice.as_ptr() as usize - input.as_bytes().as_ptr() as usize
            );
        }
    }

    #[test]
    fn test_highlight_end() {
        let tests: &[(&str, usize)] = &[
            ("", 0),
            ("\n", 1),
            ("foo", 3),
            ("foo\n", 4),
            ("foo\nbar", 7),
            ("foo\nbar\n", 7),
            ("foo\nbar\nbaz", 7),
            ("foo\nbar\nbaz\n", 7),
            ("\nfoo\nbar\nbaz", 4),
        ];

        for (input, output) in tests {
            assert_eq!(
                highlight_end(input.as_bytes()),
                *output,
                "for input {input:?}"
            );
        }
    }

    // Wrapper so that panic messages show up nicely in the test output.
    #[derive(Eq, PartialEq)]
    struct DisplayWrapper<'a>(&'a [u8]);

    impl<'a> fmt::Debug for DisplayWrapper<'a> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", String::from_utf8_lossy(self.0))
        }
    }
}
