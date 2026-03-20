// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use camino::Utf8Path;
use fixture_data::{
    fixture_project::EXPECTED_TEST_SUITES,
    models::{
        CheckResult, ExpectedReruns, ExpectedTestResult, RunProperties, TerminalCheckResult,
        TestCaseFixtureProperties, TestSuiteFixtureProperties,
    },
};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use integration_tests::{
    env::TestEnvInfo,
    nextest_cli::{CargoNextestCli, cargo_bin},
};
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustBinaryId, RustTestSuiteStatusSummary, TestCaseName,
    TestListSummary,
};
use quick_junit::{FlakyOrRerun, Report};
use regex::Regex;
use std::{collections::BTreeSet, process::Command, sync::LazyLock};

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
pub fn save_binaries_metadata(env_info: &TestEnvInfo, p: &TempProject) {
    let output = CargoNextestCli::for_test(env_info)
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
            let e = entry.test_cases.get(&case.name);
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

/// Checks the output of `cargo nextest list --message-format oneline`.
///
/// The format is `binary_id test_name` per line.
#[track_caller]
pub fn check_list_oneline_output(stdout: &str) {
    let test_suites = &*EXPECTED_TEST_SUITES;

    // Build the set of expected test instances (binary_id, test_name).
    let mut expected_tests: BTreeSet<(RustBinaryId, TestCaseName)> = BTreeSet::new();
    for fixture in test_suites {
        for test_case in &fixture.test_cases {
            // Only include non-ignored tests (oneline without --verbose skips ignored tests).
            if !test_case.status.is_ignored() {
                expected_tests.insert((fixture.binary_id.clone(), test_case.name.clone()));
            }
        }
    }

    // Parse the actual output.
    let mut actual_tests: BTreeSet<(RustBinaryId, TestCaseName)> = BTreeSet::new();
    for line in stdout.lines() {
        let parts: Vec<_> = line.splitn(2, ' ').collect();
        assert_eq!(
            parts.len(),
            2,
            "each line should be 'binary_id test_name', got: {line:?}"
        );
        let binary_id = parts[0];
        let test_name = parts[1];
        assert!(
            !binary_id.is_empty(),
            "binary_id should not be empty in line: {line:?}"
        );
        assert!(
            !test_name.is_empty(),
            "test_name should not be empty in line: {line:?}"
        );
        actual_tests.insert((RustBinaryId::new(binary_id), TestCaseName::new(test_name)));
    }

    // Compare expected vs actual.
    let missing: Vec<_> = expected_tests.difference(&actual_tests).collect();
    let extra: Vec<_> = actual_tests.difference(&expected_tests).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "test list mismatch:\n  missing: {missing:?}\n  extra: {extra:?}"
    );
}

/// Checks the output of `cargo nextest list --message-format oneline --list-type binaries-only`.
///
/// The format is `binary_id` per line.
#[track_caller]
pub fn check_list_oneline_binaries_output(stdout: &str) {
    let test_suites = &*EXPECTED_TEST_SUITES;

    // Build set of expected binary IDs.
    let expected_binaries: BTreeSet<RustBinaryId> = test_suites
        .iter()
        .map(|fixture| fixture.binary_id.clone())
        .collect();

    // Parse the actual output.
    let actual_binaries: BTreeSet<RustBinaryId> = stdout
        .lines()
        .map(|line| {
            assert!(!line.is_empty(), "binary_id should not be empty");
            assert!(
                !line.contains(' '),
                "binary_id should not contain spaces: {line:?}"
            );
            RustBinaryId::new(line)
        })
        .collect();

    // Compare expected vs actual.
    let missing: Vec<_> = expected_binaries.difference(&actual_binaries).collect();
    let extra: Vec<_> = actual_binaries.difference(&expected_binaries).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "binary list mismatch:\n  missing: {missing:?}\n  extra: {extra:?}"
    );
}

/// Uniquely identifies a test case within the fixture data.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct TestInstanceId {
    binary_id: String,
    test_name: TestCaseName,
}

impl TestInstanceId {
    fn new(binary_id: &str, test_name: &TestCaseName) -> Self {
        Self {
            binary_id: binary_id.to_owned(),
            test_name: test_name.clone(),
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
    should_run: IdOrdMap<ExpectedOutcome>,
    /// Tests that should not appear in output.
    should_not_run: BTreeSet<TestInstanceId>,
    /// Summary counts derived from should_run.
    summary: ExpectedSummary,
}

impl ExpectedTestResults {
    /// Builds expected test results by applying filters to fixture data based on properties.
    fn new(properties: RunProperties) -> Self {
        let mut should_run = IdOrdMap::new();
        let mut should_not_run = BTreeSet::new();
        let mut summary = ExpectedSummary::default();

        for fixture in &*EXPECTED_TEST_SUITES {
            let binary_id = &fixture.binary_id;

            // Check if the entire test suite should be skipped.
            let skip_suite = (fixture.has_property(TestSuiteFixtureProperties::NOT_IN_DEFAULT_SET)
                && properties.contains(RunProperties::WITH_DEFAULT_FILTER))
                || (!fixture.has_property(TestSuiteFixtureProperties::MATCHES_CDYLIB_EXAMPLE)
                    && properties.contains(RunProperties::CDYLIB_EXAMPLE_PACKAGE_FILTER));

            if skip_suite {
                // The entire suite should not appear in output.
                for test in &fixture.test_cases {
                    let identifier = TestInstanceId::new(binary_id.as_str(), &test.name);
                    should_not_run.insert(identifier);
                }
                continue;
            }

            for test in &fixture.test_cases {
                let id = TestInstanceId::new(binary_id.as_str(), &test.name);

                if properties.contains(RunProperties::BENCHMARKS) {
                    // We don't consider skipped tests while running benchmarks.
                    if !test.has_property(TestCaseFixtureProperties::IS_BENCHMARK) {
                        continue;
                    }
                }

                // Determine if this specific test should be filtered out.
                if test.should_skip(properties) {
                    should_not_run.insert(id);
                    summary.skip_count += 1;
                    continue;
                }

                // Determine the expected result for this test.
                let expected = test.expected_result(properties);

                summary.update(expected.result);
                should_run
                    .insert_unique(ExpectedOutcome { id, expected })
                    .expect("duplicate ids should not be seen");
            }
        }

        Self {
            should_run,
            should_not_run,
            summary,
        }
    }

    /// Builds expected test results for only the specified test names.
    ///
    /// This is useful for testing scenarios with specific filter expressions
    /// like `test(=test_success) | test(=test_failure_assert)`.
    fn for_test_names(test_names: &[&str], properties: RunProperties) -> Self {
        let mut should_run = IdOrdMap::new();
        let mut should_not_run = BTreeSet::new();
        let mut summary = ExpectedSummary::default();

        for fixture in &*EXPECTED_TEST_SUITES {
            let binary_id = &fixture.binary_id;

            for test in &fixture.test_cases {
                let id = TestInstanceId::new(binary_id.as_str(), &test.name);

                if !test_names.contains(&test.name.as_str()) {
                    should_not_run.insert(id);
                    continue;
                }

                if test.should_skip(properties) {
                    should_not_run.insert(id);
                    summary.skip_count += 1;
                    continue;
                }

                let expected = test.expected_result(properties);

                summary.update(expected.result);
                should_run
                    .insert_unique(ExpectedOutcome { id, expected })
                    .expect("duplicate ids should not be seen");
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
    id: TestInstanceId,
    expected: ExpectedTestResult,
}

impl IdOrdItem for ExpectedOutcome {
    type Key<'a> = &'a TestInstanceId;
    fn key(&self) -> Self::Key<'_> {
        &self.id
    }
    id_upcast!();
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
            CheckResult::Fail | CheckResult::FlakyFail | CheckResult::FlakyFailJunitSuccess => {
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
            CheckResult::Timeout => {
                self.fail_count += 1;
            }
        }
    }
}

/// Test results parsed from actual test runner output.
#[derive(Clone, Debug)]
struct ActualTestResults {
    /// Tests that appeared in output with all their attempts.
    /// Each test may have multiple attempts if retries are enabled.
    tests: IdOrdMap<ActualOutcome>,
    /// The parsed summary line.
    summary: Option<ActualSummary>,
}

/// A single test attempt with its result.
#[derive(Clone, Debug)]
struct TestAttempt {
    /// The attempt number (1-based). This is not currently used, but is tracked
    /// internally.
    #[expect(dead_code)]
    attempt: u32,
    result: TerminalCheckResult,
}

/// The actual outcome parsed from test output.
#[derive(Clone, Debug)]
struct ActualOutcome {
    id: TestInstanceId,
    /// All attempts for this test, in order of appearance.
    attempts: Vec<TestAttempt>,
}

impl ActualOutcome {
    /// Returns the final result (last attempt).
    fn final_result(&self) -> TerminalCheckResult {
        self.attempts
            .last()
            .expect("at least one attempt should exist")
            .result
    }
}

impl IdOrdItem for ActualOutcome {
    type Key<'a> = &'a TestInstanceId;
    fn key(&self) -> Self::Key<'_> {
        &self.id
    }
    id_upcast!();
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

fn debug_run_properties(properties: RunProperties) -> String {
    format!("{properties:?}")
}

// Regex patterns for parsing test result lines from nextest output.
// Format: (TRY N )?(STATUS) [duration] (count/total or progress) binary_id test_name
// Example: "        PASS [   0.004s] (  1/249) nextest-runner cargo_config::test_..."
// For flaky tests that eventually pass, the format includes "TRY N " prefix:
// Example: "  TRY 3 PASS [   1.003s] (1/1) fixture-project::basic test_flaky..."
//
// We capture ALL result lines (including intermediate TRY N lines with progress like "(─────)")
// to track all attempts. The attempt number is captured in group 1 (if present).
// Groups: 1=attempt (optional), 2=binary_id, 3=test_name
//
// NOTE: We use \s* (zero or more whitespace) instead of \s+ because some lines may have
// varying amounts of leading whitespace depending on the test status and retry attempt.
static PASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?PASS \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
static LEAK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?LEAK \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
static LEAK_FAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?(?:LEAK-FAIL|LKFAIL) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)")
        .unwrap()
});
static FAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?FAIL \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
static FAIL_LEAK_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Match both "FAIL + LEAK" (first attempt) and "FL+LK" (retry attempts).
    Regex::new(r"^\s*(?:TRY (\d+) )?(?:FAIL \+ LEAK|FL\+LK) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)")
        .unwrap()
});
static ABORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:TRY (\d+) )?(?:ABORT|ABRT|SIGSEGV|SIGABRT) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)",
    )
    .unwrap()
});
static TIMEOUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?TIMEOUT \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
// TIMEOUT-PASS (and short forms TMPASS, SLOW+TMPASS) is shown when on-timeout = pass
// is configured and the test timed out but is considered passing.
static TIMEOUT_PASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:TRY (\d+) )?(?:TIMEOUT-PASS|TMPASS|SLOW\+TMPASS) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
// FLKY-FL is shown for tests that eventually passed but have flaky-result = "fail".
// Format: "FLKY-FL 4/5 [duration] (count/total) binary_id test_name"
// Must be checked before FLAKY_RE since both start with "FL".
static FLAKY_FAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*FLKY-FL (\d+)/(\d+) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
// FLAKY is shown in the summary section for tests that eventually passed.
// Format: "FLAKY 4/5 [duration] (count/total) binary_id test_name"
static FLAKY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*FLAKY (\d+)/(\d+) \[[^\]]+\] \([^\)]+\) +(.+?) +(.+)").unwrap()
});
static SUMMARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Note: "failed" can also be "timed out" for timeout failures.
    // The "passed" parenthetical may contain "N flaky" and/or "N leaky"
    // (e.g., "22 passed (1 flaky, 1 leaky)"). We capture only the leaky
    // count; flaky count is not tracked in ExpectedSummary.
    Regex::new(r"Summary \[.*\] +(\d+) (?:tests?|benchmarks?) run: (\d+) passed(?: \((?:\d+ flaky(?:, )?)?(?:(\d+) leaky)?\))?,?(?: (\d+) (?:failed|timed out)(?: \((\d+) due to being leaky\))?,?)? (\d+) skipped").unwrap()
});

impl ActualTestResults {
    /// Parses test results from nextest output.
    ///
    /// This function tracks ALL attempts for each test, not just the final result.
    /// With retries enabled, a test may appear multiple times in the output with
    /// different results (e.g., TRY 1 FAIL, TRY 2 FAIL, TRY 3 PASS).
    fn parse(output: &str) -> Self {
        let mut tests = IdOrdMap::new();
        let mut summary = None;

        /// Helper to add an attempt to the test results.
        fn add_attempt(
            tests: &mut IdOrdMap<ActualOutcome>,
            caps: &regex::Captures,
            result: TerminalCheckResult,
        ) {
            // Groups: 1=attempt (optional), 2=binary_id, 3=test_name
            let attempt = match caps.get(1) {
                Some(m) => m.as_str().parse::<u32>().expect("parsed attempt number"),
                None => 1,
            };
            let test_name = TestCaseName::new(&caps[3]);
            let id = TestInstanceId::new(&caps[2], &test_name);
            let attempt_record = TestAttempt { attempt, result };

            match tests.entry(&id) {
                iddqd::id_ord_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().attempts.push(attempt_record);
                }
                iddqd::id_ord_map::Entry::Vacant(entry) => {
                    entry.insert(ActualOutcome {
                        id,
                        attempts: vec![attempt_record],
                    });
                }
            }
        }

        /// Handle a FLKY-FL line. These appear both during the run and in the
        /// post-summary footer. They override the final PASS attempt with
        /// FlakyFail.
        fn handle_flaky_fail(tests: &mut IdOrdMap<ActualOutcome>, caps: &regex::Captures) {
            // Groups: 1=pass_attempt, 2=total_attempts, 3=binary_id, 4=test_name
            let test_name = TestCaseName::new(&caps[4]);
            let id = TestInstanceId::new(&caps[3], &test_name);
            let attempt_record = TestAttempt {
                attempt: caps[1].parse().expect("parsed attempt number"),
                result: TerminalCheckResult::FlakyFail,
            };

            match tests.entry(&id) {
                iddqd::id_ord_map::Entry::Occupied(mut entry) => {
                    // FLKY-FL appears after the individual attempts,
                    // including a TRY N PASS for the passing attempt.
                    // Replace that final PASS with FlakyFail to reflect
                    // the overall result.
                    let attempts = &mut entry.get_mut().attempts;
                    if let Some(last) = attempts.last_mut() {
                        last.result = TerminalCheckResult::FlakyFail;
                    } else {
                        attempts.push(attempt_record);
                    }
                }
                iddqd::id_ord_map::Entry::Vacant(entry) => {
                    entry.insert(ActualOutcome {
                        id,
                        attempts: vec![attempt_record],
                    });
                }
            }
        }

        /// Handle a FLAKY line. These appear in the post-summary footer for
        /// tests that eventually passed. They confirm the final PASS result
        /// (no-op if already recorded).
        fn handle_flaky_pass(tests: &mut IdOrdMap<ActualOutcome>, caps: &regex::Captures) {
            // Groups: 1=pass_attempt, 2=total_attempts, 3=binary_id, 4=test_name
            let test_name = TestCaseName::new(&caps[4]);
            let id = TestInstanceId::new(&caps[3], &test_name);
            let attempt_record = TestAttempt {
                attempt: caps[1].parse().expect("parsed attempt number"),
                result: TerminalCheckResult::Pass,
            };

            match tests.entry(&id) {
                iddqd::id_ord_map::Entry::Occupied(mut entry) => {
                    // FLAKY line appears after individual attempts, so it may
                    // duplicate the final PASS. Only add if not already a PASS.
                    let attempts = &mut entry.get_mut().attempts;
                    if attempts.last().map(|a| a.result) != Some(TerminalCheckResult::Pass) {
                        attempts.push(attempt_record);
                    }
                }
                iddqd::id_ord_map::Entry::Vacant(entry) => {
                    entry.insert(ActualOutcome {
                        id,
                        attempts: vec![attempt_record],
                    });
                }
            }
        }

        // Parse each line for test results. Check more specific patterns first
        // (e.g., "FAIL + LEAK" before "FAIL" or "LEAK").
        //
        // After the summary line, nextest repeats failure and flaky status
        // lines. We must continue parsing past the summary to pick up FLKY-FL
        // and FLAKY lines (which update the final result for flaky tests), but
        // we skip the duplicate TRY/status lines that would inflate attempt
        // counts.
        let mut past_summary = false;
        for line in output.lines() {
            if past_summary {
                // After the summary, only parse FLKY-FL and FLAKY lines.
                // Other status lines are duplicates of earlier output.
                if let Some(caps) = FLAKY_FAIL_RE.captures(line) {
                    handle_flaky_fail(&mut tests, &caps);
                } else if let Some(caps) = FLAKY_RE.captures(line) {
                    handle_flaky_pass(&mut tests, &caps);
                }
                continue;
            }

            // Track all attempts for each test to analyze retry behavior.
            if let Some(caps) = FAIL_LEAK_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::FailLeak);
            } else if let Some(caps) = LEAK_FAIL_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::LeakFail);
            } else if let Some(caps) = ABORT_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::Abort);
            } else if let Some(caps) = TIMEOUT_PASS_RE.captures(line) {
                // TIMEOUT-PASS is shown when on-timeout = pass and the test timed out.
                // We record this as a Pass since the test is configured to pass on timeout.
                add_attempt(&mut tests, &caps, TerminalCheckResult::Pass);
            } else if let Some(caps) = TIMEOUT_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::Timeout);
            } else if let Some(caps) = LEAK_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::Leak);
            } else if let Some(caps) = FAIL_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::Fail);
            } else if let Some(caps) = PASS_RE.captures(line) {
                add_attempt(&mut tests, &caps, TerminalCheckResult::Pass);
            } else if let Some(caps) = SUMMARY_RE.captures(line) {
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

                summary = Some(ActualSummary {
                    run_count,
                    pass_count,
                    fail_count,
                    leak_count,
                    leak_fail_count,
                    skip_count,
                });

                past_summary = true;
            }
        }

        Self { tests, summary }
    }
}

/// Verifies that all expected tests appear (or don't appear) in actual output as required.
#[track_caller]
fn verify_expected_in_actual(
    expected: &ExpectedTestResults,
    actual: &ActualTestResults,
    output: &str,
    properties: RunProperties,
) {
    // Check that all tests that should run are present with correct result.
    for expected_outcome in &expected.should_run {
        let actual_outcome = actual.tests.get(&expected_outcome.id);

        match actual_outcome {
            Some(actual) => {
                // Test is present, verify the final result matches.
                // With retries, a test may have multiple attempts — we check the last one.
                // Use to_terminal() because terminal output cannot distinguish
                // between some CheckResult variants (e.g., FlakyFailJunitSuccess
                // and FlakyFail both display as FLKY-FL).
                let actual_result = actual.final_result();
                let expected_terminal = expected_outcome.expected.result.to_terminal();
                assert_eq!(
                    expected_terminal,
                    actual_result,
                    "{}: expected result {:?} but got {:?} (attempts: {:?})\n\n\
                     --- output ---\n{}\n--- end output ---",
                    expected_outcome.id.full_name(),
                    expected_terminal,
                    actual_result,
                    actual.attempts,
                    output
                );

                // Verify the number of retry attempts matches expectations.
                // The number of non-final attempts (retries) is attempts.len() - 1.
                let actual_rerun_count = actual.attempts.len() - 1;
                match expected_outcome.expected.expected_reruns {
                    ExpectedReruns::None => {
                        assert_eq!(
                            actual_rerun_count,
                            0,
                            "{}: expected no reruns but found {} (properties: {})\n\n\
                             --- output ---\n{}\n--- end output ---",
                            expected_outcome.id.full_name(),
                            actual_rerun_count,
                            debug_run_properties(properties),
                            output
                        );
                    }
                    ExpectedReruns::FlakyRunCount(expected_count) => {
                        assert_eq!(
                            actual_rerun_count,
                            expected_count,
                            "{}: expected {} reruns but found {} (properties: {})\n\n\
                             --- output ---\n{}\n--- end output ---",
                            expected_outcome.id.full_name(),
                            expected_count,
                            actual_rerun_count,
                            debug_run_properties(properties),
                            output
                        );
                    }
                    ExpectedReruns::SomeReruns => {
                        assert!(
                            actual_rerun_count > 0,
                            "{}: expected reruns but found none (properties: {})\n\n\
                             --- output ---\n{}\n--- end output ---",
                            expected_outcome.id.full_name(),
                            debug_run_properties(properties),
                            output
                        );
                    }
                }
            }
            None => {
                panic!(
                    "{}: expected to run with result {:?} but was not found in output\n\n\
                     --- output ---\n{}\n--- end output ---",
                    expected_outcome.id.full_name(),
                    expected_outcome.expected.result,
                    output
                );
            }
        }
    }

    // Check that all tests that should not run are absent.
    for id in &expected.should_not_run {
        if actual.tests.contains_key(id) {
            panic!(
                "{}: should not be run but appeared in output\n\n\
                 --- output ---\n{}\n--- end output ---",
                id.full_name(),
                output
            );
        }

        // Also check that the test name doesn't appear anywhere in the output,
        // unless ALLOW_SKIPPED_NAMES_IN_OUTPUT is set (e.g., for replay which shows SKIP lines).
        if !properties.contains(RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT) {
            let full_name = id.full_name();
            assert!(
                !output.contains(&full_name),
                "{}: should not be run but name appears in output\n\n\
                 --- output ---\n{}\n--- end output ---",
                full_name,
                output
            );
        }
    }
}

/// Verifies that all tests in actual output were expected (no unexpected tests).
#[track_caller]
fn verify_actual_in_expected(
    actual: &ActualTestResults,
    expected: &ExpectedTestResults,
    output: &str,
) {
    for outcome in &actual.tests {
        if !expected.should_run.contains_key(&outcome.id) {
            // Check if it's in should_not_run to provide a better error message.
            if expected.should_not_run.contains(&outcome.id) {
                panic!(
                    "{}: appeared in output but should not have been run\n\n\
                     --- output ---\n{}\n--- end output ---",
                    outcome.id.full_name(),
                    output
                );
            } else {
                panic!(
                    "{}: appeared in output but was not in expected test set \
                     (not in fixture data or should_not_run)\n\n\
                     --- output ---\n{}\n--- end output ---",
                    outcome.id.full_name(),
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
    properties: RunProperties,
) {
    // Skip the summary check if requested.
    if properties.contains(RunProperties::SKIP_SUMMARY_CHECK) {
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
pub fn check_run_output_with_junit(
    stderr: &[u8],
    junit_path: &Utf8Path,
    properties: RunProperties,
) {
    check_run_output_impl(stderr, Some(junit_path), properties);
}

/// Checks the output of a test run against fixture data.
///
/// This function uses the fixture data model to verify that:
/// - All expected tests are present with the correct result.
/// - No unexpected tests appear in the output.
/// - Summary counts match expectations.
///
/// Use this for verifying replay output where JUnit files aren't available.
#[track_caller]
pub fn check_run_output(stderr: &[u8], properties: RunProperties) {
    check_run_output_impl(stderr, None, properties);
}

/// Checks the output of a test run that used a filter expression.
///
/// This function verifies output for runs that only executed specific tests
/// (e.g., using `-E 'test(=test_name)'`). It:
/// - Verifies the specified tests are present with correct results.
/// - Does NOT check for the full test suite.
/// - Skips summary verification since counts won't match full fixture model.
///
/// This is useful for replay tests and other scenarios where only a subset
/// of tests were executed.
#[track_caller]
pub fn check_run_output_for_test_names(
    output: &[u8],
    test_names: &[&str],
    properties: RunProperties,
) {
    let output_str = String::from_utf8(output.to_vec()).unwrap();

    println!("{output_str}");

    let expected = ExpectedTestResults::for_test_names(test_names, properties);
    let actual = ActualTestResults::parse(&output_str);

    eprintln!("expected: {expected:?}");
    eprintln!("actual: {actual:?}");

    verify_expected_in_actual(&expected, &actual, &output_str, properties);
    verify_actual_in_expected(&actual, &expected, &output_str);
    // Skip summary verification since a filtered run won't match fixture model counts.
}

/// Checks the output of a rerun against fixture data.
///
/// This function verifies that a rerun only executes tests that failed in the
/// initial run. Tests that passed in the initial run should be skipped.
///
/// The function:
/// 1. Builds expected results from fixture data for `properties`.
/// 2. Filters to only include tests that failed (outstanding tests).
/// 3. Verifies the rerun output against the filtered expectations.
///
/// Use `RunProperties::SKIP_SUMMARY_CHECK` if the summary counts don't match
/// exactly due to additional skipped tests.
#[track_caller]
pub fn check_rerun_output(rerun_stderr: &[u8], properties: RunProperties) {
    let rerun_output = String::from_utf8(rerun_stderr.to_vec()).unwrap();

    println!("{rerun_output}");

    let initial_expected = ExpectedTestResults::new(properties);
    let rerun_expected = filter_for_rerun(&initial_expected);
    let actual = ActualTestResults::parse(&rerun_output);

    eprintln!("rerun_expected: {rerun_expected:?}");
    eprintln!("actual: {actual:?}");

    verify_expected_in_actual(&rerun_expected, &actual, &rerun_output, properties);
    verify_actual_in_expected(&actual, &rerun_expected, &rerun_output);
    verify_summary(
        &rerun_expected.summary,
        actual.summary.as_ref(),
        &rerun_output,
        properties,
    );
}

/// Checks the output of a rerun with scope expansion.
///
/// In a rerun with scope expansion, the rerun filter includes tests that were
/// not in the initial run. These "expanded" tests should run in the rerun
/// along with any outstanding (failed) tests from the initial run.
#[track_caller]
pub fn check_rerun_expanded_output(
    rerun_stderr: &[u8],
    initial_test_names: &[&str],
    expanded_test_names: &[&str],
    properties: RunProperties,
) {
    let rerun_output = String::from_utf8(rerun_stderr.to_vec()).unwrap();

    println!("{rerun_output}");

    let initial_expected = ExpectedTestResults::for_test_names(initial_test_names, properties);
    let mut rerun_expected = filter_for_rerun(&initial_expected);

    let expanded_expected = ExpectedTestResults::for_test_names(expanded_test_names, properties);

    for outcome in &expanded_expected.should_run {
        rerun_expected.should_not_run.remove(&outcome.id);
        rerun_expected
            .should_run
            .insert_unique(outcome.clone())
            .expect("expanded tests should not overlap with initial run tests");
        rerun_expected.summary.update(outcome.expected.result);
    }

    // The fixture model includes tests (like benchmarks) that the actual run
    // may not see, so skip summary verification for expanded reruns.
    let properties = properties | RunProperties::SKIP_SUMMARY_CHECK;

    let actual = ActualTestResults::parse(&rerun_output);

    eprintln!("rerun_expected: {rerun_expected:?}");
    eprintln!("actual: {actual:?}");

    verify_expected_in_actual(&rerun_expected, &actual, &rerun_output, properties);
    verify_actual_in_expected(&actual, &rerun_expected, &rerun_output);
    verify_summary(
        &rerun_expected.summary,
        actual.summary.as_ref(),
        &rerun_output,
        properties,
    );
}

/// Filters ExpectedTestResults to only include tests that should run in a
/// rerun.
///
/// This returns a new ExpectedTestResults where:
/// - `should_run` contains only tests that failed (outstanding tests).
/// - `should_not_run` contains tests that passed (already passed, skip in
///   rerun).
fn filter_for_rerun(expected: &ExpectedTestResults) -> ExpectedTestResults {
    let mut rerun_should_run = IdOrdMap::new();
    let mut rerun_should_not_run = expected.should_not_run.clone();
    let mut rerun_summary = ExpectedSummary {
        skip_count: expected.summary.skip_count,
        ..Default::default()
    };

    for outcome in &expected.should_run {
        match outcome.expected.result {
            // Failed tests are still outstanding and should run.
            CheckResult::Fail
            | CheckResult::FlakyFail
            | CheckResult::FlakyFailJunitSuccess
            | CheckResult::Abort
            | CheckResult::Timeout
            | CheckResult::LeakFail
            | CheckResult::FailLeak => {
                rerun_should_run
                    .insert_unique(ExpectedOutcome {
                        id: outcome.id.clone(),
                        expected: outcome.expected,
                    })
                    .expect("no duplicates");
                rerun_summary.update(outcome.expected.result);
            }
            // Passed tests become skipped in the rerun.
            CheckResult::Pass | CheckResult::Leak => {
                rerun_should_not_run.insert(outcome.id.clone());
                rerun_summary.skip_count += 1;
            }
        }
    }

    ExpectedTestResults {
        should_run: rerun_should_run,
        should_not_run: rerun_should_not_run,
        summary: rerun_summary,
    }
}

#[track_caller]
fn check_run_output_impl(stderr: &[u8], junit_path: Option<&Utf8Path>, properties: RunProperties) {
    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{output}");

    let expected = ExpectedTestResults::new(properties);
    let actual = ActualTestResults::parse(&output);

    eprintln!("expected: {expected:?}");
    eprintln!("actual: {actual:?}");

    verify_expected_in_actual(&expected, &actual, &output, properties);
    verify_actual_in_expected(&actual, &expected, &output);
    verify_summary(
        &expected.summary,
        actual.summary.as_ref(),
        &output,
        properties,
    );

    if let Some(path) = junit_path {
        verify_junit(&expected, path, properties);
    }
}

/// Test results parsed from JUnit XML output.
#[derive(Clone, Debug)]
struct ActualJunitResults {
    /// Tests that appeared in JUnit output with their results.
    tests: IdOrdMap<JunitOutcome>,
}

/// The actual outcome parsed from JUnit XML.
#[derive(Clone, Debug)]
struct JunitOutcome {
    id: TestInstanceId,
    /// Non-success information, or None for success.
    non_success: Option<JunitNonSuccess>,
    rerun_info: JunitRerunInfo,
}

/// Non-success information extracted from JUnit XML.
#[derive(Clone, Debug)]
struct JunitNonSuccess {
    kind: quick_junit::NonSuccessKind,
    ty: String,
    message: Option<String>,
}

/// Rerun information extracted from JUnit XML.
#[derive(Clone, Debug)]
enum JunitRerunInfo {
    /// The test passed.
    Success { rerun_count: usize },
    /// The test failed.
    NonSuccess {
        rerun_kind: FlakyOrRerun,
        rerun_count: usize,
    },
}

impl IdOrdItem for JunitOutcome {
    type Key<'a> = &'a TestInstanceId;
    fn key(&self) -> Self::Key<'_> {
        &self.id
    }
    id_upcast!();
}

impl ActualJunitResults {
    /// Parses test results from JUnit XML file.
    fn parse(junit_path: &Utf8Path) -> Self {
        let junit_xml = std::fs::read_to_string(junit_path)
            .unwrap_or_else(|e| panic!("failed to read JUnit XML from {junit_path}: {e}"));

        let report = Report::deserialize_from_str(&junit_xml)
            .unwrap_or_else(|e| panic!("failed to parse JUnit XML from {junit_path}: {e}"));

        let mut tests = IdOrdMap::new();

        for test_suite in &report.test_suites {
            let binary_id = test_suite.name.as_str();

            // Skip setup scripts - they're not in fixture data.
            if binary_id.starts_with("@setup-script:") {
                continue;
            }

            for test_case in &test_suite.test_cases {
                let test_name = TestCaseName::new(test_case.name.as_str());
                let id = TestInstanceId::new(binary_id, &test_name);

                let (non_success, rerun_info) = match &test_case.status {
                    quick_junit::TestCaseStatus::Success { flaky_runs } => (
                        None,
                        JunitRerunInfo::Success {
                            rerun_count: flaky_runs.len(),
                        },
                    ),
                    quick_junit::TestCaseStatus::NonSuccess {
                        kind,
                        ty,
                        message,
                        reruns,
                        ..
                    } => (
                        Some(JunitNonSuccess {
                            kind: *kind,
                            ty: ty.as_ref().map(|s| s.to_string()).unwrap_or_default(),
                            message: message.as_ref().map(|s| s.to_string()),
                        }),
                        JunitRerunInfo::NonSuccess {
                            rerun_kind: reruns.kind,
                            rerun_count: reruns.runs.len(),
                        },
                    ),
                    quick_junit::TestCaseStatus::Skipped { .. } => {
                        // Skipped tests are filtered out during test execution and don't appear
                        // in our fixture data's expected results, so we skip them here to
                        // maintain consistency.
                        continue;
                    }
                };

                let outcome = JunitOutcome {
                    id: id.clone(),
                    non_success,
                    rerun_info,
                };
                tests
                    .insert_unique(outcome)
                    .expect("duplicate test case should not be seen in JUnit output");
            }
        }

        Self { tests }
    }
}

/// Verifies that JUnit output matches expected test results.
#[track_caller]
fn verify_junit(expected: &ExpectedTestResults, junit_path: &Utf8Path, properties: RunProperties) {
    let actual = ActualJunitResults::parse(junit_path);

    verify_expected_in_junit(expected, &actual, junit_path, properties);

    verify_junit_in_expected(&actual, expected, junit_path, properties);
}

// Expected JUnit type strings. These match the strings produced by nextest in
// junit.rs. The Rust test harness always uses exit code 101 for test failures.
const JUNIT_FAIL: &str = "test failure with exit code 101";
const JUNIT_FAIL_LEAK: &str = "test failure with exit code 101, and leaked handles";
const JUNIT_FLAKY_FAIL: &str = "flaky failure";
const JUNIT_ABORT: &str = "test abort";

/// Expected JUnit properties for a non-success test case.
#[derive(Clone, Debug)]
struct ExpectedJunit<'a> {
    kind: quick_junit::NonSuccessKind,
    ty: &'a str,
    message: JunitExpectedMessage<'a>,
}

// Known JUnit message prefixes for failure types. These correspond to the
// heuristic error extraction in error_description.rs:
//
// - "thread '" — panic messages (thread 'name' panicked at ...)
// - "Error:" — Result-based errors since Rust 1.66 (Error: message)
// - "note: test did not panic as expected" — should_panic mismatches
const FAIL_MESSAGE_PREFIXES: &[&str] =
    &["thread '", "Error:", "note: test did not panic as expected"];

// Abort messages from UnitAbortDescription display.
const ABORT_MESSAGE_PREFIXES: &[&str] = &["process was terminated"];

/// How to verify the JUnit message field for a non-success test case.
#[derive(Clone, Debug)]
enum JunitExpectedMessage<'a> {
    /// Don't verify the message.
    None,
    /// The message must be present and start with one of the given prefixes.
    /// This is used for failure types where the message format is known but
    /// the exact content is test-specific.
    StartsWithOneOf(&'a [&'a str]),
    /// The message must start with the given prefix and end with the given
    /// suffix. Useful for messages with variable content in the middle (e.g.
    /// attempt numbers).
    StartsAndEndsWith(&'a str, &'a str),
}

impl JunitExpectedMessage<'_> {
    /// Verifies the actual message against this check. Panics with a
    /// diagnostic error on mismatch.
    #[track_caller]
    fn verify(
        &self,
        actual_message: Option<&str>,
        test_name: &str,
        junit_path: &Utf8Path,
        properties: RunProperties,
    ) {
        match self {
            JunitExpectedMessage::None => {}
            JunitExpectedMessage::StartsWithOneOf(prefixes) => {
                let message = actual_message.unwrap_or_else(|| {
                    panic!(
                        "{test_name}: expected a JUnit message starting with one of \
                         {prefixes:?}, but no message was present \
                         (properties: {})\n\
                         JUnit path: {junit_path}",
                        debug_run_properties(properties),
                    );
                });
                // When multiple errors are present, ErrorList formats as
                // "N errors occurred executing test:" and short_message()
                // strips the trailing colon. Accept this as a valid format
                // alongside type-specific prefixes.
                let matches_prefix = prefixes.iter().any(|p| message.starts_with(p));
                let matches_multi_error = message.ends_with("errors occurred executing test");
                assert!(
                    matches_prefix || matches_multi_error,
                    "{test_name}: expected JUnit message starting with one of \
                     {prefixes:?} (or multi-error summary), but got {message:?} \
                     (properties: {})\n\
                     JUnit path: {junit_path}",
                    debug_run_properties(properties),
                );
            }
            JunitExpectedMessage::StartsAndEndsWith(prefix, suffix) => {
                let message = actual_message.unwrap_or_else(|| {
                    panic!(
                        "{test_name}: expected a JUnit message starting with {prefix:?} \
                         and ending with {suffix:?}, but no message was present \
                         (properties: {})\n\
                         JUnit path: {junit_path}",
                        debug_run_properties(properties),
                    );
                });
                assert!(
                    message.starts_with(prefix) && message.ends_with(suffix),
                    "{test_name}: expected JUnit message starting with {prefix:?} \
                     and ending with {suffix:?}, but got {message:?} \
                     (properties: {})\n\
                     JUnit path: {junit_path}",
                    debug_run_properties(properties),
                );
            }
        }
    }
}

/// Returns the expected JUnit properties for a `CheckResult`, or `None` for
/// success cases (Pass, Leak).
///
/// Expected formats from nextest-runner/src/reporter/aggregator/junit.rs:
/// - Fail (exit code, no leak): "test failure with exit code 101"
/// - Fail (exit code, leaked):  "test failure with exit code 101, and leaked handles"
/// - Abort (no leak):           "test abort"
/// - LeakFail:                  "test exited with code 0, but leaked handles so was marked failed"
/// - Timeout:                   "test timeout" or "benchmark timeout"
fn expected_junit_for_result(
    result: CheckResult,
    properties: RunProperties,
) -> Option<ExpectedJunit<'static>> {
    match result {
        // FlakyFailJunitSuccess appears as a success in JUnit (the test
        // is configured with junit.flaky-fail-status = "success").
        CheckResult::Pass | CheckResult::Leak | CheckResult::FlakyFailJunitSuccess => None,
        CheckResult::LeakFail => Some(ExpectedJunit {
            kind: quick_junit::NonSuccessKind::Error,
            ty: "test exited with code 0, but leaked handles so was marked failed",
            // LeakFail tests exit with code 0 (no panic), so there's no error
            // output to extract a message from.
            message: JunitExpectedMessage::None,
        }),
        CheckResult::Fail => Some(ExpectedJunit {
            kind: quick_junit::NonSuccessKind::Failure,
            ty: JUNIT_FAIL,
            message: JunitExpectedMessage::StartsWithOneOf(FAIL_MESSAGE_PREFIXES),
        }),
        CheckResult::FlakyFail => Some(ExpectedJunit {
            kind: quick_junit::NonSuccessKind::Failure,
            ty: JUNIT_FLAKY_FAIL,
            message: JunitExpectedMessage::StartsAndEndsWith(
                "test passed on attempt ",
                " but is configured to fail when flaky",
            ),
        }),
        CheckResult::FailLeak => Some(ExpectedJunit {
            kind: quick_junit::NonSuccessKind::Failure,
            ty: JUNIT_FAIL_LEAK,
            message: JunitExpectedMessage::StartsWithOneOf(FAIL_MESSAGE_PREFIXES),
        }),
        CheckResult::Abort => Some(ExpectedJunit {
            kind: quick_junit::NonSuccessKind::Failure,
            ty: JUNIT_ABORT,
            message: JunitExpectedMessage::StartsWithOneOf(ABORT_MESSAGE_PREFIXES),
        }),
        CheckResult::Timeout => {
            let timeout_kind = if properties.contains(RunProperties::BENCHMARKS) {
                "benchmark timeout"
            } else {
                "test timeout"
            };
            Some(ExpectedJunit {
                kind: quick_junit::NonSuccessKind::Failure,
                ty: timeout_kind,
                // Timed-out tests are killed without producing panic output,
                // and FailureDescription::Timeout doesn't generate an abort
                // description, so there's no error to extract a message from.
                message: JunitExpectedMessage::None,
            })
        }
    }
}

/// Returns the expected rerun kind for a `CheckResult`, or `None` for success
/// cases. `FlakyFail` expects `Flaky` (reruns serialize as `<flakyFailure>`);
/// other failures expect `Rerun` (reruns serialize as `<rerunFailure>`).
fn expected_rerun_kind_for_result(result: CheckResult) -> Option<FlakyOrRerun> {
    match result {
        // FlakyFailJunitSuccess is reported as success in JUnit, so reruns
        // are flaky_runs on a success status (no rerun kind to check).
        CheckResult::Pass | CheckResult::Leak | CheckResult::FlakyFailJunitSuccess => None,
        CheckResult::FlakyFail => Some(FlakyOrRerun::Flaky),
        CheckResult::LeakFail
        | CheckResult::Fail
        | CheckResult::FailLeak
        | CheckResult::Abort
        | CheckResult::Timeout => Some(FlakyOrRerun::Rerun),
    }
}

/// Verifies that all expected tests appear in the JUnit output with the correct
/// status.
#[track_caller]
fn verify_expected_in_junit(
    expected: &ExpectedTestResults,
    actual: &ActualJunitResults,
    junit_path: &Utf8Path,
    properties: RunProperties,
) {
    for expected_outcome in &expected.should_run {
        let actual_outcome = actual.tests.get(&expected_outcome.id);

        match actual_outcome {
            Some(actual) => {
                // Verify the JUnit status matches our expected CheckResult.
                let expected_junit =
                    expected_junit_for_result(expected_outcome.expected.result, properties);

                match (&expected_junit, &actual.non_success) {
                    (None, None) => {
                        // Both expected and actual were successful.
                    }
                    (Some(expected), Some(actual_ns)) => {
                        if expected.kind != actual_ns.kind {
                            panic!(
                                "{}: expected JUnit kind {:?} but got {:?} (properties: {})\n\
                                 JUnit path: {}\n\
                                 Expected result: {:?}, actual type: {:?}",
                                expected_outcome.id.full_name(),
                                expected.kind,
                                actual_ns.kind,
                                debug_run_properties(properties),
                                junit_path,
                                expected_outcome.expected.result,
                                actual_ns.ty,
                            );
                        }

                        if expected.ty != actual_ns.ty {
                            panic!(
                                "{}: expected JUnit type {:?} but got {:?} \
                                 (properties: {})\n\
                                 JUnit path: {}\n\
                                 Expected result: {:?}",
                                expected_outcome.id.full_name(),
                                expected.ty,
                                actual_ns.ty,
                                debug_run_properties(properties),
                                junit_path,
                                expected_outcome.expected.result,
                            );
                        }

                        expected.message.verify(
                            actual_ns.message.as_deref(),
                            &expected_outcome.id.full_name(),
                            junit_path,
                            properties,
                        );
                    }
                    (None, Some(actual_ns)) => {
                        panic!(
                            "{}: expected success, but JUnit shows {:?} with type {:?} \
                             (properties: {})\n\
                             JUnit path: {}\n\
                             Expected result: {:?}",
                            expected_outcome.id.full_name(),
                            actual_ns.kind,
                            actual_ns.ty,
                            debug_run_properties(properties),
                            junit_path,
                            expected_outcome.expected.result,
                        );
                    }
                    (Some(expected), None) => {
                        panic!(
                            "{}: expected JUnit {:?} but got success (properties: {})\n\
                             JUnit path: {}\n\
                             Expected result: {:?}",
                            expected_outcome.id.full_name(),
                            expected.kind,
                            debug_run_properties(properties),
                            junit_path,
                            expected_outcome.expected.result,
                        );
                    }
                }

                // Verify rerun information: count and kind.
                let actual_rerun_count = match &actual.rerun_info {
                    JunitRerunInfo::Success { rerun_count }
                    | JunitRerunInfo::NonSuccess { rerun_count, .. } => *rerun_count,
                };
                match expected_outcome.expected.expected_reruns {
                    ExpectedReruns::None => {
                        assert_eq!(
                            actual_rerun_count,
                            0,
                            "{}: expected no reruns but found {} (properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            actual_rerun_count,
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                    ExpectedReruns::FlakyRunCount(expected_count) => {
                        assert_eq!(
                            actual_rerun_count,
                            expected_count,
                            "{}: expected {} flaky runs but found {} (properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            expected_count,
                            actual_rerun_count,
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                    ExpectedReruns::SomeReruns => {
                        assert!(
                            actual_rerun_count > 0,
                            "{}: expected reruns but found none (properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                }

                // Verify rerun kind matches expectations derived from the
                // check result.
                match (
                    expected_rerun_kind_for_result(expected_outcome.expected.result),
                    &actual.rerun_info,
                ) {
                    (
                        Some(expected_rerun_kind),
                        JunitRerunInfo::NonSuccess {
                            rerun_kind,
                            rerun_count,
                        },
                    ) => {
                        assert_eq!(
                            *rerun_kind,
                            expected_rerun_kind,
                            "{}: expected rerun kind {:?} but got {:?} \
                             (rerun_count: {}, properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            expected_rerun_kind,
                            rerun_kind,
                            rerun_count,
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                    (Some(expected_rerun_kind), JunitRerunInfo::Success { .. }) => {
                        panic!(
                            "{}: expected NonSuccess rerun info (rerun kind {:?}) \
                             but got Success (properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            expected_rerun_kind,
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                    (None, JunitRerunInfo::Success { .. }) => {
                        // Success result with Success rerun info — expected.
                    }
                    (None, JunitRerunInfo::NonSuccess { rerun_kind, .. }) => {
                        panic!(
                            "{}: expected Success rerun info but got NonSuccess \
                             (rerun kind {:?}, properties: {})\n\
                             JUnit path: {}",
                            expected_outcome.id.full_name(),
                            rerun_kind,
                            debug_run_properties(properties),
                            junit_path,
                        );
                    }
                }
            }
            None => {
                panic!(
                    "{}: expected to run with result {:?} but was not found in JUnit output \
                     (properties: {})\n\
                     JUnit path: {}",
                    expected_outcome.id.full_name(),
                    expected_outcome.expected.result,
                    debug_run_properties(properties),
                    junit_path,
                );
            }
        }
    }

    // Check that all tests that should not run are absent from JUnit.
    for id in &expected.should_not_run {
        if actual.tests.contains_key(id) {
            panic!(
                "{}: should not be run but appeared in JUnit output (properties: {})\n\
                 JUnit path: {}",
                id.full_name(),
                debug_run_properties(properties),
                junit_path,
            );
        }
    }
}

/// Verifies that all tests in JUnit output were expected (no unexpected tests).
#[track_caller]
fn verify_junit_in_expected(
    actual: &ActualJunitResults,
    expected: &ExpectedTestResults,
    junit_path: &Utf8Path,
    properties: RunProperties,
) {
    for outcome in &actual.tests {
        if !expected.should_run.contains_key(&outcome.id) {
            // Check if it's in should_not_run to provide a better error message.
            if expected.should_not_run.contains(&outcome.id) {
                panic!(
                    "{}: appeared in JUnit output but should not have been run (properties: {})\n\
                     JUnit path: {}",
                    outcome.id.full_name(),
                    debug_run_properties(properties),
                    junit_path,
                );
            } else {
                panic!(
                    "{}: appeared in JUnit output but was not in expected test set \
                     (not in fixture data or should_not_run) (properties: {})\n\
                     JUnit path: {}",
                    outcome.id.full_name(),
                    debug_run_properties(properties),
                    junit_path,
                );
            }
        }
    }
}
