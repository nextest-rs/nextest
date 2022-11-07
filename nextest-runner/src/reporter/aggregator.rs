// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Metadata management.

#[cfg(any(unix, windows))]
use crate::runner::AbortStatus;
use crate::{
    config::{NextestJunitConfig, NextestProfile},
    errors::WriteEventError,
    list::TestInstance,
    reporter::TestEvent,
    runner::{ExecuteStatus, ExecutionDescription, ExecutionResult},
};
use camino::Utf8PathBuf;
use chrono::{DateTime, FixedOffset, Utc};
use debug_ignore::DebugIgnore;
use once_cell::sync::Lazy;
use quick_junit::{NonSuccessKind, Output, Report, TestCase, TestCaseStatus, TestRerun, TestSuite};
use regex::{Regex, RegexBuilder};
use std::{borrow::Cow, collections::HashMap, fs::File, time::SystemTime};

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct EventAggregator<'cfg> {
    store_dir: Utf8PathBuf,
    // TODO: log information in a JSONable report (converting that to XML later) instead of directly
    // writing it to XML
    junit: Option<MetadataJunit<'cfg>>,
}

impl<'cfg> EventAggregator<'cfg> {
    pub(crate) fn new(profile: &NextestProfile<'cfg>) -> Self {
        Self {
            store_dir: profile.store_dir().to_owned(),
            junit: profile.junit().map(MetadataJunit::new),
        }
    }

    pub(crate) fn write_event(&mut self, event: TestEvent<'cfg>) -> Result<(), WriteEventError> {
        if let Some(junit) = &mut self.junit {
            junit.write_event(event)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct MetadataJunit<'cfg> {
    config: NextestJunitConfig<'cfg>,
    test_suites: DebugIgnore<HashMap<&'cfg str, TestSuite>>,
}

impl<'cfg> MetadataJunit<'cfg> {
    fn new(config: NextestJunitConfig<'cfg>) -> Self {
        Self {
            config,
            test_suites: DebugIgnore(HashMap::new()),
        }
    }

    pub(crate) fn write_event(&mut self, event: TestEvent<'cfg>) -> Result<(), WriteEventError> {
        match event {
            TestEvent::RunStarted { .. } => {}
            TestEvent::TestStarted { .. } => {}
            TestEvent::TestSlow { .. } => {}
            TestEvent::TestAttemptFailedWillRetry { .. } | TestEvent::TestRetryStarted { .. } => {
                // Retries are recorded in TestFinished.
            }
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => {
                fn kind_ty(run_status: &ExecuteStatus) -> (NonSuccessKind, Cow<'static, str>) {
                    match run_status.result {
                        ExecutionResult::Fail {
                            abort_status: Some(_),
                            leaked: true,
                        } => (
                            NonSuccessKind::Failure,
                            "test abort with leaked handles".into(),
                        ),
                        ExecutionResult::Fail {
                            abort_status: Some(_),
                            leaked: false,
                        } => (NonSuccessKind::Failure, "test abort".into()),
                        ExecutionResult::Fail {
                            abort_status: None,
                            leaked: true,
                        } => (
                            NonSuccessKind::Failure,
                            "test failure with leaked handles".into(),
                        ),
                        ExecutionResult::Fail {
                            abort_status: None,
                            leaked: false,
                        } => (NonSuccessKind::Failure, "test failure".into()),
                        ExecutionResult::Timeout => {
                            (NonSuccessKind::Failure, "test timeout".into())
                        }
                        ExecutionResult::ExecFail => {
                            (NonSuccessKind::Error, "execution failure".into())
                        }
                        ExecutionResult::Leak => (
                            NonSuccessKind::Error,
                            "test passed but leaked handles".into(),
                        ),
                        ExecutionResult::Pass => {
                            unreachable!("this is a failure status")
                        }
                    }
                }

                let testsuite = self.testsuite_for(test_instance);

                let (mut testcase_status, main_status, reruns) = match run_statuses.describe() {
                    ExecutionDescription::Success { single_status } => {
                        (TestCaseStatus::success(), single_status, &[][..])
                    }
                    ExecutionDescription::Flaky {
                        last_status,
                        prior_statuses,
                    } => (TestCaseStatus::success(), last_status, prior_statuses),
                    ExecutionDescription::Failure {
                        first_status,
                        retries,
                        ..
                    } => {
                        let (kind, ty) = kind_ty(first_status);
                        let mut testcase_status = TestCaseStatus::non_success(kind);
                        testcase_status.set_type(ty);
                        (testcase_status, first_status, retries)
                    }
                };

                for rerun in reruns {
                    let (kind, ty) = kind_ty(rerun);
                    let stdout = String::from_utf8_lossy(&rerun.stdout);
                    let stderr = String::from_utf8_lossy(&rerun.stderr);
                    let stack_trace = heuristic_extract_description(rerun.result, &stdout, &stderr);

                    let mut test_rerun = TestRerun::new(kind);
                    if let Some(description) = stack_trace {
                        test_rerun.set_description(description);
                    }
                    test_rerun
                        .set_timestamp(to_datetime(rerun.start_time))
                        .set_time(rerun.time_taken)
                        .set_type(ty)
                        .set_system_out(stdout)
                        .set_system_err(stderr);
                    // TODO: also publish time? it won't be standard JUnit (but maybe that's ok?)
                    testcase_status.add_rerun(test_rerun);
                }

                let mut testcase = TestCase::new(test_instance.name, testcase_status);
                testcase
                    .set_classname(&test_instance.suite_info.binary_id)
                    .set_timestamp(to_datetime(main_status.start_time))
                    .set_time(main_status.time_taken);

                // TODO: also provide stdout and stderr for passing tests?
                // TODO: allure seems to want the output to be in a format where text files are
                // written out to disk:
                // https://github.com/allure-framework/allure2/blob/master/plugins/junit-xml-plugin/src/main/java/io/qameta/allure/junitxml/JunitXmlPlugin.java#L192-L196
                // we may have to update this format to handle that.
                if !main_status.result.is_success() {
                    let stdout = String::from_utf8_lossy(&main_status.stdout);
                    let stderr = String::from_utf8_lossy(&main_status.stderr);
                    let description =
                        heuristic_extract_description(main_status.result, &stdout, &stderr);
                    if let Some(description) = description {
                        testcase.status.set_description(description);
                    }

                    testcase
                        .set_system_out_lossy(&main_status.stdout)
                        .set_system_err_lossy(&main_status.stderr);
                }

                testsuite.add_test_case(testcase);
            }
            TestEvent::TestSkipped { .. } => {
                // TODO: report skipped tests? causes issues if we want to aggregate runs across
                // skipped and non-skipped tests. Probably needs to be made configurable.

                // let testsuite = self.testsuite_for(test_instance);
                //
                // let mut testcase_status = TestcaseStatus::skipped();
                // testcase_status.set_message(format!("Skipped: {}", reason));
                // let testcase = Testcase::new(test_instance.name, testcase_status);
                //
                // testsuite.add_testcase(testcase);
            }
            TestEvent::RunBeginCancel { .. } => {}
            TestEvent::RunFinished {
                run_id,
                start_time,
                elapsed,
                ..
            } => {
                // Write out the report to the given file.
                let mut report = Report::new(self.config.report_name());
                report
                    .set_uuid(run_id)
                    .set_timestamp(to_datetime(start_time))
                    .set_time(elapsed)
                    .add_test_suites(self.test_suites.drain().map(|(_, testsuite)| testsuite));

                let junit_path = self.config.path();
                let junit_dir = junit_path.parent().expect("junit path must have a parent");
                std::fs::create_dir_all(junit_dir).map_err(|error| WriteEventError::Fs {
                    file: junit_dir.to_path_buf(),
                    error,
                })?;

                let f = File::create(junit_path).map_err(|error| WriteEventError::Fs {
                    file: junit_path.to_path_buf(),
                    error,
                })?;
                report
                    .serialize(f)
                    .map_err(|error| WriteEventError::Junit {
                        file: junit_path.to_path_buf(),
                        error,
                    })?;
            }
        }

        Ok(())
    }

    fn testsuite_for(&mut self, test_instance: TestInstance<'cfg>) -> &mut TestSuite {
        self.test_suites
            .entry(&test_instance.suite_info.binary_id)
            .or_insert_with(|| TestSuite::new(&test_instance.suite_info.binary_id))
    }
}

fn to_datetime(system_time: SystemTime) -> DateTime<FixedOffset> {
    // Serialize using UTC.
    let datetime = DateTime::<Utc>::from(system_time);
    datetime.into()
}

// This regex works for the default panic handler for Rust -- other panic handlers may not work,
// which is why this is heuristic.
static PANICKED_AT_REGEX_STR: &str = "^thread '([^']+)' panicked at '";
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
        return Some(
            format!(
                "Test aborted with code {}{}",
                crate::helpers::display_nt_status(exception),
                if leaked {
                    ", and also leaked handles"
                } else {
                    ""
                }
            )
            .into(),
        );
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
            // Strip invalid XML characters (e.g. ANSI escapes) if they're around.
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
