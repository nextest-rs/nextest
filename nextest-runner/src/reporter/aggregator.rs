// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Metadata management.

use super::TestEvent;
use crate::{
    config::{EvaluatableProfile, NextestJunitConfig},
    errors::{DisplayErrorChain, WriteEventError},
    list::TestInstance,
    reporter::TestEventKind,
    runner::{ExecuteStatus, ExecutionDescription, ExecutionResult},
    test_output::{ChildExecutionResult, ChildOutput},
};
use camino::Utf8PathBuf;
use debug_ignore::DebugIgnore;
use quick_junit::{
    NonSuccessKind, Report, TestCase, TestCaseStatus, TestRerun, TestSuite, XmlString,
};
use std::{borrow::Cow, collections::HashMap, fs::File};

#[derive(Clone, Debug)]
#[expect(dead_code)]
pub(crate) struct EventAggregator<'cfg> {
    store_dir: Utf8PathBuf,
    // TODO: log information in a JSONable report (converting that to XML later) instead of directly
    // writing it to XML
    junit: Option<MetadataJunit<'cfg>>,
}

impl<'cfg> EventAggregator<'cfg> {
    pub(crate) fn new(profile: &EvaluatableProfile<'cfg>) -> Self {
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
        match event.kind {
            TestEventKind::RunStarted { .. }
            | TestEventKind::RunPaused { .. }
            | TestEventKind::RunContinued { .. } => {}
            TestEventKind::SetupScriptStarted { .. }
            | TestEventKind::SetupScriptSlow { .. }
            | TestEventKind::SetupScriptFinished { .. } => {}
            TestEventKind::TestStarted { .. } => {}
            TestEventKind::TestSlow { .. } => {}
            TestEventKind::TestAttemptFailedWillRetry { .. }
            | TestEventKind::TestRetryStarted { .. } => {
                // Retries are recorded in TestFinished.
            }
            TestEventKind::TestFinished {
                test_instance,
                run_statuses,
                junit_store_success_output,
                junit_store_failure_output,
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
                    let mut test_rerun = TestRerun::new(kind);
                    test_rerun
                        .set_timestamp(rerun.start_time)
                        .set_time(rerun.time_taken)
                        .set_type(ty);

                    set_execute_status_props(
                        rerun,
                        // Reruns are always failures.
                        false,
                        junit_store_failure_output,
                        TestcaseOrRerun::Rerun(&mut test_rerun),
                    );

                    // TODO: also publish time? it won't be standard JUnit (but maybe that's ok?)
                    testcase_status.add_rerun(test_rerun);
                }

                let mut testcase = TestCase::new(test_instance.name, testcase_status);
                testcase
                    .set_classname(test_instance.suite_info.binary_id.as_str())
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
                    main_status,
                    is_success,
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
            TestEventKind::RunBeginCancel { .. } => {}
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
            .entry(test_instance.suite_info.binary_id.as_str())
            .or_insert_with(|| TestSuite::new(test_instance.suite_info.binary_id.as_str()))
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
    execute_status: &ExecuteStatus,
    is_success: bool,
    store_stdout_stderr: bool,
    mut out: TestcaseOrRerun<'_>,
) {
    match &execute_status.output {
        ChildExecutionResult::Output { output, errors } => {
            if !is_success {
                if let Some(errors) = errors {
                    // Use the child errors as the message and description.
                    out.set_message(errors.as_one_line_summary());
                    out.set_description(DisplayErrorChain::new(errors).to_string());
                };
                let description = output.heuristic_extract_description(execute_status.result);
                if let Some(description) = description {
                    out.set_description(description.display_human().to_string());
                }
            }

            if store_stdout_stderr {
                match output {
                    ChildOutput::Split(split) => {
                        if let Some(stdout) = &split.stdout {
                            out.set_system_out(stdout.as_str_lossy());
                        }
                        if let Some(stderr) = &split.stderr {
                            out.set_system_err(stderr.as_str_lossy());
                        }
                    }
                    ChildOutput::Combined { output } => {
                        out.set_system_out(output.as_str_lossy())
                            .set_system_err("(stdout and stderr are combined)");
                    }
                }
            }
        }
        ChildExecutionResult::StartError(error) => {
            out.set_message(format!("Test execution failed: {error}"));
            out.set_description(DisplayErrorChain::new(error).to_string());
        }
    }
}
