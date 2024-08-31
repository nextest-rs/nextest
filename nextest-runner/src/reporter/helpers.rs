// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::runner::{AbortStatus, ExecutionResult};
use once_cell::sync::Lazy;
use quick_junit::Output;
use regex::{Regex, RegexBuilder};

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

#[allow(unused_variables)]
/// Not part of the public API: only used for testing.
#[doc(hidden)]
pub fn heuristic_extract_description<'a>(
    exec_result: ExecutionResult,
    stdout: &'a str,
    stderr: &'a str,
) -> Option<String> {
    // If the test crashed with a signal, use that.
    #[cfg(unix)]
    if let ExecutionResult::Fail {
        abort_status: Some(AbortStatus::UnixSignal(sig)),
        leaked,
    } = exec_result
    {
        let signal_str = match crate::helpers::signal_str(sig) {
            Some(signal_str) => format!(" SIG{signal_str}"),
            None => String::new(),
        };
        return Some(format!(
            "Test aborted with signal{signal_str} (code {sig}){}",
            if leaked {
                ", and also leaked handles"
            } else {
                ""
            }
        ));
    }

    #[cfg(windows)]
    if let ExecutionResult::Fail {
        abort_status: Some(AbortStatus::WindowsNtStatus(exception)),
        leaked,
    } = exec_result
    {
        return Some(format!(
            "Test aborted with code {}{}",
            crate::helpers::display_nt_status(exception),
            if leaked {
                ", and also leaked handles"
            } else {
                ""
            }
        ));
    }

    // Try the heuristic stack trace extraction first as they're the more common kinds of test.
    if let Some(description) = heuristic_stack_trace(stderr) {
        return Some(description);
    }
    if let Some(description) = heuristic_error_str(stderr) {
        return Some(description);
    }
    heuristic_should_panic(stdout)
}

fn heuristic_should_panic(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if line.contains("note: test did not panic as expected") {
            return Some(Output::new(line).into_string());
        }
    }
    None
}

fn heuristic_stack_trace(stderr: &str) -> Option<String> {
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

    Some(Output::new(stderr[start..].trim_end()).into_string())
}

fn heuristic_error_str(stderr: &str) -> Option<String> {
    // Starting Rust 1.66, Result-based errors simply print out "Error: ".
    let error_match = ERROR_REGEX.find(stderr)?;
    let start = error_match.start();
    Some(Output::new(stderr[start..].trim_end()).into_string())
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
            assert_eq!(heuristic_should_panic(input).as_deref(), Some(*output));
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
            assert_eq!(heuristic_stack_trace(input).as_deref(), Some(*output));
        }
    }

    #[test]
    fn test_heuristic_error_str() {
        let tests: &[(&str, &str)] = &[(
            "foobar\nError: \"this is an error\"\n",
            "Error: \"this is an error\"",
        )];

        for (input, output) in tests {
            assert_eq!(heuristic_error_str(input).as_deref(), Some(*output));
        }
    }
}
