// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data models for fixture information.

use nextest_metadata::{BuildPlatform, FilterMatch};

#[derive(Copy, Clone, Debug)]
pub struct BinaryFixture {
    pub binary_id: &'static str,
    pub binary_name: &'static str,
    pub build_platform: BuildPlatform,
}

#[derive(Copy, Clone, Debug)]
pub struct TestFixture {
    pub name: &'static str,
    pub status: FixtureStatus,
}

// This isn't great, but it is the easiest way to compare a Vec of TestFixture with a Vec of (&str,
// FilterMatch).
impl PartialEq<(&str, FilterMatch)> for TestFixture {
    fn eq(&self, (name, filter_match): &(&str, FilterMatch)) -> bool {
        &self.name == name && self.status.is_ignored() != filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FixtureStatus {
    Pass,
    Fail,
    Flaky { pass_attempt: usize },
    Leak,
    Segfault,
    IgnoredPass,
    IgnoredFail,
}

impl FixtureStatus {
    pub fn is_ignored(self) -> bool {
        matches!(
            self,
            FixtureStatus::IgnoredPass | FixtureStatus::IgnoredFail
        )
    }
}
