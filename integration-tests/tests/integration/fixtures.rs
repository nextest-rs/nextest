// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use fixture_data::{
    models::{CheckResult, RunProperty, TestSuiteFixtureProperty},
    nextest_tests::EXPECTED_TEST_SUITES,
};
use integration_tests::nextest_cli::{CargoNextestCli, cargo_bin};
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustTestSuiteStatusSummary, TestListSummary,
};
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    process::Command,
    sync::LazyLock,
};

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

/// Uniquely identifies a test case within the fixture data.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct TestInstanceId {
    binary_id: String,
    test_name: String,
}

impl TestInstanceId {
    fn new(binary_id: &str, test_name: &str) -> Self {
        Self {
            binary_id: binary_id.to_owned(),
            test_name: test_name.to_owned(),
        }
    }

    fn full_name(&self) -> String {
        format!("{} {}", self.binary_id, self.test_name)
    }
}

/// The expected test execution results for a particular nextest invocation.
#[derive(Clone, Debug)]
struct ExpectedTestResults {
    /// Tests that should be run, mapped to their expected outcome.
    should_run: HashMap<TestInstanceId, ExpectedOutcome>,
    /// Tests that should not appear in output.
    should_not_run: HashSet<TestInstanceId>,
    /// Summary counts derived from should_run.
    summary: ExpectedSummary,
}

impl ExpectedTestResults {
    /// Builds expected test results by applying filters to fixture data based on properties.
    fn new(properties: u64) -> Self {
        let mut should_run = HashMap::new();
        let mut should_not_run = HashSet::new();
        let mut summary = ExpectedSummary::default();

        for fixture in &*EXPECTED_TEST_SUITES {
            let binary_id = &fixture.binary_id;

            // Check if the entire test suite should be skipped.
            let skip_suite = (fixture.has_property(TestSuiteFixtureProperty::NotInDefaultSet)
                && properties & RunProperty::WithDefaultFilter as u64 != 0)
                || (!fixture.has_property(TestSuiteFixtureProperty::MatchesCdylibExample)
                    && properties & RunProperty::CdyLibExamplePackageFilter as u64 != 0);

            if skip_suite {
                // The entire suite should not appear in output.
                for test in &fixture.test_cases {
                    let identifier = TestInstanceId::new(binary_id.as_str(), test.name);
                    should_not_run.insert(identifier);
                }
                continue;
            }

            for test in &fixture.test_cases {
                let identifier = TestInstanceId::new(binary_id.as_str(), test.name);

                // Determine if this specific test should be filtered out.
                if test.should_skip(properties) {
                    should_not_run.insert(identifier);
                    summary.skip_count += 1;
                    continue;
                }

                // Determine the expected result for this test.
                let result = test.expected_result(properties);

                summary.update(result);
                should_run.insert(identifier, ExpectedOutcome { result });
            }
        }

        Self {
            should_run,
            should_not_run,
            summary,
        }
    }
}

/// The expected outcome for a test that should be run.
#[derive(Clone, Debug)]
struct ExpectedOutcome {
    result: CheckResult,
}

/// Summary counts for expected test execution.
#[derive(Clone, Debug, Default)]
struct ExpectedSummary {
    run_count: usize,
    pass_count: usize,
    fail_count: usize,
    leak_count: usize,
    leak_fail_count: usize,
    skip_count: usize,
}

impl ExpectedSummary {
    fn update(&mut self, result: CheckResult) {
        self.run_count += 1;

        match result {
            CheckResult::Pass => {
                self.pass_count += 1;
            }
            CheckResult::Leak => {
                self.pass_count += 1;
                self.leak_count += 1;
            }
            CheckResult::LeakFail => {
                self.fail_count += 1;
                self.leak_fail_count += 1;
            }
            CheckResult::Fail => {
                self.fail_count += 1;
            }
            CheckResult::FailLeak => {
                self.fail_count += 1;
                // Note: Currently fail + leak tests are not added to leak_count,
                // just fail_count. This matches the existing behavior.
            }
            CheckResult::Abort => {
                self.fail_count += 1;
            }
        }
    }
}

/// Test results parsed from actual test runner output.
#[derive(Clone, Debug)]
struct ActualTestResults {
    /// Tests that appeared in output with their results.
    tests: HashMap<TestInstanceId, ActualOutcome>,
    /// The parsed summary line.
    summary: Option<ActualSummary>,
}

/// The actual outcome parsed from test output.
#[derive(Clone, Debug)]
struct ActualOutcome {
    result: CheckResult,
}

/// Summary counts parsed from actual test output.
#[derive(Clone, Debug)]
struct ActualSummary {
    run_count: usize,
    pass_count: usize,
    fail_count: usize,
    leak_count: usize,
    leak_fail_count: usize,
    skip_count: usize,
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
    if properties & RunProperty::CdyLibExamplePackageFilter as u64 != 0 {
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

// Regex patterns for parsing test result lines from nextest output.
// Format: (STATUS) [duration] (attempt info) binary_id test_name
// Example: "        PASS [   0.004s] (  1/249) nextest-runner cargo_config::test_..."
static PASS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+PASS \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap());
static LEAK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+LEAK \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap());
static LEAK_FAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+LEAK-FAIL \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap());
static FAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+FAIL \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap());
static FAIL_LEAK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+FAIL \+ LEAK \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap());
static ABORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(?:ABORT|SIGSEGV|SIGABRT) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});

impl ActualTestResults {
    /// Parses test results from nextest output.
    fn parse(output: &str) -> Self {
        let mut tests = HashMap::new();

        // Parse each line for test results. Check more specific patterns first
        // (e.g., "FAIL + LEAK" before "FAIL" or "LEAK").
        for line in output.lines() {
            if let Some(caps) = FAIL_LEAK_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::FailLeak,
                    },
                );
            } else if let Some(caps) = LEAK_FAIL_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::LeakFail,
                    },
                );
            } else if let Some(caps) = ABORT_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::Abort,
                    },
                );
            } else if let Some(caps) = LEAK_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::Leak,
                    },
                );
            } else if let Some(caps) = FAIL_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::Fail,
                    },
                );
            } else if let Some(caps) = PASS_RE.captures(line) {
                let identifier = TestInstanceId::new(&caps[1], &caps[2]);
                tests.insert(
                    identifier,
                    ActualOutcome {
                        result: CheckResult::Pass,
                    },
                );
            }
        }

        // Parse the summary line.
        let summary = Self::parse_summary(output);

        Self { tests, summary }
    }

    /// Parses the summary line from nextest output.
    fn parse_summary(output: &str) -> Option<ActualSummary> {
        // Summary line format examples:
        // "Summary [...] N tests run: M passed, P skipped"
        // "Summary [...] N tests run: M passed (L leaky), P skipped"
        // "Summary [...] N tests run: M passed, F failed, P skipped"
        // "Summary [...] N tests run: M passed, F failed (L due to being leaky), P skipped"
        // "Summary [...] N tests run: M passed (L leaky), F failed (L2 due to being leaky), P skipped"

        let summary_re = Regex::new(
            r"Summary \[.*\] +(\d+) tests? run: (\d+) passed(?: \((\d+) leaky\))?,?(?: (\d+) failed(?: \((\d+) due to being leaky\))?,?)? (\d+) skipped"
        ).unwrap();

        for line in output.lines() {
            if let Some(caps) = summary_re.captures(line) {
                let run_count = caps[1].parse().unwrap();
                let pass_count = caps[2].parse().unwrap();
                let leak_count = caps
                    .get(3)
                    .map(|m| m.as_str().parse().unwrap())
                    .unwrap_or(0);
                let fail_count = caps
                    .get(4)
                    .map(|m| m.as_str().parse().unwrap())
                    .unwrap_or(0);
                let leak_fail_count = caps
                    .get(5)
                    .map(|m| m.as_str().parse().unwrap())
                    .unwrap_or(0);
                let skip_count = caps[6].parse().unwrap();

                return Some(ActualSummary {
                    run_count,
                    pass_count,
                    fail_count,
                    leak_count,
                    leak_fail_count,
                    skip_count,
                });
            }
        }

        None
    }
}

/// Verifies that all expected tests appear (or don't appear) in actual output as required.
#[track_caller]
fn verify_expected_in_actual(
    expected: &ExpectedTestResults,
    actual: &ActualTestResults,
    output: &str,
) {
    // Check that all tests that should run are present with correct result.
    for (identifier, expected_outcome) in &expected.should_run {
        let actual_outcome = actual.tests.get(identifier);

        match actual_outcome {
            Some(actual) => {
                // Test is present, verify result matches.
                assert_eq!(
                    expected_outcome.result,
                    actual.result,
                    "{}: expected result {:?} but got {:?}\n\n\
                     --- output ---\n{}\n--- end output ---",
                    identifier.full_name(),
                    expected_outcome.result,
                    actual.result,
                    output
                );
            }
            None => {
                panic!(
                    "{}: expected to run with result {:?} but was not found in output\n\n\
                     --- output ---\n{}\n--- end output ---",
                    identifier.full_name(),
                    expected_outcome.result,
                    output
                );
            }
        }
    }

    // Check that all tests that should not run are absent.
    for identifier in &expected.should_not_run {
        if actual.tests.contains_key(identifier) {
            panic!(
                "{}: should not be run but appeared in output\n\n\
                 --- output ---\n{}\n--- end output ---",
                identifier.full_name(),
                output
            );
        }

        // Also check that the test name doesn't appear anywhere in the output.
        let full_name = identifier.full_name();
        assert!(
            !output.contains(&full_name),
            "{}: should not be run but name appears in output\n\n\
             --- output ---\n{}\n--- end output ---",
            full_name,
            output
        );
    }
}

/// Verifies that all tests in actual output were expected (no unexpected tests).
#[track_caller]
fn verify_actual_in_expected(
    actual: &ActualTestResults,
    expected: &ExpectedTestResults,
    output: &str,
) {
    for identifier in actual.tests.keys() {
        if !expected.should_run.contains_key(identifier) {
            // Check if it's in should_not_run to provide a better error message.
            if expected.should_not_run.contains(identifier) {
                panic!(
                    "{}: appeared in output but should not have been run\n\n\
                     --- output ---\n{}\n--- end output ---",
                    identifier.full_name(),
                    output
                );
            } else {
                panic!(
                    "{}: appeared in output but was not in expected test set \
                     (not in fixture data or should_not_run)\n\n\
                     --- output ---\n{}\n--- end output ---",
                    identifier.full_name(),
                    output
                );
            }
        }
    }
}

/// Verifies that the summary line matches expected counts.
#[track_caller]
fn verify_summary(
    expected_summary: &ExpectedSummary,
    actual_summary: Option<&ActualSummary>,
    output: &str,
    properties: u64,
) {
    // Skip the summary check if requested.
    if properties & RunProperty::SkipSummaryCheck as u64 != 0 {
        return;
    }

    let actual = match actual_summary {
        Some(s) => s,
        None => {
            panic!(
                "Summary line not found in output (properties: {})\n\n\
                 --- output ---\n{}\n--- end output ---",
                debug_run_properties(properties),
                output
            );
        }
    };

    // Compare all counts.
    assert_eq!(
        expected_summary.run_count,
        actual.run_count,
        "run_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
    assert_eq!(
        expected_summary.pass_count,
        actual.pass_count,
        "pass_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
    assert_eq!(
        expected_summary.fail_count,
        actual.fail_count,
        "fail_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
    assert_eq!(
        expected_summary.leak_count,
        actual.leak_count,
        "leak_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
    assert_eq!(
        expected_summary.leak_fail_count,
        actual.leak_fail_count,
        "leak_fail_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
    assert_eq!(
        expected_summary.skip_count,
        actual.skip_count,
        "skip_count mismatch (properties: {})\n\n--- output ---\n{}\n--- end output ---",
        debug_run_properties(properties),
        output
    );
}

#[track_caller]
pub fn check_run_output(stderr: &[u8], properties: u64) {
    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{output}");

    // Build the expected and actual test result maps.
    let expected = ExpectedTestResults::new(properties);
    let actual = ActualTestResults::parse(&output);

    // Check that all expected tests appear (or don't appear) as required.
    verify_expected_in_actual(&expected, &actual, &output);

    // Check that all tests in output were expected (no unexpected tests).
    verify_actual_in_expected(&actual, &expected, &output);

    // Verify summary counts match.
    verify_summary(
        &expected.summary,
        actual.summary.as_ref(),
        &output,
        properties,
    );
}
