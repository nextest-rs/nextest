// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Metadata management.

use crate::{
    config::{NextestJunitConfig, NextestProfile},
    errors::{JunitError, WriteEventError},
    reporter::TestEvent,
    runner::{ExecuteStatus, ExecutionDescription, ExecutionResult},
    test_list::TestInstance,
};
use camino::Utf8Path;
use chrono::{DateTime, FixedOffset, Utc};
use debug_ignore::DebugIgnore;
use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestRerun, TestSuite};
use std::{collections::HashMap, fs::File, time::SystemTime};

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct EventAggregator<'cfg> {
    store_dir: &'cfg Utf8Path,
    // TODO: log information in a JSONable report (converting that to XML later) instead of directly
    // writing it to XML
    junit: Option<MetadataJunit<'cfg>>,
}

impl<'cfg> EventAggregator<'cfg> {
    pub(crate) fn new(profile: &'cfg NextestProfile<'cfg>) -> Self {
        Self {
            store_dir: profile.store_dir(),
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
            TestEvent::TestRetry { .. } => {
                // Retries are recorded in TestFinished.
            }
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
            } => {
                fn kind_ty(run_status: &ExecuteStatus) -> (NonSuccessKind, &'static str) {
                    match run_status.result {
                        ExecutionResult::Fail => (NonSuccessKind::Failure, "test failure"),
                        ExecutionResult::ExecFail => (NonSuccessKind::Error, "execution failure"),
                        ExecutionResult::Pass => unreachable!("this is a failure status"),
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
                        .set_timestamp(to_datetime(rerun.start_time))
                        .set_time(rerun.time_taken)
                        .set_type(ty)
                        .set_system_out_lossy(rerun.stdout())
                        .set_system_err_lossy(rerun.stderr());
                    // TODO: also publish time? it won't be standard JUnit (but maybe that's ok?)
                    testcase_status.add_rerun(test_rerun);
                }

                // TODO: set message/description on testcase_status?

                let mut testcase = TestCase::new(test_instance.name, testcase_status);
                testcase
                    .set_classname(&test_instance.bin_info.binary_id)
                    .set_timestamp(to_datetime(main_status.start_time))
                    .set_time(main_status.time_taken);

                // TODO: also provide stdout and stderr for passing tests?
                // TODO: allure seems to want the output to be in a format where text files are
                // written out to disk:
                // https://github.com/allure-framework/allure2/blob/master/plugins/junit-xml-plugin/src/main/java/io/qameta/allure/junitxml/JunitXmlPlugin.java#L192-L196
                // we may have to update this format to handle that.
                if !main_status.result.is_success() {
                    // TODO: use the Arc wrapper, don't clone the system out and system err bytes
                    testcase
                        .set_system_out_lossy(main_status.stdout())
                        .set_system_err_lossy(main_status.stderr());
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
                start_time,
                elapsed,
                ..
            } => {
                // Write out the report to the given file.
                let mut report = Report::new(self.config.report_name());
                report
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
                report.serialize(f).map_err(|err| WriteEventError::Junit {
                    file: junit_path.to_path_buf(),
                    error: JunitError::new(err),
                })?;
            }
        }

        Ok(())
    }

    fn testsuite_for(&mut self, test_instance: TestInstance<'cfg>) -> &mut TestSuite {
        self.test_suites
            .entry(&test_instance.bin_info.binary_id)
            .or_insert_with(|| TestSuite::new(&test_instance.bin_info.binary_id))
    }
}

fn to_datetime(system_time: SystemTime) -> DateTime<FixedOffset> {
    // Serialize using UTC.
    let datetime = DateTime::<Utc>::from(system_time);
    datetime.into()
}
