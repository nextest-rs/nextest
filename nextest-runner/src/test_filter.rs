// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filtering tests based on user-specified parameters.
//!
//! The main structure in this module is [`TestFilter`], which is created by a [`TestFilterBuilder`].

#![allow(clippy::nonminimal_bool)]
// nonminimal_bool fires on one of the conditions below and appears to suggest an incorrect
// result

use crate::{
    errors::TestFilterBuilderError,
    list::RustTestArtifact,
    partition::{Partitioner, PartitionerBuilder},
};
use aho_corasick::AhoCorasick;
use nextest_filtering::{EvalContext, Filterset, TestQuery};
use nextest_metadata::{FilterMatch, MismatchReason};
use std::fmt;

/// Whether to run ignored tests.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum RunIgnored {
    /// Only run tests that aren't ignored.
    ///
    /// This is the default.
    #[default]
    Default,

    /// Only run tests that are ignored.
    Only,

    /// Run both ignored and non-ignored tests.
    All,
}

/// A higher-level filter.
#[derive(Clone, Copy, Debug)]
pub enum FilterBound {
    /// Filter with the default set.
    DefaultSet,

    /// Do not perform any higher-level filtering.
    All,
}

/// A builder for `TestFilter` instances.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestFilterBuilder {
    run_ignored: RunIgnored,
    partitioner_builder: Option<PartitionerBuilder>,
    name_match: NameMatch,
    exprs: TestFilterExprs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestFilterExprs {
    /// No filtersets specified to filter against -- match the default set of tests.
    All,

    /// Filtersets to match against. A match can be against any of the sets.
    Sets(Vec<Filterset>),
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
        exprs: Vec<Filterset>,
    ) -> Result<Self, TestFilterBuilderError> {
        let mut patterns: Vec<_> = patterns.into_iter().map(|s| s.into()).collect();
        patterns.sort_unstable();

        let name_match = if patterns.is_empty() {
            NameMatch::EmptyPatterns
        } else {
            let matcher = Box::new(AhoCorasick::new(&patterns)?);

            NameMatch::MatchSet { patterns, matcher }
        };

        let exprs = if exprs.is_empty() {
            TestFilterExprs::All
        } else {
            TestFilterExprs::Sets(exprs)
        };

        Ok(Self {
            run_ignored,
            partitioner_builder,
            name_match,
            exprs,
        })
    }

    /// Creates a new `TestFilterBuilder` that matches the default set of tests.
    pub fn default_set(run_ignored: RunIgnored) -> Self {
        Self {
            run_ignored,
            partitioner_builder: None,
            name_match: NameMatch::EmptyPatterns,
            exprs: TestFilterExprs::All,
        }
    }

    /// Returns a value indicating whether this binary should or should not be run to obtain the
    /// list of tests within it.
    ///
    /// This method is implemented directly on `TestFilterBuilder`. The statefulness of `TestFilter`
    /// is only used for counted test partitioning, and is not currently relevant for binaries.
    pub fn filter_binary_match(
        &self,
        test_binary: &RustTestArtifact<'_>,
        ecx: &EvalContext<'_>,
        bound: FilterBound,
    ) -> FilterBinaryMatch {
        let query = test_binary.to_binary_query();
        let expr_result = match &self.exprs {
            TestFilterExprs::All => FilterBinaryMatch::Definite,
            TestFilterExprs::Sets(exprs) => exprs.iter().fold(
                FilterBinaryMatch::Mismatch {
                    // Just use this as a placeholder as the lowest possible value.
                    reason: BinaryMismatchReason::Expression,
                },
                |acc, expr| {
                    acc.logic_or(FilterBinaryMatch::from_result(
                        expr.matches_binary(&query, ecx),
                        BinaryMismatchReason::Expression,
                    ))
                },
            ),
        };

        // If none of the expressions matched, then there's no need to check the default set.
        if !expr_result.is_match() {
            return expr_result;
        }

        match bound {
            FilterBound::All => expr_result,
            FilterBound::DefaultSet => expr_result.logic_and(FilterBinaryMatch::from_result(
                ecx.default_filter.matches_binary(&query, ecx),
                BinaryMismatchReason::DefaultSet,
            )),
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

/// Whether a binary matched filters and should be run to obtain the list of tests within.
///
/// The result of [`TestFilterBuilder::filter_binary_match`].
#[derive(Copy, Clone, Debug)]
pub enum FilterBinaryMatch {
    /// This is a definite match -- binaries should be run.
    Definite,

    /// We don't know for sure -- binaries should be run.
    Possible,

    /// This is a definite mismatch -- binaries should not be run.
    Mismatch {
        /// The reason for the mismatch.
        reason: BinaryMismatchReason,
    },
}

impl FilterBinaryMatch {
    fn from_result(result: Option<bool>, reason: BinaryMismatchReason) -> Self {
        match result {
            Some(true) => Self::Definite,
            None => Self::Possible,
            Some(false) => Self::Mismatch { reason },
        }
    }

    fn is_match(self) -> bool {
        match self {
            Self::Definite | Self::Possible => true,
            Self::Mismatch { .. } => false,
        }
    }

    fn logic_or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Definite, _) | (_, Self::Definite) => Self::Definite,
            (Self::Possible, _) | (_, Self::Possible) => Self::Possible,
            (Self::Mismatch { reason: r1 }, Self::Mismatch { reason: r2 }) => Self::Mismatch {
                reason: r1.prefer_expression(r2),
            },
        }
    }

    fn logic_and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Definite, Self::Definite) => Self::Definite,
            (Self::Definite, Self::Possible)
            | (Self::Possible, Self::Definite)
            | (Self::Possible, Self::Possible) => Self::Possible,
            (Self::Mismatch { reason: r1 }, Self::Mismatch { reason: r2 }) => {
                // If one of the mismatch reasons is `Expression` and the other is `DefaultSet`, we
                // return Expression.
                Self::Mismatch {
                    reason: r1.prefer_expression(r2),
                }
            }
            (Self::Mismatch { reason }, _) | (_, Self::Mismatch { reason }) => {
                Self::Mismatch { reason }
            }
        }
    }
}

/// The reason for a binary mismatch.
///
/// Part of [`FilterBinaryMatch`], as returned by [`TestFilterBuilder::filter_binary_match`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BinaryMismatchReason {
    /// The binary doesn't match any of the provided filtersets.
    Expression,

    /// No filtersets were specified and the binary doesn't match the default set.
    DefaultSet,
}

impl BinaryMismatchReason {
    fn prefer_expression(self, other: Self) -> Self {
        match (self, other) {
            (Self::Expression, _) | (_, Self::Expression) => Self::Expression,
            (Self::DefaultSet, Self::DefaultSet) => Self::DefaultSet,
        }
    }
}

impl fmt::Display for BinaryMismatchReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expression => write!(f, "didn't match filtersets"),
            Self::DefaultSet => write!(f, "didn't match the default set"),
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
        ecx: &EvalContext<'_>,
        bound: FilterBound,
        ignored: bool,
    ) -> FilterMatch {
        self.filter_ignored_mismatch(ignored)
            .or_else(|| {
                // ---
                // NOTE
                // ---
                //
                // Previously, if either expression OR string filters matched, we'd run the tests.
                // The current (stable) implementation is that *both* the expression AND the string
                // filters should match.
                //
                // This is because we try and skip running test binaries which don't match
                // expression filters. So for example:
                //
                //     cargo nextest run -E 'binary(foo)' test_bar
                //
                // would not even get to the point of enumerating the tests not in binary(foo), thus
                // not running any test_bars in the workspace. But, with the OR semantics:
                //
                //     cargo nextest run -E 'binary(foo) or test(test_foo)' test_bar
                //
                // would run all the test_bars in the repo. This is inconsistent, so nextest must
                // use AND semantics.
                use FilterNameMatch::*;
                match (
                    self.filter_name_match(test_name),
                    self.filter_expression_match(test_binary, test_name, ecx, bound),
                ) {
                    // Tests must be accepted by both expressions and filters.
                    (
                        MatchEmptyPatterns | MatchWithPatterns,
                        MatchEmptyPatterns | MatchWithPatterns,
                    ) => None,
                    // If rejected by at least one of the filtering strategies, the test is
                    // rejected. Note we use the _name_ mismatch reason first. That's because
                    // expression-based matches can also match against the default set. If a test
                    // fails both name and expression matches, then the name reason is more directly
                    // relevant.
                    (Mismatch(reason), _) | (_, Mismatch(reason)) => {
                        Some(FilterMatch::Mismatch { reason })
                    }
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
            RunIgnored::Only => {
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
        ecx: &EvalContext<'_>,
        bound: FilterBound,
    ) -> FilterNameMatch {
        let query = TestQuery {
            binary_query: test_binary.to_binary_query(),
            test_name,
        };

        let expr_result = match &self.builder.exprs {
            TestFilterExprs::All => FilterNameMatch::MatchEmptyPatterns,
            TestFilterExprs::Sets(exprs) => {
                if exprs.iter().any(|expr| expr.matches_test(&query, ecx)) {
                    FilterNameMatch::MatchWithPatterns
                } else {
                    return FilterNameMatch::Mismatch(MismatchReason::Expression);
                }
            }
        };

        match bound {
            FilterBound::All => expr_result,
            FilterBound::DefaultSet => {
                if ecx.default_filter.matches_test(&query, ecx) {
                    expr_result
                } else {
                    FilterNameMatch::Mismatch(MismatchReason::DefaultFilter)
                }
            }
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
    use test_strategy::proptest;

    #[proptest(cases = 50)]
    fn proptest_empty(#[strategy(vec(any::<String>(), 0..16))] test_names: Vec<String>) {
        let patterns: &[String] = &[];
        let test_filter =
            TestFilterBuilder::new(RunIgnored::Default, None, patterns, Vec::new()).unwrap();
        let single_filter = test_filter.build();
        for test_name in test_names {
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that exact names match.
    #[proptest(cases = 50)]
    fn proptest_exact(#[strategy(vec(any::<String>(), 0..16))] test_names: Vec<String>) {
        let test_filter =
            TestFilterBuilder::new(RunIgnored::Default, None, &test_names, Vec::new()).unwrap();
        let single_filter = test_filter.build();
        for test_name in test_names {
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that substrings match.
    #[proptest(cases = 50)]
    fn proptest_substring(
        #[strategy(vec([any::<String>(); 3], 0..16))] substring_prefix_suffixes: Vec<[String; 3]>,
    ) {
        let mut patterns = Vec::with_capacity(substring_prefix_suffixes.len());
        let mut test_names = Vec::with_capacity(substring_prefix_suffixes.len());
        for [substring, prefix, suffix] in substring_prefix_suffixes {
            test_names.push(prefix + &substring + &suffix);
            patterns.push(substring);
        }

        let test_filter =
            TestFilterBuilder::new(RunIgnored::Default, None, &patterns, Vec::new()).unwrap();
        let single_filter = test_filter.build();
        for test_name in test_names {
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that dropping a character from a string doesn't match.
    #[proptest(cases = 50)]
    fn proptest_no_match(substring: String, prefix: String, suffix: String) {
        prop_assume!(!substring.is_empty() && !(prefix.is_empty() && suffix.is_empty()));
        let pattern = prefix + &substring + &suffix;
        let test_filter =
            TestFilterBuilder::new(RunIgnored::Default, None, [pattern], Vec::new()).unwrap();
        let single_filter = test_filter.build();
        prop_assert!(!single_filter.filter_name_match(&substring).is_match());
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
