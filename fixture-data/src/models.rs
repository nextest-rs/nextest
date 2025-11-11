// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data models for fixture information.

use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{BuildPlatform, FilterMatch, RustBinaryId};

/// The expected result for a test execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Leak,
    LeakFail,
    Fail,
    FailLeak,
    Abort,
}

/// Properties that control which tests should be run in integration test invocations.
#[derive(Clone, Copy, Debug)]
#[repr(u64)]
pub enum RunProperty {
    Relocated = 0x1,
    WithDefaultFilter = 0x2,
    // --skip cdylib
    WithSkipCdylibFilter = 0x4,
    // --exact test_multiply_two tests::test_multiply_two_cdylib
    WithMultiplyTwoExactFilter = 0x8,
    CdyLibExamplePackageFilter = 0x10,
    SkipSummaryCheck = 0x20,
    ExpectNoBinaries = 0x40,
}

#[derive(Clone, Debug)]
pub struct TestSuiteFixture {
    pub binary_id: RustBinaryId,
    pub binary_name: &'static str,
    pub build_platform: BuildPlatform,
    pub test_cases: IdOrdMap<TestCaseFixture>,
    properties: u64,
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
            properties: 0,
        }
    }

    pub fn with_property(mut self, property: TestSuiteFixtureProperty) -> Self {
        self.properties |= property as u64;
        self
    }

    pub fn has_property(&self, property: TestSuiteFixtureProperty) -> bool {
        self.properties & property as u64 != 0
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(u64)]
pub enum TestSuiteFixtureProperty {
    NotInDefaultSet = 0x1,
    MatchesCdylibExample = 0x2,
}

#[derive(Clone, Debug)]
pub struct TestCaseFixture {
    pub name: &'static str,
    pub status: TestCaseFixtureStatus,
    properties: u64,
}

impl IdOrdItem for TestCaseFixture {
    type Key<'a> = &'static str;
    fn key(&self) -> Self::Key<'_> {
        self.name
    }
    id_upcast!();
}

impl TestCaseFixture {
    pub fn new(name: &'static str, status: TestCaseFixtureStatus) -> Self {
        Self {
            name,
            status,
            properties: 0,
        }
    }

    pub fn with_property(mut self, property: TestCaseFixtureProperty) -> Self {
        self.properties |= property as u64;
        self
    }

    pub fn has_property(&self, property: TestCaseFixtureProperty) -> bool {
        self.properties & property as u64 != 0
    }

    /// Determines if this test should be skipped based on run properties and filters.
    pub fn should_skip(&self, properties: u64) -> bool {
        // NotInDefaultSet filter.
        if self.has_property(TestCaseFixtureProperty::NotInDefaultSet)
            && properties & RunProperty::WithDefaultFilter as u64 != 0
        {
            return true;
        }

        // NotInDefaultSetUnix filter (Unix-specific).
        if cfg!(unix)
            && self.has_property(TestCaseFixtureProperty::NotInDefaultSetUnix)
            && properties & RunProperty::WithDefaultFilter as u64 != 0
        {
            return true;
        }

        // MatchesCdylib + WithSkipCdylibFilter.
        if self.has_property(TestCaseFixtureProperty::MatchesCdylib)
            && properties & RunProperty::WithSkipCdylibFilter as u64 != 0
        {
            return true;
        }

        // WithMultiplyTwoExactFilter - skip tests that don't match.
        if !self.has_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo)
            && properties & RunProperty::WithMultiplyTwoExactFilter as u64 != 0
        {
            return true;
        }

        // CdyLibExamplePackageFilter - only run test_multiply_two_cdylib.
        if properties & RunProperty::CdyLibExamplePackageFilter as u64 != 0
            && self.name != "tests::test_multiply_two_cdylib"
        {
            return true;
        }

        // ExpectNoBinaries - all tests should be skipped.
        if properties & RunProperty::ExpectNoBinaries as u64 != 0 {
            return true;
        }

        // Ignored tests are skipped by this test suite.
        if self.status.is_ignored() {
            return true;
        }

        false
    }

    /// Determines the expected test result based on test status and run properties.
    pub fn expected_result(&self, properties: u64) -> CheckResult {
        match self.status {
            TestCaseFixtureStatus::Pass => {
                // NeedsSameCwd tests fail when relocated.
                if self.has_property(TestCaseFixtureProperty::NeedsSameCwd)
                    && properties & RunProperty::Relocated as u64 != 0
                {
                    CheckResult::Fail
                } else {
                    CheckResult::Pass
                }
            }
            TestCaseFixtureStatus::Leak => CheckResult::Leak,
            TestCaseFixtureStatus::LeakFail => CheckResult::LeakFail,
            TestCaseFixtureStatus::Fail | TestCaseFixtureStatus::Flaky { .. } => {
                // Flaky tests are not currently retried by this test suite.
                CheckResult::Fail
            }
            TestCaseFixtureStatus::FailLeak => CheckResult::FailLeak,
            TestCaseFixtureStatus::Segfault => CheckResult::Abort,
            TestCaseFixtureStatus::IgnoredPass | TestCaseFixtureStatus::IgnoredFail => {
                unreachable!("ignored tests should be filtered out")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestNameAndFilterMatch<'a> {
    pub name: &'a str,
    pub filter_match: FilterMatch,
}

impl<'a> IdOrdItem for TestNameAndFilterMatch<'a> {
    type Key<'k>
        = &'a str
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
        self.name == other.name && self.status.is_ignored() != other.filter_match.is_match()
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(u64)]
pub enum TestCaseFixtureProperty {
    NeedsSameCwd = 0x1,
    NotInDefaultSet = 0x2,
    MatchesCdylib = 0x4,
    MatchesTestMultiplyTwo = 0x8,
    NotInDefaultSetUnix = 0x10,
}
