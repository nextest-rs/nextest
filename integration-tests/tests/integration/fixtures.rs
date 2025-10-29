// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use fixture_data::{
    models::{TestCaseFixtureProperty, TestCaseFixtureStatus, TestSuiteFixtureProperty},
    nextest_tests::EXPECTED_TEST_SUITES,
};
use integration_tests::nextest_cli::{CargoNextestCli, cargo_bin};
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustTestSuiteStatusSummary, TestListSummary,
};
use regex::Regex;
use std::process::Command;

#[track_caller]
pub fn save_cargo_metadata(p: &TempProject) {
    let mut cmd = Command::new(cargo_bin());
    cmd.args([
        "metadata",
        "--format-version=1",
        "--all-features",
        "--no-deps",
        "--manifest-path",
    ]);
    cmd.arg(p.manifest_path());
    let output = cmd.output().expect("cargo metadata could run");

    assert_eq!(Some(0), output.status.code());
    std::fs::write(p.cargo_metadata_path(), &output.stdout).unwrap();
}

#[track_caller]
pub fn save_binaries_metadata(p: &TempProject) {
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--list-type",
            "binaries-only",
            "--target-dir",
            p.target_dir().as_str(),
        ])
        .output();

    std::fs::write(p.binaries_metadata_path(), output.stdout).unwrap();
}

pub fn check_list_full_output(stdout: &[u8], platform: Option<BuildPlatform>) {
    let result: TestListSummary = serde_json::from_slice(stdout).unwrap();

    let test_suites = &*EXPECTED_TEST_SUITES;
    assert_eq!(
        test_suites.len(),
        result.rust_suites.len(),
        "test suite counts match"
    );

    for test_suite in test_suites {
        match platform {
            Some(p) if test_suite.build_platform != p => continue,
            _ => {}
        }

        let entry = result.rust_suites.get(&test_suite.binary_id);
        let entry = match entry {
            Some(e) => e,
            _ => panic!("Missing binary: {}", test_suite.binary_id),
        };

        if let Some(platform) = platform {
            if entry.binary.build_platform != platform {
                // The binary should be marked as skipped.
                assert_eq!(
                    entry.status,
                    RustTestSuiteStatusSummary::SKIPPED,
                    "for {}, test suite expected to be skipped because of platform mismatch",
                    test_suite.binary_id
                );
                assert!(
                    entry.test_cases.is_empty(),
                    "skipped test binaries should have no test cases"
                );
                continue;
            }
        }

        assert_eq!(
            entry.status,
            RustTestSuiteStatusSummary::LISTED,
            "for {}, test suite expected to be listed",
            test_suite.binary_id
        );
        assert_eq!(
            test_suite.test_cases.len(),
            entry.test_cases.len(),
            "testcase lengths match for {}",
            test_suite.binary_id
        );
        for case in &test_suite.test_cases {
            let e = entry.test_cases.get(case.name);
            let e = match e {
                Some(e) => e,
                _ => panic!(
                    "Missing test case '{}' in '{}'",
                    case.name, test_suite.binary_id
                ),
            };
            assert_eq!(case.status.is_ignored(), e.ignored);
        }
    }
}

#[track_caller]
pub fn check_list_binaries_output(stdout: &[u8]) {
    let result: BinaryListSummary = serde_json::from_slice(stdout).unwrap();

    let test_suite = &*EXPECTED_TEST_SUITES;
    let mut expected_binary_ids = test_suite
        .iter()
        .map(|fixture| (fixture.binary_id.clone(), fixture.build_platform))
        .collect::<Vec<_>>();
    expected_binary_ids.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut actual_binary_ids = result
        .rust_binaries
        .iter()
        .map(|(binary_id, info)| (binary_id.clone(), info.build_platform))
        .collect::<Vec<_>>();
    actual_binary_ids.sort_by(|(a, _), (b, _)| a.cmp(b));

    assert_eq!(
        expected_binary_ids, actual_binary_ids,
        "expected binaries:\n{expected_binary_ids:?}\nactual binaries\n{actual_binary_ids:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum CheckResult {
    Pass,
    Leak,
    LeakFail,
    Fail,
    FailLeak,
    Abort,
}

impl CheckResult {
    fn make_status_line_regex(self, name: &str) -> Regex {
        let name = regex::escape(name);
        match self {
            CheckResult::Pass => {
                Regex::new(&format!(r"PASS \[[^\]]+\] \([^\)]+\) *{name}")).unwrap()
            }
            CheckResult::Leak => {
                Regex::new(&format!(r"LEAK \[[^\]]+\] \([^\)]+\) *{name}")).unwrap()
            }
            CheckResult::LeakFail => {
                Regex::new(&format!(r"LEAK-FAIL \[[^\]]+\] \([^\)]+\) *{name}")).unwrap()
            }
            CheckResult::Fail => {
                Regex::new(&format!(r"FAIL \[[^\]]+\] \([^\)]+\) *{name}")).unwrap()
            }
            CheckResult::FailLeak => {
                Regex::new(&format!(r"FAIL \+ LEAK \[[^\]]+\] \([^\)]+\) *{name}")).unwrap()
            }
            CheckResult::Abort => Regex::new(&format!(
                r"(ABORT|SIGSEGV|SIGABRT) \[[^\]]+\] \([^\)]+\) *{name}"
            ))
            .unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u64)]
pub enum RunProperty {
    Relocated = 1,
    WithDefaultFilter = 2,
    // --skip cdylib
    WithSkipCdylibFilter = 4,
    // --exact test_multiply_two tests::test_multiply_two_cdylib
    WithMultiplyTwoExactFilter = 8,
    CdyLibPackageFilter = 0x21,
    SkipSummaryCheck = 0x22,
    ExpectNoBinaries = 0x24,
}

fn debug_run_properties(properties: u64) -> String {
    let mut ret = String::new();
    if properties & RunProperty::Relocated as u64 != 0 {
        ret.push_str("relocated ");
    }
    if properties & RunProperty::WithDefaultFilter as u64 != 0 {
        ret.push_str("with-default-filter ");
    }
    if properties & RunProperty::WithSkipCdylibFilter as u64 != 0 {
        ret.push_str("with-skip-cdylib-filter ");
    }
    if properties & RunProperty::WithMultiplyTwoExactFilter as u64 != 0 {
        ret.push_str("with-exact-filter ");
    }
    if properties & RunProperty::CdyLibPackageFilter as u64 != 0 {
        ret.push_str("with-dylib-package-filter ");
    }
    if properties & RunProperty::SkipSummaryCheck as u64 != 0 {
        ret.push_str("with-skip-summary-check ");
    }
    if properties & RunProperty::ExpectNoBinaries as u64 != 0 {
        ret.push_str("with-expect-no-binaries ");
    }
    ret
}

#[track_caller]
pub fn check_run_output(stderr: &[u8], properties: u64) {
    // This could be made more robust with a machine-readable output,
    // or maybe using quick-junit output

    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{output}");

    let mut run_count = 0;
    let mut leak_count = 0;
    let mut pass_count = 0;
    let mut fail_count = 0;
    let mut leak_fail_count = 0;
    let mut skip_count = 0;

    for fixture in &*EXPECTED_TEST_SUITES {
        let binary_id = &fixture.binary_id;
        if fixture.has_property(TestSuiteFixtureProperty::NotInDefaultSet)
            && properties & RunProperty::WithDefaultFilter as u64 != 0
            && properties & RunProperty::CdyLibPackageFilter as u64 == 0
        {
            eprintln!("*** skipping {binary_id}");
            for test in &fixture.test_cases {
                let name = format!("{} {}", binary_id, test.name);
                // This binary should be skipped -- ensure that it isn't in the output. If it sh
                assert!(
                    !output.contains(&name),
                    "binary {binary_id} should not be run with default set"
                );
            }
            continue;
        }

        for test in &fixture.test_cases {
            let name = format!("{} {}", binary_id, test.name);

            if test.has_property(TestCaseFixtureProperty::NotInDefaultSet)
                && properties & RunProperty::WithDefaultFilter as u64 != 0
            {
                eprintln!("*** skipping {name}");
                assert!(
                    !output.contains(&name),
                    "test '{name}' should not be run with default set"
                );
                skip_count += 1;
                continue;
            }
            if cfg!(unix)
                && test.has_property(TestCaseFixtureProperty::NotInDefaultSetUnix)
                && properties & RunProperty::WithDefaultFilter as u64 != 0
            {
                eprintln!("*** skipping {name}");
                assert!(
                    !output.contains(&name),
                    "test '{name}' should not be run with default set on Unix"
                );
                skip_count += 1;
                continue;
            }
            if test.has_property(TestCaseFixtureProperty::MatchesCdylib)
                && properties & RunProperty::WithSkipCdylibFilter as u64 != 0
            {
                eprintln!("*** skipping {name}");
                assert!(
                    !output.contains(&name),
                    "test '{name}' should not be run with --skip cdylib"
                );
                skip_count += 1;
                continue;
            }
            if !test.has_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo)
                && properties & RunProperty::WithMultiplyTwoExactFilter as u64 != 0
            {
                eprintln!("*** skipping {name}");
                assert!(
                    !output.contains(&name),
                    "test '{name}' should not be run with --exact test_multiply_two test_multiply_two_cdylib"
                );
                skip_count += 1;
                continue;
            }

            let result = match test.status {
                // This is not a complete accounting -- for example, the needs-same-cwd check should
                // also be repeated for leaky tests in principle. But it's good enough for the test
                // suite that actually exists.
                TestCaseFixtureStatus::Pass => {
                    run_count += 1;
                    if test.has_property(TestCaseFixtureProperty::NeedsSameCwd)
                        && properties & RunProperty::Relocated as u64 != 0
                    {
                        fail_count += 1;
                        CheckResult::Fail
                    } else {
                        pass_count += 1;
                        CheckResult::Pass
                    }
                }
                TestCaseFixtureStatus::Leak => {
                    run_count += 1;
                    pass_count += 1;
                    leak_count += 1;
                    CheckResult::Leak
                }
                TestCaseFixtureStatus::LeakFail => {
                    run_count += 1;
                    fail_count += 1;
                    leak_fail_count += 1;
                    CheckResult::LeakFail
                }
                TestCaseFixtureStatus::Fail | TestCaseFixtureStatus::Flaky { .. } => {
                    // Flaky tests are not currently retried by this test suite. (They are retried
                    // by the older suite in nextest-runner/tests/integration).
                    run_count += 1;
                    fail_count += 1;
                    CheckResult::Fail
                }
                TestCaseFixtureStatus::FailLeak => {
                    run_count += 1;
                    fail_count += 1;
                    // Currently, fail + leak tests are not added to the
                    // leak_count, just the fail_count. (Maybe this is worth
                    // changing in the UI?)
                    CheckResult::FailLeak
                }
                TestCaseFixtureStatus::Segfault => {
                    run_count += 1;
                    fail_count += 1;
                    CheckResult::Abort
                }
                TestCaseFixtureStatus::IgnoredPass | TestCaseFixtureStatus::IgnoredFail => {
                    // Ignored tests are not currently run by this test suite. (They are run by the
                    // older suite in nextest-runner/tests/integration).
                    skip_count += 1;
                    continue;
                }
            };

            let name = format!("{} {}", binary_id, test.name);
            let reg = result.make_status_line_regex(&name);
            let is_match = reg.is_match(&output);

            if properties & RunProperty::CdyLibPackageFilter as u64
                == RunProperty::CdyLibPackageFilter as u64
                && test.name != "tests::test_multiply_two_cdylib"
            {
                assert!(
                    !is_match,
                    "{name}: should not run when `RunProperty::CdyLibPackageFilter` is set \n\n\
                 --- output ---\n{output}\n--- end output ---"
                );
            } else if properties & RunProperty::ExpectNoBinaries as u64
                == RunProperty::ExpectNoBinaries as u64
            {
                assert!(
                    !is_match,
                    "{name}: should not run when `RunProperty::ExpectNoBinaries` is set \n\n\
                 --- output ---\n{output}\n--- end output ---"
                );
            } else {
                assert!(
                    is_match,
                    "{name}: status line result didn't match\n\n\
                 --- output ---\n{output}\n--- end output ---"
                );
            }

            // It would be nice to check for output regexes here, but it's a bit
            // inconvenient.
        }
    }

    let tests_str = if run_count == 1 { "test" } else { "tests" };
    let leak_fail_regex_str = if leak_fail_count > 0 {
        format!(r" \({leak_fail_count} due to being leaky\)")
    } else {
        String::new()
    };

    let summary_regex_str = match (leak_count, fail_count) {
        (0, 0) => {
            format!(
                r"Summary \[.*\] *{run_count} {tests_str} run: {pass_count} passed, {skip_count} skipped"
            )
        }
        (0, _) => {
            format!(
                r"Summary \[.*\] *{run_count} {tests_str} run: {pass_count} passed, {fail_count} failed{leak_fail_regex_str}, {skip_count} skipped"
            )
        }
        (_, 0) => {
            format!(
                r"Summary \[.*\] *{run_count} {tests_str} run: {pass_count} passed \({leak_count} leaky\), {skip_count} skipped"
            )
        }
        (_, _) => {
            format!(
                r"Summary \[.*\] *{run_count} {tests_str} run: {pass_count} passed \({leak_count} leaky\), {fail_count} failed{leak_fail_regex_str}, {skip_count} skipped"
            )
        }
    };

    if properties & RunProperty::SkipSummaryCheck as u64 != 0 {
        return;
    }

    let summary_reg = Regex::new(&summary_regex_str).unwrap();
    assert!(
        summary_reg.is_match(&output),
        "summary didn't match regex {summary_regex_str} (actual output: {output}, properties: {})",
        debug_run_properties(properties),
    );
}
