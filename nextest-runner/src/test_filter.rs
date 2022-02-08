// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filtering tests based on user-specified parameters.
//!
//! The main structure in this module is [`TestFilter`], which is created by a [`TestFilterBuilder`].

use crate::{
    errors::RunIgnoredParseError,
    partition::{Partitioner, PartitionerBuilder},
};
use aho_corasick::AhoCorasick;
use nextest_metadata::{FilterMatch, MismatchReason};
use std::{fmt, str::FromStr};

/// Whether to run ignored tests.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RunIgnored {
    /// Only run tests that aren't ignored.
    ///
    /// This is the default.
    Default,

    /// Only run tests that are ignored.
    IgnoredOnly,

    /// Run both ignored and non-ignored tests.
    All,
}

impl RunIgnored {
    /// String representations of all known variants.
    pub fn variants() -> &'static [&'static str] {
        &["default", "ignored-only", "all"]
    }
}

impl Default for RunIgnored {
    fn default() -> Self {
        RunIgnored::Default
    }
}

impl fmt::Display for RunIgnored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunIgnored::Default => write!(f, "default"),
            RunIgnored::IgnoredOnly => write!(f, "ignored-only"),
            RunIgnored::All => write!(f, "all"),
        }
    }
}

impl FromStr for RunIgnored {
    type Err = RunIgnoredParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "default" => RunIgnored::Default,
            "ignored-only" => RunIgnored::IgnoredOnly,
            "all" => RunIgnored::All,
            other => return Err(RunIgnoredParseError::new(other)),
        };
        Ok(val)
    }
}

/// A builder for `TestFilter` instances.
#[derive(Clone, Debug)]
pub struct TestFilterBuilder {
    run_ignored: RunIgnored,
    partitioner_builder: Option<PartitionerBuilder>,
    name_match: NameMatch,
}

#[derive(Clone, Debug)]
enum NameMatch {
    MatchAll,
    MatchSet(Box<AhoCorasick>),
}

impl TestFilterBuilder {
    /// Creates a new `TestFilterBuilder` from the given patterns.
    ///
    /// If an empty slice is passed, the test filter matches all possible test names.
    pub fn new(
        run_ignored: RunIgnored,
        partitioner_builder: Option<PartitionerBuilder>,
        patterns: &[impl AsRef<[u8]>],
    ) -> Self {
        let name_match = if patterns.is_empty() {
            NameMatch::MatchAll
        } else {
            NameMatch::MatchSet(Box::new(AhoCorasick::new_auto_configured(patterns)))
        };
        Self {
            run_ignored,
            partitioner_builder,
            name_match,
        }
    }

    /// Creates a new `TestFilterBuilder` that matches any pattern by name.
    pub fn any(run_ignored: RunIgnored) -> Self {
        Self {
            run_ignored,
            partitioner_builder: None,
            name_match: NameMatch::MatchAll,
        }
    }

    /// Creates a new test filter scoped to a single binary.
    ///
    /// This test filter may be stateful.
    pub fn build(&self) -> TestFilter<'_> {
        let partitioner = self
            .partitioner_builder
            .as_ref()
            .map(|partitioner_builder| partitioner_builder.build());
        TestFilter {
            builder: self,
            partitioner,
        }
    }
}

/// Test filter, scoped to a single binary.
#[derive(Debug)]
pub struct TestFilter<'builder> {
    builder: &'builder TestFilterBuilder,
    partitioner: Option<Box<dyn Partitioner>>,
}

impl<'filter> TestFilter<'filter> {
    /// Returns an enum describing the match status of this filter.
    pub fn filter_match(&mut self, test_name: &str, ignored: bool) -> FilterMatch {
        match self.builder.run_ignored {
            RunIgnored::IgnoredOnly => {
                if !ignored {
                    return FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    };
                }
            }
            RunIgnored::Default => {
                if ignored {
                    return FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    };
                }
            }
            _ => {}
        };

        let string_match = match &self.builder.name_match {
            NameMatch::MatchAll => true,
            NameMatch::MatchSet(set) => set.is_match(test_name),
        };
        if !string_match {
            return FilterMatch::Mismatch {
                reason: MismatchReason::String,
            };
        }

        let partition_match = match &mut self.partitioner {
            Some(partitioner) => partitioner.test_matches(test_name),
            None => true,
        };
        if !partition_match {
            return FilterMatch::Mismatch {
                reason: MismatchReason::Partition,
            };
        }

        FilterMatch::Matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::vec, prelude::*};

    proptest! {
        #[test]
        fn proptest_empty(test_names in vec(any::<String>(), 0..16)) {
            let patterns: &[String] = &[];
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, patterns);
            let mut single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that exact names match.
        #[test]
        fn proptest_exact(test_names in vec(any::<String>(), 0..16)) {
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &test_names);
            let mut single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that substrings match.
        #[test]
        fn proptest_substring(
            substring_prefix_suffixes in vec([any::<String>(); 3], 0..16),
        ) {
            let mut patterns = Vec::with_capacity(substring_prefix_suffixes.len());
            let mut test_names = Vec::with_capacity(substring_prefix_suffixes.len());
            for [substring, prefix, suffix] in substring_prefix_suffixes {
                test_names.push(prefix + &substring + &suffix);
                patterns.push(substring);
            }

            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &patterns);
            let mut single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that dropping a character from a string doesn't match.
        #[test]
        fn proptest_no_match(
            substring in any::<String>(),
            prefix in any::<String>(),
            suffix in any::<String>(),
        ) {
            prop_assume!(!substring.is_empty() && !(prefix.is_empty() && suffix.is_empty()));
            let pattern = prefix + &substring + &suffix;
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &[&pattern]);
            let mut single_filter = test_filter.build();
            prop_assert!(!single_filter.filter_match(&substring, false).is_match());
        }
    }

    // /// Creates a fake test binary instance.
    // fn make_test_binary() -> TestBinary {
    //     TestBinary {
    //         binary: "/fake/path".into(),
    //         binary_id: "fake-id".to_owned(),
    //         cwd: "/fake".into(),
    //     }
    // }
}
