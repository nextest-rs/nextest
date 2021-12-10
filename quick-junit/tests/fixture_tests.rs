// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use chrono::DateTime;
use goldenfile::Mint;
use owo_colors::OwoColorize;
use quick_junit::{
    NonSuccessKind, Property, Report, TestRerun, Testcase, TestcaseStatus, Testsuite,
};
use std::time::Duration;

#[test]
fn fixtures() {
    let mut mint = Mint::new("tests/fixtures");

    let f = mint
        .new_goldenfile("basic_report.xml")
        .expect("creating new goldenfile succeeds");

    let basic_report = basic_report();
    basic_report
        .serialize(f)
        .expect("serializing basic_report succeeds");
}

fn basic_report() -> Report {
    let mut report = Report::new("my-test-run");
    report.set_timestamp(
        DateTime::parse_from_rfc2822("Thu, 1 Apr 2021 10:52:37 -0800")
            .expect("valid RFC2822 datetime"),
    );
    report.set_time(Duration::new(42, 234_567_890));

    let mut testsuite = Testsuite::new("testsuite0");
    testsuite.set_timestamp(
        DateTime::parse_from_rfc2822("Thu, 1 Apr 2021 10:52:39 -0800")
            .expect("valid RFC2822 datetime"),
    );

    // ---

    let testcase_status = TestcaseStatus::success();
    let mut testcase = Testcase::new("testcase0", testcase_status);
    testcase.set_system_out("testcase0-output");
    testsuite.add_testcase(testcase);

    // ---

    let mut testcase_status = TestcaseStatus::non_success(NonSuccessKind::Failure);
    testcase_status
        .set_description("this is the failure description")
        .set_message("testcase1-message");
    let mut testcase = Testcase::new("testcase1", testcase_status);
    testcase
        .set_system_err("some sort of failure output")
        .set_time(Duration::from_millis(4242));
    testsuite.add_testcase(testcase);

    // ---

    let mut testcase_status = TestcaseStatus::non_success(NonSuccessKind::Error);
    testcase_status
        .set_description("testcase2 error description")
        .set_type("error type");
    let mut testcase = Testcase::new("testcase2", testcase_status);
    testcase.set_time(Duration::from_nanos(421580));
    testsuite.add_testcase(testcase);

    // ---

    let mut testcase_status = TestcaseStatus::skipped();
    testcase_status
        .set_type("skipped type")
        .set_message("skipped message");
    // no description to test that.
    let mut testcase = Testcase::new("testcase3", testcase_status);
    testcase
        .set_timestamp(
            DateTime::parse_from_rfc2822("Thu, 1 Apr 2021 11:52:41 -0700")
                .expect("valid RFC2822 datetime"),
        )
        .set_assertions(20)
        .set_system_out("testcase3 output")
        .set_system_err("testcase3 error");
    testsuite.add_testcase(testcase);

    // ---

    let mut testcase_status = TestcaseStatus::success();

    let mut test_rerun = TestRerun::new(NonSuccessKind::Failure);
    test_rerun
        .set_type("flaky failure type")
        .set_description("this is a flaky failure description");
    testcase_status.add_rerun(test_rerun);

    let mut test_rerun = TestRerun::new(NonSuccessKind::Error);
    test_rerun
        .set_type("flaky error type")
        .set_system_out("flaky system output")
        .set_system_err(format!(
            "flaky system error with {}",
            "ANSI escape codes".blue()
        ))
        .set_stack_trace("flaky stack trace")
        .set_description("flaky error description");
    testcase_status.add_rerun(test_rerun);

    let mut testcase = Testcase::new("testcase4", testcase_status);
    testcase.set_time(Duration::from_millis(661661));
    testsuite.add_testcase(testcase);

    // ---

    let mut testcase_status = TestcaseStatus::non_success(NonSuccessKind::Failure);
    testcase_status.set_description("main test failure description");

    let mut test_rerun = TestRerun::new(NonSuccessKind::Failure);
    test_rerun.set_type("retry failure type");
    testcase_status.add_rerun(test_rerun);

    let mut test_rerun = TestRerun::new(NonSuccessKind::Error);
    test_rerun
        .set_type("retry error type")
        .set_system_out("retry error system output")
        .set_stack_trace("retry error stack trace");
    testcase_status.add_rerun(test_rerun);

    let mut testcase = Testcase::new("testcase5", testcase_status);
    testcase.set_time(Duration::from_millis(156));
    testsuite.add_testcase(testcase);

    testsuite.add_property(Property::new("env", "FOOBAR"));

    report.add_testsuite(testsuite);

    report
}
