// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use goldenfile::Mint;
use quick_junit::{Property, Report, Testcase, TestcaseStatus, Testsuite};
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
    report.set_time(Duration::new(42, 234_567_890));

    let mut testsuite = Testsuite::new("testsuite0");

    let testcase_status = TestcaseStatus::success();
    let mut testcase = Testcase::new("testcase0", testcase_status);
    testcase.set_system_out("testcase0-output");
    testsuite.add_testcase(testcase);

    let mut testcase_status = TestcaseStatus::failure();
    testcase_status
        .set_description("this is the failure description")
        .set_message("testcase1-message");
    let mut testcase = Testcase::new("testcase1", testcase_status);
    testcase
        .set_system_err("some sort of failure output")
        .set_time(Duration::from_millis(4242));
    testsuite.add_testcase(testcase);

    let mut testcase_status = TestcaseStatus::error();
    testcase_status
        .set_description("testcase2 error description")
        .set_type("error type");
    let mut testcase = Testcase::new("testcase2", testcase_status);
    testcase.set_time(Duration::from_nanos(421580));
    testsuite.add_testcase(testcase);

    let mut testcase_status = TestcaseStatus::skipped();
    testcase_status
        .set_type("skipped type")
        .set_message("skipped message");
    // no description to test that.
    let mut testcase = Testcase::new("testcase3", testcase_status);
    testcase
        .set_assertions(20)
        .set_system_out("testcase3 output")
        .set_system_err("testcase3 error");
    testsuite.add_testcase(testcase);

    testsuite.add_property(Property::new("env", "FOOBAR"));

    report.add_testsuite(testsuite);

    report
}
