// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Metadata management.

use crate::{
    reporter::TestEvent,
    runner::{RunDescribe, TestRunStatus, TestStatus},
    test_list::TestInstance,
};
use camino::Utf8Path;
use chrono::{DateTime, FixedOffset, Utc};
use color_eyre::eyre::{Result, WrapErr};
use debug_ignore::DebugIgnore;
use nextest_config::{NextestJunitConfig, NextestProfile};
use quick_junit::{NonSuccessKind, Report, TestRerun, Testcase, TestcaseStatus, Testsuite};
use std::{collections::HashMap, fs::File, time::SystemTime};

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct MetadataReporter<'cfg> {
    metadata_dir: &'cfg Utf8Path,
    metadata_name: &'cfg str,
    // TODO: log information in a JSONable report (converting that to XML later) instead of directly
    // writing it to XML
    junit: Option<MetadataJunit<'cfg>>,
}

impl<'cfg> MetadataReporter<'cfg> {
    pub(crate) fn new(profile: &'cfg NextestProfile<'cfg>) -> Self {
        let metadata_name = profile.metadata_name();
        Self {
            metadata_dir: profile.metadata_dir(),
            metadata_name,
            junit: profile
                .junit()
                .map(|config| MetadataJunit::new(metadata_name, config)),
        }
    }

    pub(crate) fn write_event(&mut self, event: TestEvent<'cfg>) -> Result<()> {
        if let Some(junit) = &mut self.junit {
            junit.write_event(event)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct MetadataJunit<'cfg> {
    metadata_name: &'cfg str,
    config: NextestJunitConfig<'cfg>,
    testsuites: DebugIgnore<HashMap<&'cfg str, Testsuite>>,
}

impl<'cfg> MetadataJunit<'cfg> {
    fn new(metadata_name: &'cfg str, config: NextestJunitConfig<'cfg>) -> Self {
        Self {
            metadata_name,
            config,
            testsuites: DebugIgnore(HashMap::new()),
        }
    }

    pub(crate) fn write_event(&mut self, event: TestEvent<'cfg>) -> Result<()> {
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
                fn kind_ty(run_status: &TestRunStatus) -> (NonSuccessKind, &'static str) {
                    match run_status.status {
                        TestStatus::Fail => (NonSuccessKind::Failure, "test failure"),
                        TestStatus::ExecFail => (NonSuccessKind::Error, "execution failure"),
                        TestStatus::Pass => unreachable!("this is a failure status"),
                    }
                }

                let testsuite = self.testsuite_for(test_instance);

                let (mut testcase_status, main_status, reruns) = match run_statuses.describe() {
                    RunDescribe::Success { run_status } => {
                        (TestcaseStatus::success(), run_status, &[][..])
                    }
                    RunDescribe::Flaky {
                        last_status,
                        prior_statuses,
                    } => (TestcaseStatus::success(), last_status, prior_statuses),
                    RunDescribe::Failure {
                        first_status,
                        retries,
                        ..
                    } => {
                        let (kind, ty) = kind_ty(first_status);
                        let mut testcase_status = TestcaseStatus::non_success(kind);
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

                let mut testcase = Testcase::new(test_instance.name, testcase_status);
                testcase
                    .set_classname(test_instance.binary_id)
                    .set_timestamp(to_datetime(main_status.start_time))
                    .set_time(main_status.time_taken);

                // TODO: also provide stdout and stderr for passing tests?
                // TODO: allure seems to want the output to be in a format where text files are
                // written out to disk:
                // https://github.com/allure-framework/allure2/blob/master/plugins/junit-xml-plugin/src/main/java/io/qameta/allure/junitxml/JunitXmlPlugin.java#L192-L196
                // we may have to update this format to handle that.
                if !main_status.status.is_success() {
                    // TODO: use the Arc wrapper, don't clone the system out and system err bytes
                    testcase
                        .set_system_out_lossy(main_status.stdout())
                        .set_system_err_lossy(main_status.stderr());
                }

                testsuite.add_testcase(testcase);
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
                let mut report = Report::new(self.metadata_name);
                report
                    .set_timestamp(to_datetime(start_time))
                    .set_time(elapsed)
                    .add_testsuites(self.testsuites.drain().map(|(_, testsuite)| testsuite));

                let junit_path = self.config.path();
                let junit_dir = junit_path.parent().expect("junit path must have a parent");
                std::fs::create_dir_all(junit_dir).wrap_err_with(|| {
                    format!("failed to create junit output directory '{}'", junit_dir)
                })?;

                let f = File::create(junit_path).wrap_err_with(|| {
                    format!("failed to open junit file '{}' for writing", junit_path)
                })?;
                report
                    .serialize(f)
                    .wrap_err_with(|| format!("failed to serialize junit to {}", junit_path))?;
            }
        }

        Ok(())
    }

    fn testsuite_for(&mut self, test_instance: TestInstance<'cfg>) -> &mut Testsuite {
        self.testsuites
            .entry(test_instance.binary_id)
            .or_insert_with(|| Testsuite::new(test_instance.binary_id))
    }
}

fn to_datetime(system_time: SystemTime) -> DateTime<FixedOffset> {
    // Serialize using UTC.
    let datetime = DateTime::<Utc>::from(system_time);
    datetime.into()
}
