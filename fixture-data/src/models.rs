// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data models for fixture information.

use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{BuildPlatform, FilterMatch, RustBinaryId, TestCaseName};

/// The expected result for a test execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Leak,
    LeakFail,
    Fail,
    FailLeak,
    Abort,
    Timeout,
}

bitflags::bitflags! {
    /// Properties that control which tests should be run in integration test invocations.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RunProperties: u64 {
        const RELOCATED = 0x1;
        const WITH_DEFAULT_FILTER = 0x2;
        // --skip cdylib
        const WITH_SKIP_CDYLIB_FILTER = 0x4;
        // --exact test_multiply_two tests::test_multiply_two_cdylib
        const WITH_MULTIPLY_TWO_EXACT_FILTER = 0x8;
        const CDYLIB_EXAMPLE_PACKAGE_FILTER = 0x10;
        const SKIP_SUMMARY_CHECK = 0x20;
        const EXPECT_NO_BINARIES = 0x40;
        const BENCHMARKS = 0x80;
        /// Run ignored benchmarks with the `with-bench-override` profile.
        const BENCH_OVERRIDE_TIMEOUT = 0x100;
        /// Run ignored benchmarks with the `with-bench-termination` profile.
        const BENCH_TERMINATION = 0x200;
        /// Run benchmarks with the `with-test-termination-only` profile.
        const BENCH_IGNORES_TEST_TIMEOUT = 0x400;
        /// Run ignored tests only (--run-ignored only), excluding slow timeout tests.
        const RUN_IGNORED_ONLY = 0x800;
        /// Run with with-timeout-retries-success profile, slow_timeout tests only.
        /// These tests time out but pass due to on-timeout=pass.
        const TIMEOUT_RETRIES_PASS = 0x1000;
        /// Run with with-timeout-retries-success profile, flaky slow timeout test only.
        /// This test fails twice then times out (passes) on the 3rd attempt.
        const TIMEOUT_RETRIES_FLAKY = 0x2000;
        /// Run with the with-retries profile. Flaky tests should pass after retries.
        const WITH_RETRIES = 0x4000;
        /// Run with a target runner set. On Unix, segfaults are reported as regular
        /// failures because the passthrough runner doesn't propagate signal info.
        const WITH_TARGET_RUNNER = 0x8000;
        /// Run with the with-termination profile. Tests should time out.
        const WITH_TERMINATION = 0x10000;
        /// Run with the with-timeout-success profile. test_slow_timeout passes
        /// (on-timeout = "pass"), others fail.
        const WITH_TIMEOUT_SUCCESS = 0x20000;
        /// Allow skipped test names to appear in output (e.g., for replay which shows SKIP lines).
        /// Without this flag, verification fails if any skipped test name appears in the output.
        const ALLOW_SKIPPED_NAMES_IN_OUTPUT = 0x40000;
    }
}

#[derive(Clone, Debug)]
pub struct TestSuiteFixture {
    pub binary_id: RustBinaryId,
    pub binary_name: &'static str,
    pub build_platform: BuildPlatform,
    pub test_cases: IdOrdMap<TestCaseFixture>,
    properties: TestSuiteFixtureProperties,
}

impl IdOrdItem for TestSuiteFixture {
    type Key<'a> = &'a RustBinaryId;
    fn key(&self) -> Self::Key<'_> {
        &self.binary_id
    }
    id_upcast!();
}

impl TestSuiteFixture {
    pub fn new(
        binary_id: &'static str,
        binary_name: &'static str,
        build_platform: BuildPlatform,
        test_cases: IdOrdMap<TestCaseFixture>,
    ) -> Self {
        Self {
            binary_id: binary_id.into(),
            binary_name,
            build_platform,
            test_cases,
            properties: TestSuiteFixtureProperties::empty(),
        }
    }

    pub fn with_property(mut self, property: TestSuiteFixtureProperties) -> Self {
        self.properties |= property;
        self
    }

    pub fn has_property(&self, property: TestSuiteFixtureProperties) -> bool {
        self.properties.contains(property)
    }

    pub fn assert_test_cases_match(&self, other: &IdOrdMap<TestNameAndFilterMatch<'_>>) {
        if self.test_cases.len() != other.len() {
            panic!(
                "test cases mismatch: expected {} test cases, found {}; \
                 expected: {self:#?}, actual: {other:#?}",
                self.test_cases.len(),
                other.len(),
            );
        }

        for name_and_filter_match in other {
            if let Some(test_case) = self.test_cases.get(name_and_filter_match.name) {
                if test_case.status.is_ignored() == name_and_filter_match.filter_match.is_match() {
                    panic!(
                        "test case status mismatch for '{}': expected {:?}, found {:?}; \
                         expected: {self:#?}, actual: {other:#?}",
                        name_and_filter_match.name,
                        test_case.status,
                        name_and_filter_match.filter_match,
                    );
                }
            } else {
                panic!(
                    "test case '{}' not found in test suite '{}'; \
                     expected: {self:#?}, actual: {other:#?}",
                    name_and_filter_match.name, self.binary_name,
                );
            }
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct TestSuiteFixtureProperties: u64 {
        const NOT_IN_DEFAULT_SET = 0x1;
        const MATCHES_CDYLIB_EXAMPLE = 0x2;
    }
}

#[derive(Clone, Debug)]
pub struct TestCaseFixture {
    pub name: TestCaseName,
    pub status: TestCaseFixtureStatus,
    properties: TestCaseFixtureProperties,
}

impl IdOrdItem for TestCaseFixture {
    type Key<'a> = &'a TestCaseName;
    fn key(&self) -> Self::Key<'_> {
        &self.name
    }
    id_upcast!();
}

impl TestCaseFixture {
    pub fn new(name: &str, status: TestCaseFixtureStatus) -> Self {
        Self {
            name: TestCaseName::new(name),
            status,
            properties: TestCaseFixtureProperties::empty(),
        }
    }

    pub fn with_property(mut self, property: TestCaseFixtureProperties) -> Self {
        self.properties |= property;
        self
    }

    pub fn has_property(&self, property: TestCaseFixtureProperties) -> bool {
        self.properties.contains(property)
    }

    /// Determines if this test should be skipped based on run properties and filters.
    pub fn should_skip(&self, properties: RunProperties) -> bool {
        // NotInDefaultSet filter.
        if self.has_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET)
            && properties.contains(RunProperties::WITH_DEFAULT_FILTER)
        {
            return true;
        }

        // NotInDefaultSetUnix filter (Unix-specific).
        if cfg!(unix)
            && self.has_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET_UNIX)
            && properties.contains(RunProperties::WITH_DEFAULT_FILTER)
        {
            return true;
        }

        // MatchesCdylib + WithSkipCdylibFilter.
        if self.has_property(TestCaseFixtureProperties::MATCHES_CDYLIB)
            && properties.contains(RunProperties::WITH_SKIP_CDYLIB_FILTER)
        {
            return true;
        }

        // WithMultiplyTwoExactFilter - skip tests that don't match.
        if !self.has_property(TestCaseFixtureProperties::MATCHES_TEST_MULTIPLY_TWO)
            && properties.contains(RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER)
        {
            return true;
        }

        // CdyLibExamplePackageFilter - only run test_multiply_two_cdylib.
        if properties.contains(RunProperties::CDYLIB_EXAMPLE_PACKAGE_FILTER)
            && self.name != TestCaseName::new("tests::test_multiply_two_cdylib")
        {
            return true;
        }

        // ExpectNoBinaries - all tests should be skipped.
        if properties.contains(RunProperties::EXPECT_NO_BINARIES) {
            return true;
        }

        // BenchOverrideTimeout - only run the specific benchmark that times out.
        if properties.contains(RunProperties::BENCH_OVERRIDE_TIMEOUT) {
            return !self.has_property(TestCaseFixtureProperties::BENCH_OVERRIDE_TIMEOUT);
        }

        // BenchTermination - only run the specific benchmark that times out.
        if properties.contains(RunProperties::BENCH_TERMINATION) {
            return !self.has_property(TestCaseFixtureProperties::BENCH_TERMINATION);
        }

        // BenchIgnoresTestTimeout - only run the specific benchmark that passes.
        if properties.contains(RunProperties::BENCH_IGNORES_TEST_TIMEOUT) {
            return !self.has_property(TestCaseFixtureProperties::BENCH_IGNORES_TEST_TIMEOUT);
        }

        // TIMEOUT_RETRIES_PASS - only run tests with the
        // TEST_SLOW_TIMEOUT_SUBSTRING property (not benchmarks). These are the
        // test_slow_timeout* tests that time out but pass.
        if properties.contains(RunProperties::TIMEOUT_RETRIES_PASS) {
            // Skip if not SLOW_TIMEOUT or if it's a benchmark.
            return !self.has_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING)
                || self.has_property(TestCaseFixtureProperties::IS_BENCHMARK);
        }

        // TIMEOUT_RETRIES_FLAKY - only run the flaky slow timeout test.
        if properties.contains(RunProperties::TIMEOUT_RETRIES_FLAKY) {
            return !self.has_property(TestCaseFixtureProperties::FLAKY_SLOW_TIMEOUT_SUBSTRING);
        }

        // WITH_TERMINATION - only run test_slow_timeout* tests (they time out).
        if properties.contains(RunProperties::WITH_TERMINATION) {
            return !self.has_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING)
                || self.has_property(TestCaseFixtureProperties::IS_BENCHMARK);
        }

        // WITH_TIMEOUT_SUCCESS - only run test_slow_timeout* tests.
        if properties.contains(RunProperties::WITH_TIMEOUT_SUCCESS) {
            return !self.has_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING)
                || self.has_property(TestCaseFixtureProperties::IS_BENCHMARK);
        }

        // RUN_IGNORED_ONLY: run only ignored tests, excluding slow timeout
        // tests.
        if properties.contains(RunProperties::RUN_IGNORED_ONLY) {
            // Skip slow timeout tests (filtered out in the test).
            if self.has_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING) {
                return true;
            }
            // Skip non-ignored tests.
            if !self.status.is_ignored() {
                return true;
            }
            // Run other ignored tests.
            return false;
        }

        // Ignored tests are skipped by this test suite.
        if self.status.is_ignored() {
            return true;
        }

        false
    }

    /// Determines the expected test result based on test status and run properties.
    pub fn expected_result(&self, properties: RunProperties) -> CheckResult {
        // BenchOverrideTimeout - the benchmark times out due to override.
        if self.has_property(TestCaseFixtureProperties::BENCH_OVERRIDE_TIMEOUT)
            && properties.contains(RunProperties::BENCH_OVERRIDE_TIMEOUT)
        {
            return CheckResult::Timeout;
        }

        // BenchTermination - the benchmark times out due to bench.slow-timeout.
        if self.has_property(TestCaseFixtureProperties::BENCH_TERMINATION)
            && properties.contains(RunProperties::BENCH_TERMINATION)
        {
            return CheckResult::Timeout;
        }

        // BenchIgnoresTestTimeout - the benchmark passes because it uses
        // bench.slow-timeout (30 years default) instead of slow-timeout.
        if self.has_property(TestCaseFixtureProperties::BENCH_IGNORES_TEST_TIMEOUT)
            && properties.contains(RunProperties::BENCH_IGNORES_TEST_TIMEOUT)
        {
            return CheckResult::Pass;
        }

        // TIMEOUT_RETRIES_PASS - tests time out but pass due to on-timeout=pass.
        // The output shows PASS, not TIMEOUT.
        if self.has_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING)
            && properties.contains(RunProperties::TIMEOUT_RETRIES_PASS)
        {
            return CheckResult::Pass;
        }

        // TIMEOUT_RETRIES_FLAKY - test is flaky (fails twice, then times out and passes).
        // The output shows PASS, not TIMEOUT.
        if self.has_property(TestCaseFixtureProperties::FLAKY_SLOW_TIMEOUT_SUBSTRING)
            && properties.contains(RunProperties::TIMEOUT_RETRIES_FLAKY)
        {
            return CheckResult::Pass;
        }

        // WITH_TERMINATION - all test_slow_timeout* tests time out.
        if self.has_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING)
            && properties.contains(RunProperties::WITH_TERMINATION)
        {
            return CheckResult::Timeout;
        }

        // WITH_TIMEOUT_SUCCESS - test_slow_timeout passes (on-timeout = "pass"),
        // while other test_slow_timeout* tests fail.
        if properties.contains(RunProperties::WITH_TIMEOUT_SUCCESS) {
            if self.has_property(TestCaseFixtureProperties::EXACT_TEST_SLOW_TIMEOUT) {
                // test_slow_timeout has on-timeout = "pass" override.
                return CheckResult::Pass;
            }
            if self.has_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING) {
                // Other test_slow_timeout* tests time out normally.
                return CheckResult::Timeout;
            }
        }

        match self.status {
            TestCaseFixtureStatus::Pass => {
                // NeedsSameCwd tests fail when relocated.
                if self.has_property(TestCaseFixtureProperties::NEEDS_SAME_CWD)
                    && properties.contains(RunProperties::RELOCATED)
                {
                    CheckResult::Fail
                } else {
                    CheckResult::Pass
                }
            }
            TestCaseFixtureStatus::Leak => CheckResult::Leak,
            TestCaseFixtureStatus::LeakFail => CheckResult::LeakFail,
            TestCaseFixtureStatus::Fail => CheckResult::Fail,
            TestCaseFixtureStatus::Flaky { .. } => {
                // With retries, flaky tests eventually pass. (Retries are
                // configured in a way which ensures that all tests eventually
                // pass.)
                if properties.contains(RunProperties::WITH_RETRIES) {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail
                }
            }
            TestCaseFixtureStatus::FailLeak => CheckResult::FailLeak,
            TestCaseFixtureStatus::Segfault => {
                // On Unix, segfaults aren't passed through by the passthrough runner.
                // They show as regular failures instead of aborts.
                if cfg!(unix) && properties.contains(RunProperties::WITH_TARGET_RUNNER) {
                    CheckResult::Fail
                } else {
                    CheckResult::Abort
                }
            }
            TestCaseFixtureStatus::IgnoredPass => {
                if properties.contains(RunProperties::RUN_IGNORED_ONLY) {
                    CheckResult::Pass
                } else {
                    unreachable!("ignored tests should be filtered out")
                }
            }
            TestCaseFixtureStatus::IgnoredFail => {
                if properties.contains(RunProperties::RUN_IGNORED_ONLY) {
                    CheckResult::Fail
                } else {
                    unreachable!("ignored tests should be filtered out")
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestNameAndFilterMatch<'a> {
    pub name: &'a TestCaseName,
    pub filter_match: FilterMatch,
}

impl<'a> IdOrdItem for TestNameAndFilterMatch<'a> {
    type Key<'k>
        = &'a TestCaseName
    where
        Self: 'k;
    fn key(&self) -> Self::Key<'_> {
        self.name
    }
    id_upcast!();
}

// This isn't great, but it is the easiest way to compare an IdOrdMap of
// TestFixture with an IdOrdMap of TestNameAndFilterMatch.
impl PartialEq<TestNameAndFilterMatch<'_>> for TestCaseFixture {
    fn eq(&self, other: &TestNameAndFilterMatch<'_>) -> bool {
        self.name == *other.name && self.status.is_ignored() != other.filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TestCaseFixtureStatus {
    Pass,
    Fail,
    Flaky { pass_attempt: u32 },
    Leak,
    LeakFail,
    FailLeak,
    Segfault,
    IgnoredPass,
    IgnoredFail,
}

impl TestCaseFixtureStatus {
    pub fn is_ignored(self) -> bool {
        matches!(
            self,
            TestCaseFixtureStatus::IgnoredPass | TestCaseFixtureStatus::IgnoredFail
        )
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct TestCaseFixtureProperties: u64 {
        const NEEDS_SAME_CWD = 0x1;
        const NOT_IN_DEFAULT_SET = 0x2;
        const MATCHES_CDYLIB = 0x4;
        const MATCHES_TEST_MULTIPLY_TWO = 0x8;
        const NOT_IN_DEFAULT_SET_UNIX = 0x10;
        const IS_BENCHMARK = 0x20;
        /// Benchmark that times out with the with-bench-override profile.
        const BENCH_OVERRIDE_TIMEOUT = 0x40;
        /// Benchmark that times out with the with-bench-termination profile.
        const BENCH_TERMINATION = 0x80;
        /// Benchmark that passes with the with-test-termination-only profile.
        const BENCH_IGNORES_TEST_TIMEOUT = 0x100;
        /// Test with "slow_timeout" as a substring.
        const SLOW_TIMEOUT_SUBSTRING = 0x200;
        /// Test with "test_slow_timeout" as a substring.
        const TEST_SLOW_TIMEOUT_SUBSTRING = 0x400;
        /// Test with "flaky_slow_timeout" as a substring.
        const FLAKY_SLOW_TIMEOUT_SUBSTRING = 0x800;
        /// Exactly test_slow_timeout (not test_slow_timeout_2 or test_slow_timeout_subprocess).
        const EXACT_TEST_SLOW_TIMEOUT = 0x1000;
    }
}
