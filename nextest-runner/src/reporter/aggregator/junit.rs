// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Code to generate JUnit XML reports from test events.

use crate::{
    config::{
        elements::{JunitConfig, LeakTimeoutResult, SlowTimeoutResult},
        scripts::ScriptId,
    },
    errors::{DisplayErrorChain, WriteEventError},
    list::TestInstanceId,
    reporter::{
        UnitErrorDescription,
        displayer::DisplayUnitKind,
        events::{
            ChildExecutionOutputDescription, ChildOutputDescription, ExecutionDescription,
            ExecutionResultDescription, FailureDescription, StressIndex, TestEvent, TestEventKind,
            UnitKind,
        },
    },
    run_mode::NextestRunMode,
    test_output::ChildSingleOutput,
};
use debug_ignore::DebugIgnore;
use indexmap::IndexMap;
use nextest_metadata::RustBinaryId;
use quick_junit::{
    NonSuccessKind, Report, TestCase, TestCaseStatus, TestRerun, TestSuite, XmlString,
};
use std::{fmt, fs::File};

static STDOUT_STDERR_COMBINED: &str = "(stdout and stderr are combined)";
static STDOUT_NOT_CAPTURED: &str = "(stdout not captured)";
static STDERR_NOT_CAPTURED: &str = "(stderr not captured)";
static PROCESS_FAILED_TO_START: &str = "(process failed to start)";

#[derive(Clone, Debug)]
pub(super) struct MetadataJunit<'cfg> {
    mode: NextestRunMode,
    config: JunitConfig<'cfg>,
    test_suites: DebugIgnore<IndexMap<SuiteKey<'cfg>, TestSuite>>,
}

impl<'cfg> MetadataJunit<'cfg> {
    pub(super) fn new(mode: NextestRunMode, config: JunitConfig<'cfg>) -> Self {
        Self {
            mode,
            config,
            test_suites: DebugIgnore(IndexMap::new()),
        }
    }

    pub(super) fn write_event(
        &mut self,
        event: Box<TestEvent<'cfg>>,
    ) -> Result<(), WriteEventError> {
        // Copy mode at the start to avoid borrow checker conflicts.
        let mode = self.mode;
        match event.kind {
            TestEventKind::RunStarted { .. }
            | TestEventKind::StressSubRunStarted { .. }
            | TestEventKind::RunPaused { .. }
            | TestEventKind::RunContinued { .. }
            | TestEventKind::StressSubRunFinished { .. } => {}
            TestEventKind::SetupScriptStarted { .. } | TestEventKind::SetupScriptSlow { .. } => {}
            TestEventKind::SetupScriptFinished {
                stress_index,
                index: _,
                total: _,
                script_id,
                program,
                args,
                junit_store_success_output,
                junit_store_failure_output,
                no_capture: _,
                run_status,
            } => {
                let is_success = run_status.result.is_success();

                let test_suite = self.testsuite_for_setup_script(stress_index, script_id.clone());
                let testcase_status = if is_success {
                    TestCaseStatus::success()
                } else {
                    let (kind, ty) =
                        non_success_kind_and_type(mode, UnitKind::Script, &run_status.result);
                    let mut testcase_status = TestCaseStatus::non_success(kind);
                    testcase_status.set_type(ty);
                    testcase_status
                };

                let mut testcase =
                    TestCase::new(script_id.as_identifier().as_str(), testcase_status);
                // classname doesn't quite make sense for setup scripts, but it
                // is required by the spec at https://llg.cubic.org/docs/junit/.
                // We use the same name as the test suite.
                testcase
                    .set_classname(test_suite.name.clone())
                    .set_timestamp(run_status.start_time)
                    .set_time(run_status.time_taken);

                let store_stdout_stderr = (junit_store_success_output && is_success)
                    || (junit_store_failure_output && !is_success);

                set_execute_status_props(
                    &run_status.output,
                    store_stdout_stderr,
                    TestcaseOrRerun::Testcase(&mut testcase),
                );

                test_suite.add_test_case(testcase);

                // Add properties corresponding to the setup script.
                test_suite.add_property(("command", program.as_str()));
                test_suite.add_property(("args".to_owned(), shell_words::join(args)));
                // Also add environment variables set by the script.
                if let Some(env_map) = run_status.env_map {
                    for (key, value) in env_map.env_map {
                        test_suite.add_property((format!("output-env:{key}"), value));
                    }
                }
            }
            TestEventKind::InfoStarted { .. }
            | TestEventKind::InfoResponse { .. }
            | TestEventKind::InfoFinished { .. } => {}
            TestEventKind::InputEnter { .. } => {}
            TestEventKind::TestStarted { .. } => {}
            TestEventKind::TestSlow { .. } => {}
            TestEventKind::TestAttemptFailedWillRetry { .. }
            | TestEventKind::TestRetryStarted { .. } => {
                // Retries are recorded in TestFinished.
            }
            TestEventKind::TestFinished {
                stress_index,
                test_instance,
                run_statuses,
                junit_store_success_output,
                junit_store_failure_output,
                ..
            } => {
                let testsuite = self.testsuite_for_test(stress_index, test_instance);

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
                        let (kind, ty) =
                            non_success_kind_and_type(mode, UnitKind::Test, &first_status.result);
                        let mut testcase_status = TestCaseStatus::non_success(kind);
                        testcase_status.set_type(ty);
                        (testcase_status, first_status, retries)
                    }
                };

                for rerun in reruns {
                    let (kind, ty) = non_success_kind_and_type(mode, UnitKind::Test, &rerun.result);
                    let mut test_rerun = TestRerun::new(kind);
                    test_rerun
                        .set_timestamp(rerun.start_time)
                        .set_time(rerun.time_taken)
                        .set_type(ty);

                    set_execute_status_props(
                        &rerun.output,
                        junit_store_failure_output,
                        TestcaseOrRerun::Rerun(&mut test_rerun),
                    );

                    // TODO: also publish time? it won't be standard JUnit (but maybe that's ok?)
                    testcase_status.add_rerun(test_rerun);
                }

                let mut testcase = TestCase::new(test_instance.test_name.as_str(), testcase_status);
                testcase
                    .set_classname(test_instance.binary_id.as_str())
                    .set_timestamp(main_status.start_time)
                    .set_time(main_status.time_taken);

                // TODO: allure seems to want the output to be in a format where text files are
                // written out to disk:
                // https://github.com/allure-framework/allure2/blob/master/plugins/junit-xml-plugin/src/main/java/io/qameta/allure/junitxml/JunitXmlPlugin.java#L192-L196
                // we may have to update this format to handle that.
                let is_success = main_status.result.is_success();
                let store_stdout_stderr = (junit_store_success_output && is_success)
                    || (junit_store_failure_output && !is_success);

                set_execute_status_props(
                    &main_status.output,
                    store_stdout_stderr,
                    TestcaseOrRerun::Testcase(&mut testcase),
                );

                testsuite.add_test_case(testcase);
            }
            TestEventKind::TestSkipped { .. } => {
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
            TestEventKind::RunBeginCancel { .. } | TestEventKind::RunBeginKill { .. } => {}
            TestEventKind::RunFinished {
                run_id,
                start_time,
                elapsed,
                ..
            } => {
                // Write out the report to the given file.
                let mut report = Report::new(self.config.report_name());
                report
                    .set_report_uuid(run_id)
                    .set_timestamp(start_time)
                    .set_time(elapsed)
                    .add_test_suites(self.test_suites.drain(..).map(|(_, testsuite)| testsuite));

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

    fn testsuite_for_setup_script(
        &mut self,
        stress_index: Option<StressIndex>,
        script_id: ScriptId,
    ) -> &mut TestSuite {
        let key = SuiteKey::SetupScript {
            script_id: script_id.clone(),
            stress_index,
        };
        self.test_suites
            .entry(key.clone())
            .or_insert_with(|| TestSuite::new(key.to_string()))
    }

    fn testsuite_for_test(
        &mut self,
        stress_index: Option<StressIndex>,
        test_instance: TestInstanceId<'cfg>,
    ) -> &mut TestSuite {
        let key = SuiteKey::TestBinary {
            binary_id: test_instance.binary_id,
            stress_index,
        };
        self.test_suites
            .entry(key.clone())
            .or_insert_with(|| TestSuite::new(key.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum SuiteKey<'cfg> {
    // Each script gets a separate suite, because in the future we'll likely want to set up
    SetupScript {
        script_id: ScriptId,
        stress_index: Option<StressIndex>,
    },
    TestBinary {
        binary_id: &'cfg RustBinaryId,
        stress_index: Option<StressIndex>,
    },
}

impl fmt::Display for SuiteKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SuiteKey::SetupScript {
                script_id,
                stress_index,
            } => {
                write!(f, "@setup-script:{script_id}")?;
                if let Some(stress_index) = stress_index {
                    write!(f, "@stress-{}", stress_index.current)?;
                }
                Ok(())
            }
            SuiteKey::TestBinary {
                binary_id,
                stress_index,
            } => {
                write!(f, "{binary_id}")?;
                if let Some(stress_index) = stress_index {
                    write!(f, "@stress-{}", stress_index.current)?;
                }
                Ok(())
            }
        }
    }
}

fn non_success_kind_and_type(
    mode: NextestRunMode,
    kind: UnitKind,
    result: &ExecutionResultDescription,
) -> (NonSuccessKind, String) {
    match result {
        ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort { .. },
            leaked: true,
        } => (
            NonSuccessKind::Failure,
            format!(
                "{} abort with leaked handles",
                DisplayUnitKind::new(mode, kind),
            ),
        ),
        ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort { .. },
            leaked: false,
        } => (
            NonSuccessKind::Failure,
            format!("{} abort", DisplayUnitKind::new(mode, kind)),
        ),
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code },
            leaked: true,
        } => (
            NonSuccessKind::Failure,
            format!(
                "{} failure with exit code {code}, and leaked handles",
                DisplayUnitKind::new(mode, kind),
            ),
        ),
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code },
            leaked: false,
        } => (
            NonSuccessKind::Failure,
            format!(
                "{} failure with exit code {code}",
                DisplayUnitKind::new(mode, kind),
            ),
        ),
        ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Fail,
        } => (
            NonSuccessKind::Failure,
            format!("{} timeout", DisplayUnitKind::new(mode, kind)),
        ),
        ExecutionResultDescription::ExecFail => {
            (NonSuccessKind::Error, "execution failure".to_owned())
        }
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        } => (
            NonSuccessKind::Error,
            format!(
                "{} passed but leaked handles",
                DisplayUnitKind::new(mode, kind),
            ),
        ),
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Fail,
        } => (
            NonSuccessKind::Error,
            format!(
                "{} exited with code 0, but leaked handles so was marked failed",
                DisplayUnitKind::new(mode, kind),
            ),
        ),
        ExecutionResultDescription::Pass
        | ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Pass,
        } => {
            unreachable!("this is a failure status")
        }
    }
}

enum TestcaseOrRerun<'a> {
    Testcase(&'a mut TestCase),
    Rerun(&'a mut TestRerun),
}

impl TestcaseOrRerun<'_> {
    fn set_message(&mut self, message: impl Into<XmlString>) -> &mut Self {
        match self {
            TestcaseOrRerun::Testcase(testcase) => {
                testcase.status.set_message(message.into());
            }
            TestcaseOrRerun::Rerun(rerun) => {
                rerun.set_message(message.into());
            }
        }
        self
    }

    fn set_description(&mut self, description: impl Into<XmlString>) -> &mut Self {
        match self {
            TestcaseOrRerun::Testcase(testcase) => {
                testcase.status.set_description(description.into());
            }
            TestcaseOrRerun::Rerun(rerun) => {
                rerun.set_description(description.into());
            }
        }
        self
    }

    fn set_system_out(&mut self, system_out: impl Into<XmlString>) -> &mut Self {
        match self {
            TestcaseOrRerun::Testcase(testcase) => {
                testcase.set_system_out(system_out.into());
            }
            TestcaseOrRerun::Rerun(rerun) => {
                rerun.set_system_out(system_out.into());
            }
        }
        self
    }

    fn set_system_err(&mut self, system_err: impl Into<XmlString>) -> &mut Self {
        match self {
            TestcaseOrRerun::Testcase(testcase) => {
                testcase.set_system_err(system_err.into());
            }
            TestcaseOrRerun::Rerun(rerun) => {
                rerun.set_system_err(system_err.into());
            }
        }
        self
    }
}

fn set_execute_status_props(
    exec_output: &ChildExecutionOutputDescription<ChildSingleOutput>,
    store_stdout_stderr: bool,
    mut out: TestcaseOrRerun<'_>,
) {
    // Currently we only aggregate test results, so always specify UnitKind::Test.
    let description = UnitErrorDescription::new(UnitKind::Test, exec_output);
    if let Some(errors) = description.all_error_list() {
        out.set_message(errors.short_message());
        out.set_description(DisplayErrorChain::new(errors).to_string());
    }

    if store_stdout_stderr {
        match exec_output {
            ChildExecutionOutputDescription::Output {
                output: ChildOutputDescription::Split { stdout, stderr },
                ..
            } => {
                if let Some(stdout) = stdout {
                    out.set_system_out(stdout.as_str_lossy());
                } else {
                    out.set_system_out(STDOUT_NOT_CAPTURED);
                }
                if let Some(stderr) = stderr {
                    out.set_system_err(stderr.as_str_lossy());
                } else {
                    out.set_system_err(STDERR_NOT_CAPTURED);
                }
            }
            ChildExecutionOutputDescription::Output {
                output: ChildOutputDescription::Combined { output },
                ..
            } => {
                out.set_system_out(output.as_str_lossy())
                    .set_system_err(STDOUT_STDERR_COMBINED);
            }
            ChildExecutionOutputDescription::StartError(_) => {
                out.set_system_out(PROCESS_FAILED_TO_START)
                    .set_system_err(PROCESS_FAILED_TO_START);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::reporter::events::{AbortStatus, SIGTERM};
    use crate::{
        errors::{ChildError, ChildFdError, ChildStartError, ErrorList},
        reporter::events::{ChildExecutionOutputDescription, ExecutionResult, FailureStatus},
        test_output::{ChildExecutionOutput, ChildOutput, ChildSplitOutput},
    };
    use bytes::Bytes;
    use std::{io, sync::Arc};

    #[test]
    fn test_set_execute_status_props() {
        let cases = [
            ExecuteStatusPropsCase {
                comment: "success + combined + store",
                status: TestCaseStatus::success(),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Pass),
                    output: ChildOutput::Combined {
                        output: Bytes::from("stdout\nstderr").into(),
                    },
                    errors: None,
                }
                .into(),
                store_stdout_stderr: true,
                message: None,
                description: None,
                system_out: Some("stdout\nstderr"),
                system_err: Some(STDOUT_STDERR_COMBINED),
            },
            ExecuteStatusPropsCase {
                comment: "success + combined + no store",
                status: TestCaseStatus::success(),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Pass),
                    output: ChildOutput::Combined {
                        output: Bytes::from("stdout\nstderr").into(),
                    },
                    errors: None,
                }
                .into(),
                store_stdout_stderr: false,
                message: None,
                description: None,
                system_out: None,
                system_err: None,
            },
            ExecuteStatusPropsCase {
                comment: "success + split + store",
                status: TestCaseStatus::success(),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Pass),
                    output: ChildOutput::Split(ChildSplitOutput {
                        stdout: Some(Bytes::from("stdout").into()),
                        stderr: Some(Bytes::from("stderr").into()),
                    }),
                    errors: None,
                }
                .into(),
                store_stdout_stderr: true,
                message: None,
                description: None,
                system_out: Some("stdout"),
                system_err: Some("stderr"),
            },
            // success + split + no store is not hugely important to test --
            // it's just another combination of the above.
            ExecuteStatusPropsCase {
                comment: "failure + combined + store",
                status: TestCaseStatus::non_success(NonSuccessKind::Failure),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Fail {
                        failure_status: FailureStatus::ExitCode(101),
                        leaked: true,
                    }),
                    output: ChildOutput::Combined {
                        output: Bytes::from(
                            "stdout\nstderr\nthread 'foo' panicked at xyz.rs:40:\nstrange\n\
                             extra\nextra2",
                        )
                        .into(),
                    },
                    errors: None,
                }
                .into(),
                store_stdout_stderr: true,
                message: Some("thread 'foo' panicked at xyz.rs:40"),
                description: Some("thread 'foo' panicked at xyz.rs:40:\nstrange\nextra\nextra2"),
                system_out: Some(
                    "stdout\nstderr\nthread 'foo' panicked at xyz.rs:40:\nstrange\n\
                     extra\nextra2",
                ),
                system_err: Some(STDOUT_STDERR_COMBINED),
            },
            ExecuteStatusPropsCase {
                comment: "failure + split + no store",
                status: TestCaseStatus::non_success(NonSuccessKind::Failure),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Fail {
                        failure_status: FailureStatus::ExitCode(101),
                        leaked: false,
                    }),
                    output: ChildOutput::Split(ChildSplitOutput {
                        stdout: None,
                        stderr: Some(
                            Bytes::from(
                                "stdout\nstderr\nthread 'foo' panicked at xyz.rs:40:\n\
                                 strange\nextra\nextra2",
                            )
                            .into(),
                        ),
                    }),
                    errors: None,
                }
                .into(),
                store_stdout_stderr: false,
                message: Some("thread 'foo' panicked at xyz.rs:40"),
                description: Some(
                    "thread 'foo' panicked at xyz.rs:40:\n\
                     strange\nextra\nextra2",
                ),
                system_out: None,
                system_err: None,
            },
            #[cfg(unix)]
            ExecuteStatusPropsCase {
                comment: "abort + split + store (unix)",
                status: TestCaseStatus::non_success(NonSuccessKind::Failure),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Fail {
                        failure_status: FailureStatus::Abort(AbortStatus::UnixSignal(SIGTERM)),
                        leaked: false,
                    }),
                    output: ChildOutput::Split(ChildSplitOutput {
                        stdout: Some(Bytes::from("stdout\nstdout 2\n").into()),
                        stderr: None,
                    }),
                    errors: None,
                }
                .into(),
                store_stdout_stderr: true,
                message: Some("process aborted with signal 15 (SIGTERM)"),
                description: Some("process aborted with signal 15 (SIGTERM)"),
                system_out: Some("stdout\nstdout 2\n"),
                system_err: Some(STDERR_NOT_CAPTURED),
            },
            #[cfg(unix)]
            ExecuteStatusPropsCase {
                comment: "abort + multiple errors + no store (unix)",
                status: TestCaseStatus::non_success(NonSuccessKind::Failure),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Fail {
                        failure_status: FailureStatus::Abort(AbortStatus::UnixSignal(SIGTERM)),
                        leaked: true,
                    }),
                    output: ChildOutput::Split(ChildSplitOutput {
                        stdout: None,
                        stderr: Some(
                            Bytes::from("stdout\nthread 'foo' panicked at xyz.rs:40").into(),
                        ),
                    }),
                    errors: ErrorList::new(
                        "collecting child output",
                        vec![ChildError::Fd(ChildFdError::Wait(Arc::new(
                            io::Error::other("huh"),
                        )))],
                    ),
                }
                .into(),
                store_stdout_stderr: false,
                message: Some("3 errors occurred executing test"),
                description: Some(indoc::indoc! {"
                    3 errors occurred executing test:
                    * error waiting for child process to exit
                        caused by:
                        - huh
                    * process aborted with signal 15 (SIGTERM), and also leaked handles
                    * thread 'foo' panicked at xyz.rs:40
                "}),
                system_out: None,
                system_err: None,
            },
            ExecuteStatusPropsCase {
                comment: "multiple errors + store",
                status: TestCaseStatus::non_success(NonSuccessKind::Failure),
                output: ChildExecutionOutput::Output {
                    result: Some(ExecutionResult::Fail {
                        failure_status: FailureStatus::ExitCode(101),
                        leaked: false,
                    }),
                    output: ChildOutput::Split(ChildSplitOutput {
                        stdout: None,
                        stderr: Some(
                            Bytes::from("stdout\nthread 'foo' panicked at xyz.rs:40").into(),
                        ),
                    }),
                    errors: ErrorList::new(
                        "collecting child output",
                        vec![ChildError::Fd(ChildFdError::ReadStdout(Arc::new(
                            io::Error::other("stdout error"),
                        )))],
                    ),
                }
                .into(),
                store_stdout_stderr: false,
                message: Some("2 errors occurred executing test"),
                description: Some(indoc::indoc! {"
                    2 errors occurred executing test:
                    * error reading standard output
                        caused by:
                        - stdout error
                    * thread 'foo' panicked at xyz.rs:40
                "}),
                system_out: None,
                system_err: None,
            },
            ExecuteStatusPropsCase {
                comment: "exec fail + combined + store (exec fail means nothing to store)",
                status: TestCaseStatus::non_success(NonSuccessKind::Error),
                output: ChildExecutionOutput::StartError(ChildStartError::Spawn(Arc::new(
                    io::Error::other("start error"),
                )))
                .into(),
                store_stdout_stderr: true,
                message: Some("error spawning child process"),
                description: Some(indoc::indoc! {"
                    error spawning child process
                      caused by:
                      - start error"
                }),
                system_out: Some(PROCESS_FAILED_TO_START),
                system_err: Some(PROCESS_FAILED_TO_START),
            },
        ];

        for case in cases {
            eprintln!("** testing: {}", case.comment);

            let mut testcase = TestCase::new("test", case.status);
            set_execute_status_props(
                &case.output,
                case.store_stdout_stderr,
                TestcaseOrRerun::Testcase(&mut testcase),
            );
            assert_eq!(
                get_message(&testcase.status),
                case.message,
                "message matches"
            );
            assert_eq!(
                get_description(&testcase.status),
                case.description,
                "description matches"
            );
            assert_eq!(
                testcase.system_out.as_ref().map(|s| s.as_str()),
                case.system_out,
                "system_out matches"
            );
            assert_eq!(
                testcase.system_err.as_ref().map(|s| s.as_str()),
                case.system_err,
                "system_err matches"
            );
        }
    }

    #[derive(Debug)]
    struct ExecuteStatusPropsCase<'a> {
        comment: &'a str,
        status: TestCaseStatus,
        output: ChildExecutionOutputDescription<ChildSingleOutput>,
        store_stdout_stderr: bool,
        message: Option<&'a str>,
        description: Option<&'a str>,
        system_out: Option<&'a str>,
        system_err: Option<&'a str>,
    }

    fn get_message(status: &TestCaseStatus) -> Option<&str> {
        match status {
            TestCaseStatus::Success { .. } => None,
            TestCaseStatus::NonSuccess { message, .. } => message.as_deref(),
            TestCaseStatus::Skipped { message, .. } => message.as_deref(),
        }
    }

    fn get_description(status: &TestCaseStatus) -> Option<&str> {
        match status {
            TestCaseStatus::Success { .. } => None,
            TestCaseStatus::NonSuccess { description, .. } => description.as_deref(),
            TestCaseStatus::Skipped { description, .. } => description.as_deref(),
        }
    }
}
