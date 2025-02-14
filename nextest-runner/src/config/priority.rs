// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::TestPriorityOutOfRange;
use serde::{Deserialize, Deserializer};

/// A test priority: a number between -100 and 100.
///
/// The sort order is from highest to lowest priority.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TestPriority(i8);

impl TestPriority {
    /// Creates a new `TestPriority`.
    pub fn new(priority: i8) -> Result<Self, TestPriorityOutOfRange> {
        if !(-100..=100).contains(&priority) {
            return Err(TestPriorityOutOfRange { priority });
        }
        Ok(Self(priority))
    }

    /// Returns the priority as an `i8`.
    pub fn to_i8(self) -> i8 {
        self.0
    }
}

impl PartialOrd for TestPriority {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TestPriority {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse the order to sort from highest to lowest priority.
        other.0.cmp(&self.0)
    }
}

impl<'de> Deserialize<'de> for TestPriority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let priority = i8::deserialize(deserializer)?;
        TestPriority::new(priority).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_out_of_range() {
        let priority = TestPriority::new(-101);
        priority.expect_err("priority must be between -100 and 100");

        let priority = TestPriority::new(101);
        priority.expect_err("priority must be between -100 and 100");
    }

    #[test]
    fn priority_deserialize() {
        let priority: TestPriority = serde_json::from_str("-100").unwrap();
        assert_eq!(priority.to_i8(), -100);

        let priority: TestPriority = serde_json::from_str("0").unwrap();
        assert_eq!(priority.to_i8(), 0);

        let priority: TestPriority = serde_json::from_str("100").unwrap();
        assert_eq!(priority.to_i8(), 100);

        let priority: Result<TestPriority, _> = serde_json::from_str("-101");
        priority.expect_err("priority must be between -100 and 100");

        let priority: Result<TestPriority, _> = serde_json::from_str("101");
        priority.expect_err("priority must be between -100 and 100");
    }

    #[test]
    fn priority_sort_order() {
        let mut priorities = vec![
            TestPriority::new(0).unwrap(),
            TestPriority::new(100).unwrap(),
            TestPriority::new(-100).unwrap(),
        ];
        priorities.sort();
        assert_eq!(
            priorities,
            [
                TestPriority::new(100).unwrap(),
                TestPriority::new(0).unwrap(),
                TestPriority::new(-100).unwrap()
            ]
        );
    }
}
