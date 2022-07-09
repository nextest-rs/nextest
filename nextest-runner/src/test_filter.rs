// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filtering tests based on user-specified parameters.
//!
//! The main structure in this module is [`TestFilter`], which is created by a [`TestFilterBuilder`].

#![allow(clippy::nonminimal_bool)]
// nonminimal_bool fires on one of the conditions below and appears to suggest an incorrect
// result

use crate::{
    errors::RunIgnoredParseError,
    helpers::convert_build_platform,
    list::RustTestArtifact,
    partition::{Partitioner, PartitionerBuilder},
};
use aho_corasick::AhoCorasick;
use nextest_filtering::{FilteringExpr, FilteringExprQuery};
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestFilterBuilder {
    run_ignored: RunIgnored,
    partitioner_builder: Option<PartitionerBuilder>,
    name_match: NameMatch,
    exprs: Vec<FilteringExpr>,
}

#[derive(Clone, Debug)]
enum NameMatch {
    EmptyPatterns,
    MatchSet {
        patterns: Vec<String>,
        matcher: Box<AhoCorasick>,
    },
}

impl PartialEq for NameMatch {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::EmptyPatterns, Self::EmptyPatterns) => true,
            (Self::MatchSet { patterns: sp, .. }, Self::MatchSet { patterns: op, .. })
                if sp == op =>
            {
                true
            }
            _ => false,
        }
    }
}

impl Eq for NameMatch {}

impl TestFilterBuilder {
    /// Creates a new `TestFilterBuilder` from the given patterns.
    ///
    /// If an empty slice is passed, the test filter matches all possible test names.
    pub fn new(
        run_ignored: RunIgnored,
        partitioner_builder: Option<PartitionerBuilder>,
        patterns: impl IntoIterator<Item = impl Into<String>>,
        exprs: Vec<FilteringExpr>,
    ) -> Self {
        let mut patterns: Vec<_> = patterns.into_iter().map(|s| s.into()).collect();
        patterns.sort_unstable();

        let name_match = if patterns.is_empty() {
            NameMatch::EmptyPatterns
        } else {
            let matcher = Box::new(AhoCorasick::new_auto_configured(&patterns));

            NameMatch::MatchSet { patterns, matcher }
        };

        Self {
            run_ignored,
            partitioner_builder,
            name_match,
            exprs,
        }
    }

    /// Creates a new `TestFilterBuilder` that matches any pattern by name.
    pub fn any(run_ignored: RunIgnored) -> Self {
        Self {
            run_ignored,
            partitioner_builder: None,
            name_match: NameMatch::EmptyPatterns,
            exprs: Vec::new(),
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
    pub fn filter_match(
        &mut self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &str,
        ignored: bool,
    ) -> FilterMatch {
        self.filter_ignored_mismatch(ignored)
            .or_else(|| {
                use FilterNameMatch::*;
                match (
                    self.filter_name_match(test_name),
                    self.filter_expression_match(test_binary, test_name),
                ) {
                    // if accepted by at least one filtering strategy => accepted
                    (MatchEmptyPatterns, MatchEmptyPatterns)
                    | (MatchWithPatterns, _)
                    | (_, MatchWithPatterns) => None,
                    // if rejected by both filtering strategies, or if one match is empty and is
                    // rejected by the other match => rejected
                    (MatchEmptyPatterns, Mismatch(reason))
                    | (Mismatch(reason), MatchEmptyPatterns)
                    | (Mismatch(reason), Mismatch(_)) => Some(FilterMatch::Mismatch { reason }),
                }
            })
            // Note that partition-based filtering MUST come after all other kinds of filtering,
            // so that count-based bucketing applies after ignored, name and expression matching.
            // This also means that mutable count state must be maintained by the partitioner.
            .or_else(|| self.filter_partition_mismatch(test_name))
            .unwrap_or(FilterMatch::Matches)
    }

    fn filter_ignored_mismatch(&self, ignored: bool) -> Option<FilterMatch> {
        match self.builder.run_ignored {
            RunIgnored::IgnoredOnly => {
                if !ignored {
                    return Some(FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    });
                }
            }
            RunIgnored::Default => {
                if ignored {
                    return Some(FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    });
                }
            }
            _ => {}
        }
        None
    }

    fn filter_name_match(&self, test_name: &str) -> FilterNameMatch {
        match &self.builder.name_match {
            NameMatch::EmptyPatterns => FilterNameMatch::MatchEmptyPatterns,
            NameMatch::MatchSet { matcher, .. } => {
                if matcher.is_match(test_name) {
                    FilterNameMatch::MatchWithPatterns
                } else {
                    FilterNameMatch::Mismatch(MismatchReason::String)
                }
            }
        }
    }

    fn filter_expression_match(
        &self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &str,
    ) -> FilterNameMatch {
        let query = FilteringExprQuery {
            package_id: test_binary.package.id(),
            kind: test_binary.kind.as_str(),
            binary_name: &test_binary.binary_name,
            platform: convert_build_platform(test_binary.build_platform),
            test_name,
        };
        if self.builder.exprs.is_empty() {
            FilterNameMatch::MatchEmptyPatterns
        } else if self.builder.exprs.iter().any(|expr| expr.matches(&query)) {
            FilterNameMatch::MatchWithPatterns
        } else {
            FilterNameMatch::Mismatch(MismatchReason::Expression)
        }
    }

    fn filter_partition_mismatch(&mut self, test_name: &str) -> Option<FilterMatch> {
        let partition_match = match &mut self.partitioner {
            Some(partitioner) => partitioner.test_matches(test_name),
            None => true,
        };
        if partition_match {
            None
        } else {
            Some(FilterMatch::Mismatch {
                reason: MismatchReason::Partition,
            })
        }
    }
}

#[derive(Clone, Debug)]
enum FilterNameMatch {
    /// Match because there are no patterns.
    MatchEmptyPatterns,
    /// Matches with non-empty patterns.
    MatchWithPatterns,
    /// Mismatch.
    Mismatch(MismatchReason),
}

impl FilterNameMatch {
    #[cfg(test)]
    fn is_match(&self) -> bool {
        match self {
            Self::MatchEmptyPatterns | Self::MatchWithPatterns => true,
            Self::Mismatch(_) => false,
        }
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
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, patterns, Vec::new());
            let single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_name_match(&test_name).is_match());
            }
        }

        // Test that exact names match.
        #[test]
        fn proptest_exact(test_names in vec(any::<String>(), 0..16)) {
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &test_names, Vec::new());
            let single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_name_match(&test_name).is_match());
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

            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &patterns, Vec::new());
            let single_filter = test_filter.build();
            for test_name in test_names {
                prop_assert!(single_filter.filter_name_match(&test_name).is_match());
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
            let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &[pattern], Vec::new());
            let single_filter = test_filter.build();
            prop_assert!(!single_filter.filter_name_match(&substring).is_match());
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
