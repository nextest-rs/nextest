// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use chrono::DateTime;
use goldenfile::Mint;
use owo_colors::OwoColorize;
use quick_junit::{
    NonSuccessKind, Property, Report, TestCase, TestCaseStatus, TestRerun, TestSuite,
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

    let mut test_suite = TestSuite::new("testsuite0");
    test_suite.set_timestamp(
        DateTime::parse_from_rfc2822("Thu, 1 Apr 2021 10:52:39 -0800")
            .expect("valid RFC2822 datetime"),
    );

    // ---

    let test_case_status = TestCaseStatus::success();
    let mut test_case = TestCase::new("testcase0", test_case_status);
    test_case.set_system_out("testcase0-output");
    test_suite.add_test_case(test_case);

    // ---

    let mut test_case_status = TestCaseStatus::non_success(NonSuccessKind::Failure);
    test_case_status
        .set_description("this is the failure description")
        .set_message("testcase1-message");
    let mut test_case = TestCase::new("testcase1", test_case_status);
    test_case
        .set_system_err("some sort of failure output")
        .set_time(Duration::from_millis(4242));
    test_suite.add_test_case(test_case);

    // ---

    let mut test_case_status = TestCaseStatus::non_success(NonSuccessKind::Error);
    test_case_status
        .set_description("testcase2 error description")
        .set_type("error type");
    let mut test_case = TestCase::new("testcase2", test_case_status);
    test_case.set_time(Duration::from_nanos(421580));
    test_suite.add_test_case(test_case);

    // ---

    let mut test_case_status = TestCaseStatus::skipped();
    test_case_status
        .set_type("skipped type")
        .set_message("skipped message");
    // no description to test that.
    let mut test_case = TestCase::new("testcase3", test_case_status);
    test_case
        .set_timestamp(
            DateTime::parse_from_rfc2822("Thu, 1 Apr 2021 11:52:41 -0700")
                .expect("valid RFC2822 datetime"),
        )
        .set_assertions(20)
        .set_system_out("testcase3 output")
        .set_system_err("testcase3 error");
    test_suite.add_test_case(test_case);

    // ---

    let mut test_case_status = TestCaseStatus::success();

    let mut test_rerun = TestRerun::new(NonSuccessKind::Failure);
    test_rerun
        .set_type("flaky failure type")
        .set_description("this is a flaky failure description");
    test_case_status.add_rerun(test_rerun);

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
    test_case_status.add_rerun(test_rerun);

    let mut test_case = TestCase::new("testcase4", test_case_status);
    test_case.set_time(Duration::from_millis(661661));
    test_suite.add_test_case(test_case);

    // ---

    let mut test_case_status = TestCaseStatus::non_success(NonSuccessKind::Failure);
    test_case_status.set_description("main test failure description");

    let mut test_rerun = TestRerun::new(NonSuccessKind::Failure);
    test_rerun.set_type("retry failure type");
    test_case_status.add_rerun(test_rerun);

    let mut test_rerun = TestRerun::new(NonSuccessKind::Error);
    test_rerun
        .set_type("retry error type")
        .set_system_out("retry error system output")
        .set_stack_trace("retry error stack trace");
    test_case_status.add_rerun(test_rerun);

    let mut test_case = TestCase::new("testcase5", test_case_status);
    test_case.set_time(Duration::from_millis(156));
    test_suite.add_test_case(test_case);

    let test_case_status = TestCaseStatus::success();
    let mut test_case = TestCase::new("testcase6", test_case_status);
    test_case.add_property(Property::new("step", "foobar"));
    test_suite.add_test_case(test_case);

    test_suite.add_property(Property::new("env", "FOOBAR"));

    report.add_test_suite(test_suite);

    report
}
