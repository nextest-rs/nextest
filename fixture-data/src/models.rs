// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data models for fixture information.

use nextest_metadata::{BuildPlatform, FilterMatch, RustBinaryId};

#[derive(Clone, Debug)]
pub struct TestSuiteFixture {
    pub binary_id: RustBinaryId,
    pub binary_name: &'static str,
    pub build_platform: BuildPlatform,
    pub test_cases: Vec<TestCaseFixture>,
    properties: u64,
}

impl TestSuiteFixture {
    pub fn new(
        binary_id: &'static str,
        binary_name: &'static str,
        build_platform: BuildPlatform,
        test_cases: Vec<TestCaseFixture>,
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
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(u64)]
pub enum TestSuiteFixtureProperty {
    NotInDefaultSet = 1,
}

#[derive(Clone, Debug)]
pub struct TestCaseFixture {
    pub name: &'static str,
    pub status: TestCaseFixtureStatus,
    properties: u64,
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
}

// This isn't great, but it is the easiest way to compare a Vec of TestFixture with a Vec of (&str,
// FilterMatch).
impl PartialEq<(&str, FilterMatch)> for TestCaseFixture {
    fn eq(&self, (name, filter_match): &(&str, FilterMatch)) -> bool {
        &self.name == name && self.status.is_ignored() != filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TestCaseFixtureStatus {
    Pass,
    Fail,
    Flaky { pass_attempt: usize },
    Leak,
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
    NeedsSameCwd = 1,
    NotInDefaultSet = 2,
}
