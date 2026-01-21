// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filtering tests based on user-specified parameters.
//!
//! The main structure in this module is [`TestFilter`], which is created by a [`TestFilterBuilder`].

use crate::{
    errors::TestFilterBuilderError,
    list::RustTestArtifact,
    partition::{Partitioner, PartitionerBuilder},
    record::ComputedRerunInfo,
    run_mode::NextestRunMode,
};
use aho_corasick::AhoCorasick;
use nextest_filtering::{EvalContext, Filterset, TestQuery};
use nextest_metadata::{FilterMatch, MismatchReason, RustTestKind, TestCaseName};
use std::{collections::HashSet, fmt, mem};

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

#[derive(Clone, Debug, Eq, PartialEq)]
/// Filters Binaries based on `TestFilterExprs`.
pub struct BinaryFilter {
    exprs: TestFilterExprs,
}

impl BinaryFilter {
    /// Creates a new `BinaryFilter` from `exprs`.
    ///
    /// If `exprs` is an empty slice, all binaries will match.
    pub fn new(exprs: Vec<Filterset>) -> Self {
        let exprs = if exprs.is_empty() {
            TestFilterExprs::All
        } else {
            TestFilterExprs::Sets(exprs)
        };
        Self { exprs }
    }

    /// Returns a value indicating whether this binary should or should not be run to obtain the
    /// list of tests within it.
    pub fn check_match(
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
}

/// A builder for `TestFilter` instances.
#[derive(Clone, Debug)]
pub struct TestFilterBuilder {
    mode: NextestRunMode,
    rerun_info: Option<ComputedRerunInfo>,
    run_ignored: RunIgnored,
    partitioner_builder: Option<PartitionerBuilder>,
    patterns: ResolvedFilterPatterns,
    binary_filter: BinaryFilter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestFilterExprs {
    /// No filtersets specified to filter against -- match the default set of tests.
    All,

    /// Filtersets to match against. A match can be against any of the sets.
    Sets(Vec<Filterset>),
}

/// A set of string-based patterns for test filters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestFilterPatterns {
    /// The only patterns specified (if any) are skip patterns: match the default set of tests minus
    /// the skip patterns.
    SkipOnly {
        /// Skip patterns.
        skip_patterns: Vec<String>,

        /// Skip patterns to match exactly.
        skip_exact_patterns: HashSet<String>,
    },

    /// At least one substring or exact pattern is specified.
    ///
    /// In other words, at least one of `patterns` or `exact_patterns` should be non-empty.
    ///
    /// A fully empty `Patterns` is logically sound (will match no tests), but never created by
    /// nextest itself.
    Patterns {
        /// Substring patterns.
        patterns: Vec<String>,

        /// Patterns to match exactly.
        exact_patterns: HashSet<String>,

        /// Patterns passed in via `--skip`.
        skip_patterns: Vec<String>,

        /// Skip patterns to match exactly.
        skip_exact_patterns: HashSet<String>,
    },
}

impl Default for TestFilterPatterns {
    fn default() -> Self {
        Self::SkipOnly {
            skip_patterns: Vec::new(),
            skip_exact_patterns: HashSet::new(),
        }
    }
}

impl TestFilterPatterns {
    /// Initializes a new `TestFilterPatterns` with a set of substring patterns specified before
    /// `--`.
    ///
    /// An empty slice matches all tests.
    pub fn new(substring_patterns: Vec<String>) -> Self {
        if substring_patterns.is_empty() {
            Self::default()
        } else {
            Self::Patterns {
                patterns: substring_patterns,
                exact_patterns: HashSet::new(),
                skip_patterns: Vec::new(),
                skip_exact_patterns: HashSet::new(),
            }
        }
    }

    /// Adds a regular pattern to the set of patterns.
    pub fn add_substring_pattern(&mut self, pattern: String) {
        match self {
            Self::SkipOnly {
                skip_patterns,
                skip_exact_patterns,
            } => {
                *self = Self::Patterns {
                    patterns: vec![pattern],
                    exact_patterns: HashSet::new(),
                    skip_patterns: mem::take(skip_patterns),
                    skip_exact_patterns: mem::take(skip_exact_patterns),
                };
            }
            Self::Patterns { patterns, .. } => {
                patterns.push(pattern);
            }
        }
    }

    /// Adds an exact pattern to the set of patterns.
    pub fn add_exact_pattern(&mut self, pattern: String) {
        match self {
            Self::SkipOnly {
                skip_patterns,
                skip_exact_patterns,
            } => {
                *self = Self::Patterns {
                    patterns: Vec::new(),
                    exact_patterns: [pattern].into_iter().collect(),
                    skip_patterns: mem::take(skip_patterns),
                    skip_exact_patterns: mem::take(skip_exact_patterns),
                };
            }
            Self::Patterns { exact_patterns, .. } => {
                exact_patterns.insert(pattern);
            }
        }
    }

    /// Adds a skip pattern to the set of patterns.
    pub fn add_skip_pattern(&mut self, pattern: String) {
        match self {
            Self::SkipOnly { skip_patterns, .. } => {
                skip_patterns.push(pattern);
            }
            Self::Patterns { skip_patterns, .. } => {
                skip_patterns.push(pattern);
            }
        }
    }

    /// Adds a skip pattern to match exactly.
    pub fn add_skip_exact_pattern(&mut self, pattern: String) {
        match self {
            Self::SkipOnly {
                skip_exact_patterns,
                ..
            } => {
                skip_exact_patterns.insert(pattern);
            }
            Self::Patterns {
                skip_exact_patterns,
                ..
            } => {
                skip_exact_patterns.insert(pattern);
            }
        }
    }

    fn resolve(self) -> Result<ResolvedFilterPatterns, TestFilterBuilderError> {
        match self {
            Self::SkipOnly {
                mut skip_patterns,
                skip_exact_patterns,
            } => {
                if skip_patterns.is_empty() {
                    Ok(ResolvedFilterPatterns::All)
                } else {
                    // sort_unstable allows the PartialEq implementation to work correctly.
                    skip_patterns.sort_unstable();
                    let skip_pattern_matcher = Box::new(AhoCorasick::new(&skip_patterns)?);
                    Ok(ResolvedFilterPatterns::SkipOnly {
                        skip_patterns,
                        skip_pattern_matcher,
                        skip_exact_patterns,
                    })
                }
            }
            Self::Patterns {
                mut patterns,
                exact_patterns,
                mut skip_patterns,
                skip_exact_patterns,
            } => {
                // sort_unstable allows the PartialEq implementation to work correctly.
                patterns.sort_unstable();
                skip_patterns.sort_unstable();

                let pattern_matcher = Box::new(AhoCorasick::new(&patterns)?);
                let skip_pattern_matcher = Box::new(AhoCorasick::new(&skip_patterns)?);

                Ok(ResolvedFilterPatterns::Patterns {
                    patterns,
                    exact_patterns,
                    skip_patterns,
                    skip_exact_patterns,
                    pattern_matcher,
                    skip_pattern_matcher,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
enum ResolvedFilterPatterns {
    /// Match all tests.
    ///
    /// This is mostly for convenience -- it's equivalent to `SkipOnly` with an empty set of skip
    /// patterns.
    #[default]
    All,

    /// Match all tests except those that match the skip patterns.
    SkipOnly {
        skip_patterns: Vec<String>,
        skip_pattern_matcher: Box<AhoCorasick>,
        skip_exact_patterns: HashSet<String>,
    },

    /// Match tests that match the patterns and don't match the skip patterns.
    Patterns {
        patterns: Vec<String>,
        exact_patterns: HashSet<String>,
        skip_patterns: Vec<String>,
        skip_exact_patterns: HashSet<String>,
        pattern_matcher: Box<AhoCorasick>,
        skip_pattern_matcher: Box<AhoCorasick>,
    },
}

impl ResolvedFilterPatterns {
    fn name_match(&self, test_name: &TestCaseName) -> FilterNameMatch {
        let test_name = test_name.as_str();
        match self {
            Self::All => FilterNameMatch::MatchEmptyPatterns,
            Self::SkipOnly {
                // skip_patterns is covered by the matcher.
                skip_patterns: _,
                skip_exact_patterns,
                skip_pattern_matcher,
            } => {
                if skip_exact_patterns.contains(test_name)
                    || skip_pattern_matcher.is_match(test_name)
                {
                    FilterNameMatch::Mismatch(MismatchReason::String)
                } else {
                    FilterNameMatch::MatchWithPatterns
                }
            }
            Self::Patterns {
                // patterns is covered by the matcher.
                patterns: _,
                exact_patterns,
                // skip_patterns is covered by the matcher.
                skip_patterns: _,
                skip_exact_patterns,
                pattern_matcher,
                skip_pattern_matcher,
            } => {
                // skip overrides all other patterns.
                if skip_exact_patterns.contains(test_name)
                    || skip_pattern_matcher.is_match(test_name)
                {
                    FilterNameMatch::Mismatch(MismatchReason::String)
                } else if exact_patterns.contains(test_name) || pattern_matcher.is_match(test_name)
                {
                    FilterNameMatch::MatchWithPatterns
                } else {
                    FilterNameMatch::Mismatch(MismatchReason::String)
                }
            }
        }
    }
}

impl PartialEq for ResolvedFilterPatterns {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All, Self::All) => true,
            (
                Self::SkipOnly {
                    skip_patterns,
                    skip_exact_patterns,
                    // The matcher is derived from `skip_patterns`, so it can be ignored.
                    skip_pattern_matcher: _,
                },
                Self::SkipOnly {
                    skip_patterns: other_skip_patterns,
                    skip_exact_patterns: other_skip_exact_patterns,
                    skip_pattern_matcher: _,
                },
            ) => {
                skip_patterns == other_skip_patterns
                    && skip_exact_patterns == other_skip_exact_patterns
            }
            (
                Self::Patterns {
                    patterns,
                    exact_patterns,
                    skip_patterns,
                    skip_exact_patterns,
                    // The matchers are derived from `patterns` and `skip_patterns`, so they can be
                    // ignored.
                    pattern_matcher: _,
                    skip_pattern_matcher: _,
                },
                Self::Patterns {
                    patterns: other_patterns,
                    exact_patterns: other_exact_patterns,
                    skip_patterns: other_skip_patterns,
                    skip_exact_patterns: other_skip_exact_patterns,
                    pattern_matcher: _,
                    skip_pattern_matcher: _,
                },
            ) => {
                patterns == other_patterns
                    && exact_patterns == other_exact_patterns
                    && skip_patterns == other_skip_patterns
                    && skip_exact_patterns == other_skip_exact_patterns
            }
            _ => false,
        }
    }
}

impl Eq for ResolvedFilterPatterns {}

impl TestFilterBuilder {
    /// Creates a new `TestFilterBuilder` from the given patterns.
    ///
    /// If an empty slice is passed, the test filter matches all possible test names.
    pub fn new(
        mode: NextestRunMode,
        run_ignored: RunIgnored,
        partitioner_builder: Option<PartitionerBuilder>,
        patterns: TestFilterPatterns,
        exprs: Vec<Filterset>,
    ) -> Result<Self, TestFilterBuilderError> {
        let patterns = patterns.resolve()?;

        let binary_filter = BinaryFilter::new(exprs);

        Ok(Self {
            mode,
            rerun_info: None,
            run_ignored,
            partitioner_builder,
            patterns,
            binary_filter,
        })
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
        self.binary_filter.check_match(test_binary, ecx, bound)
    }

    /// Creates a new `TestFilterBuilder` that matches the default set of tests.
    pub fn default_set(mode: NextestRunMode, run_ignored: RunIgnored) -> Self {
        let binary_filter = BinaryFilter::new(Vec::new());
        Self {
            mode,
            rerun_info: None,
            run_ignored,
            partitioner_builder: None,
            patterns: ResolvedFilterPatterns::default(),
            binary_filter,
        }
    }

    /// Set the list of outstanding tests, if this is a rerun.
    pub fn set_outstanding_tests(&mut self, rerun_info: ComputedRerunInfo) {
        self.rerun_info = Some(rerun_info);
    }

    /// Returns the nextest execution mode.
    pub fn mode(&self) -> NextestRunMode {
        self.mode
    }

    /// Compares the patterns between two `TestFilterBuilder`s.
    pub fn patterns_eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
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

    /// Consumes self, returning the underlying [`ComputedRerunInfo`] if any.
    pub fn into_rerun_info(self) -> Option<ComputedRerunInfo> {
        self.rerun_info
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

impl TestFilter<'_> {
    /// Returns an enum describing the match status of this filter.
    pub fn filter_match(
        &mut self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &TestCaseName,
        test_kind: &RustTestKind,
        ecx: &EvalContext<'_>,
        bound: FilterBound,
        ignored: bool,
    ) -> FilterMatch {
        // Handle benchmark mismatches first.
        if let Some(mismatch) = self.filter_benchmark_mismatch(test_kind) {
            return mismatch;
        }

        // Check if this test already passed in a prior rerun.
        //
        // RerunAlreadyPassed is a high-order bit: if a test passed in a prior
        // rerun, we shouldn't run it again, regardless of other filter results.
        // However, we must still go through the motions of checking all other
        // filters, particularly for counted partitioning, to maintain
        // consistent bucketing across reruns.
        //
        // Note that we don't support reruns with benchmarks yet (probably
        // ever?), so NotABenchmark and RerunAlreadyPassed are mutually
        // exclusive.
        if self.is_rerun_already_passed(test_binary, test_name) {
            // Run through the base filter to maintain partition counts.
            let _ = self.filter_match_base(test_binary, test_name, ecx, bound, ignored);
            return FilterMatch::Mismatch {
                reason: MismatchReason::RerunAlreadyPassed,
            };
        }

        self.filter_match_base(test_binary, test_name, ecx, bound, ignored)
    }

    /// Core filter matching logic, used by `filter_match`.
    fn filter_match_base(
        &mut self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &TestCaseName,
        ecx: &EvalContext<'_>,
        bound: FilterBound,
        ignored: bool,
    ) -> FilterMatch {
        if let Some(mismatch) = self.filter_ignored_mismatch(ignored) {
            return mismatch;
        }

        {
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
                ) => {}
                // If rejected by at least one of the filtering strategies, the test is
                // rejected. Note we use the _name_ mismatch reason first. That's because
                // expression-based matches can also match against the default set. If a test
                // fails both name and expression matches, then the name reason is more directly
                // relevant.
                (Mismatch(reason), _) | (_, Mismatch(reason)) => {
                    return FilterMatch::Mismatch { reason };
                }
            }
        }

        // Note that partition-based filtering MUST come after all other kinds
        // of filtering, so that count-based bucketing applies after ignored,
        // name, and expression matching. This also means that mutable count
        // state must be maintained by the partitioner.
        if let Some(mismatch) = self.filter_partition_mismatch(test_name) {
            return mismatch;
        }

        FilterMatch::Matches
    }

    /// Returns true if this test already passed in a prior rerun.
    fn is_rerun_already_passed(
        &self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &TestCaseName,
    ) -> bool {
        if let Some(rerun_info) = &self.builder.rerun_info
            && let Some(suite) = rerun_info.test_suites.get(&test_binary.binary_id)
        {
            return suite.passing.contains(test_name);
        }
        false
    }

    fn filter_benchmark_mismatch(&self, test_kind: &RustTestKind) -> Option<FilterMatch> {
        if self.builder.mode == NextestRunMode::Benchmark && test_kind != &RustTestKind::BENCH {
            Some(FilterMatch::Mismatch {
                reason: MismatchReason::NotBenchmark,
            })
        } else {
            None
        }
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

    fn filter_name_match(&self, test_name: &TestCaseName) -> FilterNameMatch {
        self.builder.patterns.name_match(test_name)
    }

    fn filter_expression_match(
        &self,
        test_binary: &RustTestArtifact<'_>,
        test_name: &TestCaseName,
        ecx: &EvalContext<'_>,
        bound: FilterBound,
    ) -> FilterNameMatch {
        let query = TestQuery {
            binary_query: test_binary.to_binary_query(),
            test_name,
        };

        let expr_result = match &self.builder.binary_filter.exprs {
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

    fn filter_partition_mismatch(&mut self, test_name: &TestCaseName) -> Option<FilterMatch> {
        let partition_match = match &mut self.partitioner {
            Some(partitioner) => partitioner.test_matches(test_name.as_str()),
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
        let patterns = TestFilterPatterns::default();
        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns,
            Vec::new(),
        )
        .unwrap();
        let single_filter = test_filter.build();
        for test_name in test_names {
            let test_name = TestCaseName::new(&test_name);
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that exact names match.
    #[proptest(cases = 50)]
    fn proptest_exact(#[strategy(vec(any::<String>(), 0..16))] test_names: Vec<String>) {
        // Test with the default matcher.
        let patterns = TestFilterPatterns::new(test_names.clone());
        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns,
            Vec::new(),
        )
        .unwrap();
        let single_filter = test_filter.build();
        for test_name in &test_names {
            let test_name = TestCaseName::new(test_name);
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }

        // Test with the exact matcher.
        let mut patterns = TestFilterPatterns::default();
        for test_name in &test_names {
            patterns.add_exact_pattern(test_name.clone());
        }
        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns,
            Vec::new(),
        )
        .unwrap();
        let single_filter = test_filter.build();
        for test_name in &test_names {
            let test_name = TestCaseName::new(test_name);
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that substrings match.
    #[proptest(cases = 50)]
    fn proptest_substring(
        #[strategy(vec([any::<String>(); 3], 0..16))] substring_prefix_suffixes: Vec<[String; 3]>,
    ) {
        let mut patterns = TestFilterPatterns::default();
        let mut test_names = Vec::with_capacity(substring_prefix_suffixes.len());
        for [substring, prefix, suffix] in substring_prefix_suffixes {
            test_names.push(prefix + &substring + &suffix);
            patterns.add_substring_pattern(substring);
        }

        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns,
            Vec::new(),
        )
        .unwrap();
        let single_filter = test_filter.build();
        for test_name in test_names {
            let test_name = TestCaseName::new(&test_name);
            prop_assert!(single_filter.filter_name_match(&test_name).is_match());
        }
    }

    // Test that dropping a character from a string doesn't match.
    #[proptest(cases = 50)]
    fn proptest_no_match(substring: String, prefix: String, suffix: String) {
        prop_assume!(!substring.is_empty() && !prefix.is_empty() && !suffix.is_empty());
        let pattern = prefix + &substring + &suffix;
        let patterns = TestFilterPatterns::new(vec![pattern]);
        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns,
            Vec::new(),
        )
        .unwrap();
        let single_filter = test_filter.build();
        let substring = TestCaseName::new(&substring);
        prop_assert!(!single_filter.filter_name_match(&substring).is_match());
    }

    fn test_name(s: &str) -> TestCaseName {
        TestCaseName::new(s)
    }

    #[test]
    fn pattern_examples() {
        let mut patterns = TestFilterPatterns::new(vec!["foo".to_string()]);
        patterns.add_substring_pattern("bar".to_string());
        patterns.add_exact_pattern("baz".to_string());
        patterns.add_skip_pattern("quux".to_string());
        patterns.add_skip_exact_pattern("quuz".to_string());

        let resolved = patterns.clone().resolve().unwrap();

        // Test substring matches.
        assert_eq!(
            resolved.name_match(&test_name("foo")),
            FilterNameMatch::MatchWithPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("1foo2")),
            FilterNameMatch::MatchWithPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("bar")),
            FilterNameMatch::MatchWithPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("x_bar_y")),
            FilterNameMatch::MatchWithPatterns,
        );

        // Test exact matches.
        assert_eq!(
            resolved.name_match(&test_name("baz")),
            FilterNameMatch::MatchWithPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("abazb")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );

        // Both substring and exact matches.
        assert_eq!(
            resolved.name_match(&test_name("bazfoo")),
            FilterNameMatch::MatchWithPatterns,
        );

        // Skip patterns.
        assert_eq!(
            resolved.name_match(&test_name("quux")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
        assert_eq!(
            resolved.name_match(&test_name("1quux2")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );

        // Skip and substring patterns.
        assert_eq!(
            resolved.name_match(&test_name("quuxbar")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );

        // Skip-exact patterns.
        assert_eq!(
            resolved.name_match(&test_name("quuz")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );

        // Skip overrides regular patterns -- in this case, add `baz` to the skip list.
        patterns.add_skip_pattern("baz".to_string());
        let resolved = patterns.resolve().unwrap();
        assert_eq!(
            resolved.name_match(&test_name("quuxbaz")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
    }

    #[test]
    fn skip_only_pattern_examples() {
        let mut patterns = TestFilterPatterns::default();
        patterns.add_skip_pattern("foo".to_string());
        patterns.add_skip_pattern("bar".to_string());
        patterns.add_skip_exact_pattern("baz".to_string());

        let resolved = patterns.clone().resolve().unwrap();

        // Test substring matches.
        assert_eq!(
            resolved.name_match(&test_name("foo")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
        assert_eq!(
            resolved.name_match(&test_name("1foo2")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
        assert_eq!(
            resolved.name_match(&test_name("bar")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
        assert_eq!(
            resolved.name_match(&test_name("x_bar_y")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );

        // Test exact matches.
        assert_eq!(
            resolved.name_match(&test_name("baz")),
            FilterNameMatch::Mismatch(MismatchReason::String),
        );
        assert_eq!(
            resolved.name_match(&test_name("abazb")),
            FilterNameMatch::MatchWithPatterns,
        );

        // Anything that doesn't match the skip filter should match.
        assert_eq!(
            resolved.name_match(&test_name("quux")),
            FilterNameMatch::MatchWithPatterns,
        );
    }

    #[test]
    fn empty_pattern_examples() {
        let patterns = TestFilterPatterns::default();
        let resolved = patterns.resolve().unwrap();
        assert_eq!(resolved, ResolvedFilterPatterns::All);

        // Anything matches.
        assert_eq!(
            resolved.name_match(&test_name("foo")),
            FilterNameMatch::MatchEmptyPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("1foo2")),
            FilterNameMatch::MatchEmptyPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("bar")),
            FilterNameMatch::MatchEmptyPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("x_bar_y")),
            FilterNameMatch::MatchEmptyPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("baz")),
            FilterNameMatch::MatchEmptyPatterns,
        );
        assert_eq!(
            resolved.name_match(&test_name("abazb")),
            FilterNameMatch::MatchEmptyPatterns,
        );
    }
}
