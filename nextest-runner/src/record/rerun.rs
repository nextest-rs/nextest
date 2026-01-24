// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Rerun support for nextest.
//!
//! This module provides types and functions for rerunning tests that failed or
//! didn't complete in a previous recorded run.

use crate::{
    errors::RecordReadError,
    list::OwnedTestInstanceId,
    record::{
        CoreEventKind, OutputEventKind, RecordReader, TestEventKindSummary,
        format::{RerunInfo, RerunRootInfo, RerunTestSuiteInfo},
    },
};
use iddqd::IdOrdMap;
use nextest_metadata::{
    FilterMatch, MismatchReason, RustBinaryId, RustTestSuiteStatusSummary, TestCaseName,
    TestListSummary,
};
use quick_junit::ReportUuid;
use std::collections::{BTreeSet, HashMap};

/// Trait abstracting over test list access for rerun computation.
///
/// This allows the same logic to work with both the real [`TestListSummary`]
/// and a simplified model for property-based testing.
pub(crate) trait TestListInfo {
    /// Iterator type for binaries.
    type BinaryIter<'a>: Iterator<Item = (&'a RustBinaryId, BinaryInfo<'a>)>
    where
        Self: 'a;

    /// Returns an iterator over all binaries in the test list.
    fn binaries(&self) -> Self::BinaryIter<'_>;
}

/// Information about a single binary in the test list.
pub(crate) enum BinaryInfo<'a> {
    /// Binary was listed; contains test cases.
    Listed {
        /// Iterator over test cases: (name, filter match).
        test_cases: Box<dyn Iterator<Item = (&'a TestCaseName, FilterMatch)> + 'a>,
    },
    /// Binary was skipped (not listed).
    Skipped,
}

impl TestListInfo for TestListSummary {
    type BinaryIter<'a> = TestListSummaryBinaryIter<'a>;

    fn binaries(&self) -> Self::BinaryIter<'_> {
        TestListSummaryBinaryIter {
            inner: self.rust_suites.iter(),
        }
    }
}

/// Iterator over binaries in a [`TestListSummary`].
pub(crate) struct TestListSummaryBinaryIter<'a> {
    inner:
        std::collections::btree_map::Iter<'a, RustBinaryId, nextest_metadata::RustTestSuiteSummary>,
}

impl<'a> Iterator for TestListSummaryBinaryIter<'a> {
    type Item = (&'a RustBinaryId, BinaryInfo<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(binary_id, suite)| {
            let info = if suite.status == RustTestSuiteStatusSummary::LISTED {
                BinaryInfo::Listed {
                    test_cases: Box::new(
                        suite
                            .test_cases
                            .iter()
                            .map(|(name, tc)| (name, tc.filter_match)),
                    ),
                }
            } else {
                BinaryInfo::Skipped
            };
            (binary_id, info)
        })
    }
}

/// Pure computation of outstanding tests.
pub(crate) fn compute_outstanding_pure(
    prev_info: Option<&IdOrdMap<RerunTestSuiteInfo>>,
    test_list: &impl TestListInfo,
    outcomes: &HashMap<OwnedTestInstanceId, TestOutcome>,
) -> IdOrdMap<RerunTestSuiteInfo> {
    let mut new_outstanding = IdOrdMap::new();

    // Track which binaries were in the test list (listed or skipped) so we can
    // distinguish between "binary is in test list but has no tests to track"
    // vs "binary is not in test list at all".
    let mut binaries_in_test_list = BTreeSet::new();

    for (binary_id, binary_info) in test_list.binaries() {
        binaries_in_test_list.insert(binary_id.clone());

        match binary_info {
            BinaryInfo::Listed { test_cases } => {
                // The binary was listed, so we can rely on the set of test cases
                // produced by it.
                let prev = prev_info.and_then(|p| p.get(binary_id));

                let mut curr = RerunTestSuiteInfo::new(binary_id.clone());
                for (test_name, filter_match) in test_cases {
                    match filter_match {
                        FilterMatch::Matches => {
                            // This test should have been run.
                            let key = OwnedTestInstanceId {
                                binary_id: binary_id.clone(),
                                test_name: test_name.clone(),
                            };
                            match outcomes.get(&key) {
                                Some(TestOutcome::Passed) => {
                                    // This test passed.
                                    curr.passing.insert(test_name.clone());
                                }
                                Some(TestOutcome::Failed) => {
                                    // This test failed, and so is outstanding.
                                    curr.outstanding.insert(test_name.clone());
                                }
                                Some(TestOutcome::Skipped(skipped)) => {
                                    // This is strange! FilterMatch::Matches means
                                    // the test should not be skipped. But compute
                                    // this anyway.
                                    handle_skipped(test_name, *skipped, prev, &mut curr);
                                }
                                None => {
                                    // The test was scheduled, but was not seen in
                                    // the event log. It must be re-run.
                                    curr.outstanding.insert(test_name.clone());
                                }
                            }
                        }
                        FilterMatch::Mismatch { reason } => {
                            handle_skipped(
                                test_name,
                                TestOutcomeSkipped::from_mismatch_reason(reason),
                                prev,
                                &mut curr,
                            );
                        }
                    }
                }

                // Any outstanding tests that were not accounted for in the
                // loop above should be carried forward, since we're still
                // tracking them.
                if let Some(prev) = prev {
                    for t in &prev.outstanding {
                        if !curr.passing.contains(t) && !curr.outstanding.contains(t) {
                            curr.outstanding.insert(t.clone());
                        }
                    }
                }

                // What about tests that were originally passing, and now not
                // present? We want to treat them as implicitly outstanding (not
                // actively tracking, but if they show up again we'll want to
                // re-run them).

                // Only insert if there are tests to track.
                if !curr.passing.is_empty() || !curr.outstanding.is_empty() {
                    new_outstanding
                        .insert_unique(curr)
                        .expect("binaries iterator should not yield duplicates");
                }
            }
            BinaryInfo::Skipped => {
                // The suite was not listed.
                //
                // If this is an original run, then there's not much we can do. (If
                // the subsequent rerun causes a test to be included, it will be run
                // by dint of not being in the passing set.)
                //
                // If this is a rerun, then we should carry forward the cached list
                // of passing tests for this binary. The next time the binary is
                // seen, we'll reuse the serialized cached list.
                if let Some(prev_outstanding) = prev_info
                    && let Some(outstanding) = prev_outstanding.get(binary_id)
                {
                    // We know the set of outstanding tests.
                    new_outstanding
                        .insert_unique(outstanding.clone())
                        .expect("binaries iterator should not yield duplicates");
                }
                // Else: An interesting case -- the test suite was discovered but
                // not listed, and also was not known. Not much we can do
                // here for now, but maybe we want to track this explicitly
                // in the future?
            }
        }
    }

    // Carry forward binaries from previous run that are not in the current test
    // list at all (neither listed nor skipped).
    if let Some(prev) = prev_info {
        for prev_suite in prev.iter() {
            if !binaries_in_test_list.contains(&prev_suite.binary_id) {
                new_outstanding
                    .insert_unique(prev_suite.clone())
                    .expect("binary not in test list, so this should succeed");
            }
        }
    }

    new_outstanding
}

/// Result of computing outstanding and passing tests from a recorded run.
#[derive(Clone, Debug)]
pub struct ComputedRerunInfo {
    /// The set of tests that are outstanding.
    ///
    /// This set is serialized into `rerun-info.json`.
    pub test_suites: IdOrdMap<RerunTestSuiteInfo>,
}

impl ComputedRerunInfo {
    /// Returns the set of all outstanding test instance IDs.
    ///
    /// This is used to track which tests were expected to run in a rerun.
    pub fn expected_test_ids(&self) -> BTreeSet<OwnedTestInstanceId> {
        self.test_suites
            .iter()
            .flat_map(|suite| {
                suite.outstanding.iter().map(|name| OwnedTestInstanceId {
                    binary_id: suite.binary_id.clone(),
                    test_name: name.clone(),
                })
            })
            .collect()
    }

    /// Computes outstanding tests from a recorded run.
    ///
    /// If this is a rerun chain, also returns information about the root of the
    /// chain.
    pub fn compute(
        reader: &mut RecordReader,
    ) -> Result<(Self, Option<RerunRootInfo>), RecordReadError> {
        let rerun_info = reader.read_rerun_info()?;
        let test_list = reader.read_test_list()?;
        let outcomes = TestEventOutcomes::collect(reader)?;

        let prev_test_suites = rerun_info.as_ref().map(|info| &info.test_suites);
        let new_test_suites =
            compute_outstanding_pure(prev_test_suites, &test_list, &outcomes.outcomes);

        let root_info = rerun_info.map(|info| info.root_info);

        Ok((
            Self {
                test_suites: new_test_suites,
            },
            root_info,
        ))
    }

    /// Consumes self, converting to a [`RerunInfo`] for storage.
    pub fn into_rerun_info(self, parent_run_id: ReportUuid, root_info: RerunRootInfo) -> RerunInfo {
        RerunInfo {
            parent_run_id,
            root_info,
            test_suites: self.test_suites,
        }
    }
}

fn handle_skipped(
    test_name: &TestCaseName,
    skipped: TestOutcomeSkipped,
    prev: Option<&RerunTestSuiteInfo>,
    curr: &mut RerunTestSuiteInfo,
) {
    match skipped {
        TestOutcomeSkipped::Rerun => {
            // This test was skipped due to having passed in a prior run in this
            // rerun chain. Add it to passing.
            //
            // Note that if a test goes from passing to not being present in the
            // list at all, and then back to being present, it becomes
            // outstanding. This is deliberate.
            curr.passing.insert(test_name.clone());
        }
        TestOutcomeSkipped::Explicit => {
            // If a test is explicitly skipped, the behavior depends on whether
            // this is the rerun of an initial run or part of a rerun chain.
            //
            // If this is a rerun of an initial run, then it doesn't make sense
            // to add the test to the outstanding list, because the user
            // explicitly skipped it.
            //
            // If this is a rerun chain, then whether it is still outstanding
            // depends on whether it was originally outstanding. If it was
            // originally outstanding, then that should be carried forward. If
            // it was originally passing, we should assume that that hasn't
            // changed and it is still passing. If neither, then it's not part
            // of the set of tests we care about.
            if let Some(prev) = prev {
                if prev.outstanding.contains(test_name) {
                    curr.outstanding.insert(test_name.clone());
                } else if prev.passing.contains(test_name) {
                    curr.passing.insert(test_name.clone());
                }
            } else {
                // This is either not a rerun chain, or it is a rerun chain and
                // this binary has never been seen before.
            }
        }
    }
}

/// Reason why a test was skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestOutcomeSkipped {
    /// Test was explicitly skipped by the user.
    Explicit,

    /// Test was skipped due to this being a rerun.
    Rerun,
}

impl TestOutcomeSkipped {
    /// Computes the skipped reason from a `MismatchReason`.
    fn from_mismatch_reason(reason: MismatchReason) -> Self {
        match reason {
            MismatchReason::NotBenchmark
            | MismatchReason::Ignored
            | MismatchReason::String
            | MismatchReason::Expression
            | MismatchReason::Partition
            | MismatchReason::DefaultFilter => TestOutcomeSkipped::Explicit,
            MismatchReason::RerunAlreadyPassed => TestOutcomeSkipped::Rerun,
            other => unreachable!("all known match arms are covered, found {other:?}"),
        }
    }
}

/// Outcome of a single test from a run's event log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestOutcome {
    /// Test passed (had a successful `TestFinished` event).
    Passed,

    /// Test was skipped.
    Skipped(TestOutcomeSkipped),

    /// Test failed (had a `TestFinished` event but did not pass).
    Failed,
}

/// Outcomes extracted from a run's event log.
///
/// This is used for computing outstanding and passing tests.
#[derive(Clone, Debug)]
struct TestEventOutcomes {
    /// Map from test instance to its outcome.
    outcomes: HashMap<OwnedTestInstanceId, TestOutcome>,
}

impl TestEventOutcomes {
    /// Collects test outcomes from the event log.
    ///
    /// Returns information about which tests passed and which tests were seen
    /// (had any event: started, finished, or skipped).
    fn collect(reader: &mut RecordReader) -> Result<Self, RecordReadError> {
        reader.load_dictionaries()?;

        let events: Vec<_> = reader.events()?.collect::<Result<Vec<_>, _>>()?;
        let outcomes = collect_from_events(events.iter().map(|e| &e.kind));

        Ok(Self { outcomes })
    }
}

/// Collects test outcomes from an iterator of events.
///
/// This helper exists to make the event processing logic testable without
/// requiring a full `RecordReader`.
fn collect_from_events<'a, O>(
    events: impl Iterator<Item = &'a TestEventKindSummary<O>>,
) -> HashMap<OwnedTestInstanceId, TestOutcome>
where
    O: 'a,
{
    let mut outcomes = HashMap::new();

    for kind in events {
        match kind {
            TestEventKindSummary::Output(OutputEventKind::TestFinished {
                test_instance,
                run_statuses,
                ..
            }) => {
                // Determine outcome for this iteration/finish event.
                let outcome = if run_statuses.last_status().result.is_success() {
                    TestOutcome::Passed
                } else {
                    TestOutcome::Failed
                };

                // For stress runs: multiple TestFinished events occur for the
                // same test_instance (one per stress iteration). The overall
                // outcome is Failed if any iteration failed.
                //
                // We use entry() to only "upgrade" from Passed to Failed, never
                // downgrade. This ensures [Pass, Fail, Pass] → Failed.
                outcomes
                    .entry(test_instance.clone())
                    .and_modify(|existing| {
                        if outcome == TestOutcome::Failed {
                            *existing = TestOutcome::Failed;
                        }
                    })
                    .or_insert(outcome);
            }
            TestEventKindSummary::Core(CoreEventKind::TestSkipped {
                test_instance,
                reason,
                ..
            }) => {
                let skipped_reason = TestOutcomeSkipped::from_mismatch_reason(*reason);
                outcomes.insert(test_instance.clone(), TestOutcome::Skipped(skipped_reason));
            }
            _ => {}
        }
    }

    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record::{OutputEventKind, StressIndexSummary, TestEventKindSummary},
        reporter::{
            TestOutputDisplay,
            events::{
                ChildExecutionOutputDescription, ChildOutputDescription, ExecuteStatus,
                ExecutionResultDescription, ExecutionStatuses, FailureDescription, RetryData,
                RunStats,
            },
        },
    };
    use chrono::Utc;
    use proptest::prelude::*;
    use std::{
        collections::{BTreeMap, btree_map},
        num::NonZero,
        sync::OnceLock,
        time::Duration,
    };
    use test_strategy::proptest;

    // ---
    // Tests
    // ---

    /// Main property: the SUT matches the oracle.
    #[proptest(cases = 200)]
    fn sut_matches_oracle(#[strategy(arb_rerun_model())] model: RerunModel) {
        let expected = model.compute_rerun_info_decision_table();
        let actual = run_sut(&model);
        prop_assert_eq!(actual, expected);
    }

    /// Property: passing and outstanding are always disjoint.
    #[proptest(cases = 200)]
    fn passing_and_outstanding_disjoint(#[strategy(arb_rerun_model())] model: RerunModel) {
        let result = run_sut(&model);
        for suite in result.iter() {
            let intersection: BTreeSet<_> =
                suite.passing.intersection(&suite.outstanding).collect();
            prop_assert!(
                intersection.is_empty(),
                "passing and outstanding should be disjoint for {}: {:?}",
                suite.binary_id,
                intersection
            );
        }
    }

    /// Property: every matching test with a definitive outcome ends up in either
    /// passing or outstanding.
    ///
    /// Tests that are explicitly skipped (with no prior tracking history) are
    /// not tracked, so they may not be in either set.
    #[proptest(cases = 200)]
    fn matching_tests_with_outcomes_are_tracked(#[strategy(arb_rerun_model())] model: RerunModel) {
        let result = run_sut(&model);

        // Check final state against final test list.
        let final_step = model.reruns.last().unwrap_or(&model.initial);

        for (binary_id, binary_model) in &final_step.test_list.binaries {
            if let BinaryModel::Listed { tests } = binary_model {
                let rust_binary_id = binary_id.rust_binary_id();

                for (test_name, filter_match) in tests {
                    if matches!(filter_match, FilterMatch::Matches) {
                        let key = (*binary_id, *test_name);
                        let outcome = final_step.outcomes.get(&key);

                        // Tests with Passed/Failed/Skipped(Rerun) or no outcome
                        // (not seen) should be tracked. Tests with
                        // Skipped(Explicit) might not be tracked if there's no
                        // prior history.
                        let should_be_tracked = match outcome {
                            Some(TestOutcome::Passed)
                            | Some(TestOutcome::Failed)
                            | Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun))
                            | None => true,
                            Some(TestOutcome::Skipped(TestOutcomeSkipped::Explicit)) => false,
                        };

                        if should_be_tracked {
                            let tcn = test_name.test_case_name();
                            let suite = result.get(&rust_binary_id);
                            let in_passing = suite.is_some_and(|s| s.passing.contains(tcn));
                            let in_outstanding = suite.is_some_and(|s| s.outstanding.contains(tcn));
                            prop_assert!(
                                in_passing || in_outstanding,
                                "matching test {:?}::{:?} with outcome {:?} should be in passing or outstanding",
                                binary_id,
                                test_name,
                                outcome
                            );
                        }
                    }
                }
            }
        }
    }

    /// Test the decision table function directly with all combinations.
    #[test]
    fn decide_test_outcome_truth_table() {
        use Decision as D;
        use FilterMatchResult as F;
        use PrevStatus as P;

        // Binary not present: carry forward previous status.
        assert_eq!(
            decide_test_outcome(P::Passing, F::BinaryNotPresent, None),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, F::BinaryNotPresent, None),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Unknown, F::BinaryNotPresent, None),
            D::NotTracked
        );

        // Binary skipped: carry forward previous status.
        assert_eq!(
            decide_test_outcome(P::Passing, F::BinarySkipped, None),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, F::BinarySkipped, None),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Unknown, F::BinarySkipped, None),
            D::NotTracked
        );

        // Test not in list: only carry forward outstanding.
        assert_eq!(
            decide_test_outcome(P::Passing, F::TestNotInList, None),
            D::NotTracked
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, F::TestNotInList, None),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Unknown, F::TestNotInList, None),
            D::NotTracked
        );

        // FilterMatch::Matches with various outcomes.
        let matches = F::HasMatch(FilterMatch::Matches);

        // Passed -> Passing.
        assert_eq!(
            decide_test_outcome(P::Unknown, matches, Some(TestOutcome::Passed)),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Passing, matches, Some(TestOutcome::Passed)),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, matches, Some(TestOutcome::Passed)),
            D::Passing
        );

        // Failed -> Outstanding.
        assert_eq!(
            decide_test_outcome(P::Unknown, matches, Some(TestOutcome::Failed)),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Passing, matches, Some(TestOutcome::Failed)),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, matches, Some(TestOutcome::Failed)),
            D::Outstanding
        );

        // Not seen (None outcome) -> Outstanding.
        assert_eq!(
            decide_test_outcome(P::Unknown, matches, None),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Passing, matches, None),
            D::Outstanding
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, matches, None),
            D::Outstanding
        );

        // Skipped(Rerun) -> Passing.
        let rerun_skipped = Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun));
        assert_eq!(
            decide_test_outcome(P::Unknown, matches, rerun_skipped),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Passing, matches, rerun_skipped),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, matches, rerun_skipped),
            D::Passing
        );

        // Skipped(Explicit) -> carry forward.
        let explicit_skipped = Some(TestOutcome::Skipped(TestOutcomeSkipped::Explicit));
        assert_eq!(
            decide_test_outcome(P::Unknown, matches, explicit_skipped),
            D::NotTracked
        );
        assert_eq!(
            decide_test_outcome(P::Passing, matches, explicit_skipped),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, matches, explicit_skipped),
            D::Outstanding
        );

        // FilterMatch::Mismatch with RerunAlreadyPassed -> Passing.
        let rerun_mismatch = F::HasMatch(FilterMatch::Mismatch {
            reason: MismatchReason::RerunAlreadyPassed,
        });
        assert_eq!(
            decide_test_outcome(P::Unknown, rerun_mismatch, None),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Passing, rerun_mismatch, None),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, rerun_mismatch, None),
            D::Passing
        );

        // FilterMatch::Mismatch with other reasons -> carry forward.
        let explicit_mismatch = F::HasMatch(FilterMatch::Mismatch {
            reason: MismatchReason::Ignored,
        });
        assert_eq!(
            decide_test_outcome(P::Unknown, explicit_mismatch, None),
            D::NotTracked
        );
        assert_eq!(
            decide_test_outcome(P::Passing, explicit_mismatch, None),
            D::Passing
        );
        assert_eq!(
            decide_test_outcome(P::Outstanding, explicit_mismatch, None),
            D::Outstanding
        );
    }

    // ---
    // Spec property verification
    // ---
    //
    // These tests verify properties of the decision table itself (not the
    // implementation). Since the (sub)domain is finite, we enumerate all cases.

    /// All possible previous states.
    const ALL_PREV_STATUSES: [PrevStatus; 3] = [
        PrevStatus::Passing,
        PrevStatus::Outstanding,
        PrevStatus::Unknown,
    ];

    /// All possible outcomes (including None = not seen).
    fn all_outcomes() -> [Option<TestOutcome>; 5] {
        [
            None,
            Some(TestOutcome::Passed),
            Some(TestOutcome::Failed),
            Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun)),
            Some(TestOutcome::Skipped(TestOutcomeSkipped::Explicit)),
        ]
    }

    /// All HasMatch filter results (test is in the list).
    fn all_in_list_filter_results() -> Vec<FilterMatchResult> {
        let mut results = vec![FilterMatchResult::HasMatch(FilterMatch::Matches)];
        for &reason in MismatchReason::ALL_VARIANTS {
            results.push(FilterMatchResult::HasMatch(FilterMatch::Mismatch {
                reason,
            }));
        }
        results
    }

    /// Spec property: Passing tests stay Passing under non-regressing conditions.
    ///
    /// A test that was Passing remains Passing if:
    /// - It's still in the test list (any HasMatch variant)
    /// - Its outcome is non-regressing (Passed, Skipped(Rerun), or Skipped(Explicit))
    ///
    /// Verified exhaustively: 8 filter variants × 3 outcomes = 24 cases.
    #[test]
    fn spec_property_passing_monotonicity() {
        let non_regressing_outcomes = [
            Some(TestOutcome::Passed),
            Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun)),
            Some(TestOutcome::Skipped(TestOutcomeSkipped::Explicit)),
        ];

        for filter in all_in_list_filter_results() {
            for outcome in non_regressing_outcomes {
                let decision = decide_test_outcome(PrevStatus::Passing, filter, outcome);
                assert_eq!(
                    decision,
                    Decision::Passing,
                    "monotonicity violated: Passing + {:?} + {:?} -> {:?}",
                    filter,
                    outcome,
                    decision
                );
            }
        }
    }

    /// Spec property: Outstanding tests become Passing when they pass.
    ///
    /// This is the convergence property: the only way out of Outstanding is to
    /// pass.
    #[test]
    fn spec_property_outstanding_to_passing_on_pass() {
        let passing_outcomes = [
            Some(TestOutcome::Passed),
            Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun)),
        ];

        for outcome in passing_outcomes {
            let decision = decide_test_outcome(
                PrevStatus::Outstanding,
                FilterMatchResult::HasMatch(FilterMatch::Matches),
                outcome,
            );
            assert_eq!(
                decision,
                Decision::Passing,
                "convergence violated: Outstanding + Matches + {:?} -> {:?}",
                outcome,
                decision
            );
        }
    }

    /// Spec property: Failed or not-seen tests become Outstanding.
    ///
    /// If a test matches the filter but fails or isn't seen, it's outstanding.
    #[test]
    fn spec_property_failed_becomes_outstanding() {
        let failing_outcomes = [None, Some(TestOutcome::Failed)];

        for prev in ALL_PREV_STATUSES {
            for outcome in failing_outcomes {
                let decision = decide_test_outcome(
                    prev,
                    FilterMatchResult::HasMatch(FilterMatch::Matches),
                    outcome,
                );
                assert_eq!(
                    decision,
                    Decision::Outstanding,
                    "FAILED->OUTSTANDING VIOLATED: {:?} + Matches + {:?} -> {:?}",
                    prev,
                    outcome,
                    decision
                );
            }
        }
    }

    /// Spec property: Carry-forward preserves Outstanding but drops Passing for
    /// tests not in the list.
    ///
    /// When a listed binary no longer contains a test (TestNotInList),
    /// Outstanding is preserved but Passing is dropped (becomes NotTracked).
    /// This ensures tests that disappear and reappear are re-run.
    #[test]
    fn spec_property_test_not_in_list_behavior() {
        for outcome in all_outcomes() {
            // Outstanding is preserved.
            assert_eq!(
                decide_test_outcome(
                    PrevStatus::Outstanding,
                    FilterMatchResult::TestNotInList,
                    outcome
                ),
                Decision::Outstanding,
            );
            // Passing is dropped.
            assert_eq!(
                decide_test_outcome(
                    PrevStatus::Passing,
                    FilterMatchResult::TestNotInList,
                    outcome
                ),
                Decision::NotTracked,
            );
            // Unknown stays untracked.
            assert_eq!(
                decide_test_outcome(
                    PrevStatus::Unknown,
                    FilterMatchResult::TestNotInList,
                    outcome
                ),
                Decision::NotTracked,
            );
        }
    }

    // ---
    // Model types
    // ---

    /// A fixed universe of binary IDs for testing.
    ///
    /// Using a small, fixed set ensures meaningful interactions between reruns.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    enum ModelBinaryId {
        A,
        B,
        C,
        D,
    }

    impl ModelBinaryId {
        fn rust_binary_id(self) -> &'static RustBinaryId {
            match self {
                Self::A => {
                    static ID: OnceLock<RustBinaryId> = OnceLock::new();
                    ID.get_or_init(|| RustBinaryId::new("binary-a"))
                }
                Self::B => {
                    static ID: OnceLock<RustBinaryId> = OnceLock::new();
                    ID.get_or_init(|| RustBinaryId::new("binary-b"))
                }
                Self::C => {
                    static ID: OnceLock<RustBinaryId> = OnceLock::new();
                    ID.get_or_init(|| RustBinaryId::new("binary-c"))
                }
                Self::D => {
                    static ID: OnceLock<RustBinaryId> = OnceLock::new();
                    ID.get_or_init(|| RustBinaryId::new("binary-d"))
                }
            }
        }
    }

    /// A fixed universe of test names for testing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    enum ModelTestName {
        Test1,
        Test2,
        Test3,
        Test4,
        Test5,
    }

    impl ModelTestName {
        fn test_case_name(self) -> &'static TestCaseName {
            match self {
                Self::Test1 => {
                    static NAME: OnceLock<TestCaseName> = OnceLock::new();
                    NAME.get_or_init(|| TestCaseName::new("test_1"))
                }
                Self::Test2 => {
                    static NAME: OnceLock<TestCaseName> = OnceLock::new();
                    NAME.get_or_init(|| TestCaseName::new("test_2"))
                }
                Self::Test3 => {
                    static NAME: OnceLock<TestCaseName> = OnceLock::new();
                    NAME.get_or_init(|| TestCaseName::new("test_3"))
                }
                Self::Test4 => {
                    static NAME: OnceLock<TestCaseName> = OnceLock::new();
                    NAME.get_or_init(|| TestCaseName::new("test_4"))
                }
                Self::Test5 => {
                    static NAME: OnceLock<TestCaseName> = OnceLock::new();
                    NAME.get_or_init(|| TestCaseName::new("test_5"))
                }
            }
        }
    }

    /// Model of a binary's state.
    #[derive(Clone, Debug)]
    enum BinaryModel {
        /// Binary was listed; contains test cases with their filter match.
        Listed {
            tests: BTreeMap<ModelTestName, FilterMatch>,
        },
        /// Binary was skipped, so it cannot have tests.
        Skipped,
    }

    /// Test list state for one run.
    #[derive(Clone, Debug)]
    struct TestListModel {
        binaries: BTreeMap<ModelBinaryId, BinaryModel>,
    }

    /// A single run (initial or rerun).
    #[derive(Clone, Debug)]
    struct RunStep {
        /// The test list state for this run.
        test_list: TestListModel,
        /// Outcomes for tests that ran.
        outcomes: BTreeMap<(ModelBinaryId, ModelTestName), TestOutcome>,
    }

    /// The complete model: initial run + subsequent reruns.
    #[derive(Clone, Debug)]
    struct RerunModel {
        /// The initial run.
        initial: RunStep,
        /// The sequence of reruns.
        reruns: Vec<RunStep>,
    }

    impl TestListInfo for TestListModel {
        type BinaryIter<'a> = TestListModelBinaryIter<'a>;

        fn binaries(&self) -> Self::BinaryIter<'_> {
            TestListModelBinaryIter {
                inner: self.binaries.iter(),
            }
        }
    }

    /// Iterator over binaries in a [`TestListModel`].
    struct TestListModelBinaryIter<'a> {
        inner: btree_map::Iter<'a, ModelBinaryId, BinaryModel>,
    }

    impl<'a> Iterator for TestListModelBinaryIter<'a> {
        type Item = (&'a RustBinaryId, BinaryInfo<'a>);

        fn next(&mut self) -> Option<Self::Item> {
            self.inner.next().map(|(model_id, binary_model)| {
                let rust_id = model_id.rust_binary_id();
                let info = match binary_model {
                    BinaryModel::Listed { tests } => BinaryInfo::Listed {
                        test_cases: Box::new(
                            tests.iter().map(|(name, fm)| (name.test_case_name(), *fm)),
                        ),
                    },
                    BinaryModel::Skipped => BinaryInfo::Skipped,
                };
                (rust_id, info)
            })
        }
    }

    // ---
    // Generators
    // ---

    fn arb_model_binary_id() -> impl Strategy<Value = ModelBinaryId> {
        prop_oneof![
            Just(ModelBinaryId::A),
            Just(ModelBinaryId::B),
            Just(ModelBinaryId::C),
            Just(ModelBinaryId::D),
        ]
    }

    fn arb_model_test_name() -> impl Strategy<Value = ModelTestName> {
        prop_oneof![
            Just(ModelTestName::Test1),
            Just(ModelTestName::Test2),
            Just(ModelTestName::Test3),
            Just(ModelTestName::Test4),
            Just(ModelTestName::Test5),
        ]
    }

    fn arb_filter_match() -> impl Strategy<Value = FilterMatch> {
        prop_oneof![
            4 => Just(FilterMatch::Matches),
            1 => any::<MismatchReason>().prop_map(|reason| FilterMatch::Mismatch { reason }),
        ]
    }

    fn arb_test_outcome() -> impl Strategy<Value = TestOutcome> {
        prop_oneof![
            4 => Just(TestOutcome::Passed),
            2 => Just(TestOutcome::Failed),
            1 => Just(TestOutcome::Skipped(TestOutcomeSkipped::Explicit)),
            1 => Just(TestOutcome::Skipped(TestOutcomeSkipped::Rerun)),
        ]
    }

    fn arb_test_map() -> impl Strategy<Value = BTreeMap<ModelTestName, FilterMatch>> {
        proptest::collection::btree_map(arb_model_test_name(), arb_filter_match(), 0..5)
    }

    fn arb_binary_model() -> impl Strategy<Value = BinaryModel> {
        prop_oneof![
            8 => arb_test_map().prop_map(|tests| BinaryModel::Listed { tests }),
            2 => Just(BinaryModel::Skipped),
        ]
    }

    fn arb_test_list_model() -> impl Strategy<Value = TestListModel> {
        proptest::collection::btree_map(arb_model_binary_id(), arb_binary_model(), 0..4)
            .prop_map(|binaries| TestListModel { binaries })
    }

    /// Generate outcomes consistent with a test list.
    ///
    /// Only generates outcomes for tests that match the filter in listed binaries.
    /// Takes a list of matching tests to generate outcomes for.
    fn arb_outcomes_for_matching_tests(
        matching_tests: Vec<(ModelBinaryId, ModelTestName)>,
    ) -> BoxedStrategy<BTreeMap<(ModelBinaryId, ModelTestName), TestOutcome>> {
        if matching_tests.is_empty() {
            Just(BTreeMap::new()).boxed()
        } else {
            let len = matching_tests.len();
            proptest::collection::btree_map(
                proptest::sample::select(matching_tests),
                arb_test_outcome(),
                0..=len,
            )
            .boxed()
        }
    }

    /// Extract matching tests from a test list model.
    fn extract_matching_tests(test_list: &TestListModel) -> Vec<(ModelBinaryId, ModelTestName)> {
        test_list
            .binaries
            .iter()
            .filter_map(|(binary_id, model)| match model {
                BinaryModel::Listed { tests } => Some(
                    tests
                        .iter()
                        .filter(|(_, fm)| matches!(fm, FilterMatch::Matches))
                        .map(move |(tn, _)| (*binary_id, *tn)),
                ),
                BinaryModel::Skipped => None,
            })
            .flatten()
            .collect()
    }

    fn arb_run_step() -> impl Strategy<Value = RunStep> {
        arb_test_list_model().prop_flat_map(|test_list| {
            let matching_tests = extract_matching_tests(&test_list);
            arb_outcomes_for_matching_tests(matching_tests).prop_map(move |outcomes| RunStep {
                test_list: test_list.clone(),
                outcomes,
            })
        })
    }

    fn arb_rerun_model() -> impl Strategy<Value = RerunModel> {
        (
            arb_run_step(),
            proptest::collection::vec(arb_run_step(), 0..5),
        )
            .prop_map(|(initial, reruns)| RerunModel { initial, reruns })
    }

    // ---
    // Helper to convert model outcomes to HashMap<OwnedTestInstanceId, TestOutcome>
    // ---

    fn model_outcomes_to_hashmap(
        outcomes: &BTreeMap<(ModelBinaryId, ModelTestName), TestOutcome>,
    ) -> HashMap<OwnedTestInstanceId, TestOutcome> {
        outcomes
            .iter()
            .map(|((binary_id, test_name), outcome)| {
                let id = OwnedTestInstanceId {
                    binary_id: binary_id.rust_binary_id().clone(),
                    test_name: test_name.test_case_name().clone(),
                };
                (id, *outcome)
            })
            .collect()
    }

    // ---
    // Helpers
    // ---

    /// Runs the SUT through an entire `RerunModel`.
    fn run_sut(model: &RerunModel) -> IdOrdMap<RerunTestSuiteInfo> {
        let outcomes = model_outcomes_to_hashmap(&model.initial.outcomes);
        let mut result = compute_outstanding_pure(None, &model.initial.test_list, &outcomes);

        for rerun in &model.reruns {
            let outcomes = model_outcomes_to_hashmap(&rerun.outcomes);
            result = compute_outstanding_pure(Some(&result), &rerun.test_list, &outcomes);
        }

        result
    }

    // ---
    // Oracle: per-test decision table
    // ---
    //
    // The oracle determines each test's fate independently using a decision
    // table (`decide_test_outcome`). This is verifiable by inspection and
    // structurally different from the SUT.

    /// Status of a test in the previous run.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum PrevStatus {
        /// Test was in the passing set.
        Passing,
        /// Test was in the outstanding set.
        Outstanding,
        /// Test was not tracked (not in either set).
        Unknown,
    }

    /// What to do with this test after applying the decision table.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Decision {
        /// Add to the passing set.
        Passing,
        /// Add to the outstanding set.
        Outstanding,
        /// Don't track this test.
        NotTracked,
    }

    /// Result of looking up a test's filter match in the current step.
    ///
    /// This distinguishes between different reasons a filter match might not
    /// exist, which affects how the test's state is handled.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FilterMatchResult {
        /// Binary is not in the test list at all. Carry forward the entire
        /// suite.
        BinaryNotPresent,
        /// Binary is in the test list but skipped. Carry forward the entire
        /// suite.
        BinarySkipped,
        /// Binary is listed but this test is not in its test map. Only carry
        /// forward outstanding tests; passing tests become untracked.
        TestNotInList,
        /// Test has a filter match.
        HasMatch(FilterMatch),
    }

    /// Pure decision table for a single test.
    ///
    /// This is the core logic expressed as a truth table, making it easy to verify
    /// by inspection that each case is handled correctly.
    fn decide_test_outcome(
        prev: PrevStatus,
        filter_result: FilterMatchResult,
        outcome: Option<TestOutcome>,
    ) -> Decision {
        match filter_result {
            FilterMatchResult::BinaryNotPresent | FilterMatchResult::BinarySkipped => {
                // Binary not present or skipped: carry forward previous status.
                match prev {
                    PrevStatus::Passing => Decision::Passing,
                    PrevStatus::Outstanding => Decision::Outstanding,
                    PrevStatus::Unknown => Decision::NotTracked,
                }
            }
            FilterMatchResult::TestNotInList => {
                // Test is not in the current test list of a listed binary.
                // Only carry forward outstanding tests. Passing tests that
                // disappear from the list become untracked (and will be re-run
                // if they reappear).
                match prev {
                    PrevStatus::Outstanding => Decision::Outstanding,
                    PrevStatus::Passing | PrevStatus::Unknown => Decision::NotTracked,
                }
            }
            FilterMatchResult::HasMatch(FilterMatch::Matches) => {
                match outcome {
                    Some(TestOutcome::Passed) => Decision::Passing,
                    Some(TestOutcome::Failed) => Decision::Outstanding,
                    None => {
                        // Test was scheduled but not seen in event log: outstanding.
                        Decision::Outstanding
                    }
                    Some(TestOutcome::Skipped(TestOutcomeSkipped::Rerun)) => Decision::Passing,
                    Some(TestOutcome::Skipped(TestOutcomeSkipped::Explicit)) => {
                        // Carry forward, or not tracked if unknown.
                        match prev {
                            PrevStatus::Passing => Decision::Passing,
                            PrevStatus::Outstanding => Decision::Outstanding,
                            PrevStatus::Unknown => Decision::NotTracked,
                        }
                    }
                }
            }
            FilterMatchResult::HasMatch(FilterMatch::Mismatch { reason }) => {
                match TestOutcomeSkipped::from_mismatch_reason(reason) {
                    TestOutcomeSkipped::Rerun => Decision::Passing,
                    TestOutcomeSkipped::Explicit => {
                        // Carry forward, or not tracked if unknown.
                        match prev {
                            PrevStatus::Passing => Decision::Passing,
                            PrevStatus::Outstanding => Decision::Outstanding,
                            PrevStatus::Unknown => Decision::NotTracked,
                        }
                    }
                }
            }
        }
    }

    impl RerunModel {
        /// Per-test decision table oracle.
        ///
        /// This is structurally different from the main oracle: instead of iterating
        /// through binaries and updating state imperatively, it determines each
        /// test's fate independently using a truth table.
        fn compute_rerun_info_decision_table(&self) -> IdOrdMap<RerunTestSuiteInfo> {
            // Compute all previous states by running through the chain.
            let mut prev_state: HashMap<(ModelBinaryId, ModelTestName), PrevStatus> =
                HashMap::new();

            // Process initial run.
            self.update_state_from_step(&mut prev_state, &self.initial);

            // Process reruns.
            for rerun in &self.reruns {
                self.update_state_from_step(&mut prev_state, rerun);
            }

            // Convert final state to result.
            self.collect_final_state(&prev_state)
        }

        fn update_state_from_step(
            &self,
            state: &mut HashMap<(ModelBinaryId, ModelTestName), PrevStatus>,
            step: &RunStep,
        ) {
            // Enumerate all tests we need to consider:
            // - Tests in the current test list
            // - Tests from previous state (for carry-forward)
            let all_tests = self.enumerate_all_tests(state, step);

            for (binary_id, test_name) in all_tests {
                let prev = state
                    .get(&(binary_id, test_name))
                    .copied()
                    .unwrap_or(PrevStatus::Unknown);

                let filter_result = self.get_filter_match_result(step, binary_id, test_name);
                let outcome = step.outcomes.get(&(binary_id, test_name)).copied();

                let decision = decide_test_outcome(prev, filter_result, outcome);

                // Update state based on decision.
                match decision {
                    Decision::Passing => {
                        state.insert((binary_id, test_name), PrevStatus::Passing);
                    }
                    Decision::Outstanding => {
                        state.insert((binary_id, test_name), PrevStatus::Outstanding);
                    }
                    Decision::NotTracked => {
                        state.remove(&(binary_id, test_name));
                    }
                }
            }
        }

        /// Gets the filter match result for a test in a step.
        ///
        /// Returns a `FilterMatchResult` indicating why the filter match is
        /// present or absent.
        fn get_filter_match_result(
            &self,
            step: &RunStep,
            binary_id: ModelBinaryId,
            test_name: ModelTestName,
        ) -> FilterMatchResult {
            match step.test_list.binaries.get(&binary_id) {
                None => FilterMatchResult::BinaryNotPresent,
                Some(BinaryModel::Skipped) => FilterMatchResult::BinarySkipped,
                Some(BinaryModel::Listed { tests }) => match tests.get(&test_name) {
                    Some(filter_match) => FilterMatchResult::HasMatch(*filter_match),
                    None => FilterMatchResult::TestNotInList,
                },
            }
        }

        /// Enumerates all tests that need to be considered for a step.
        ///
        /// This includes tests from the current test list and tests from the
        /// previous state (for carry-forward).
        fn enumerate_all_tests(
            &self,
            prev_state: &HashMap<(ModelBinaryId, ModelTestName), PrevStatus>,
            step: &RunStep,
        ) -> BTreeSet<(ModelBinaryId, ModelTestName)> {
            let mut tests = BTreeSet::new();

            // Tests from current test list.
            for (binary_id, binary_model) in &step.test_list.binaries {
                if let BinaryModel::Listed { tests: test_map } = binary_model {
                    for test_name in test_map.keys() {
                        tests.insert((*binary_id, *test_name));
                    }
                }
            }

            // Tests from previous state (for carry-forward).
            for (binary_id, test_name) in prev_state.keys() {
                tests.insert((*binary_id, *test_name));
            }

            tests
        }

        /// Converts the final state to an `IdOrdMap<TestSuiteOutstanding>`.
        fn collect_final_state(
            &self,
            state: &HashMap<(ModelBinaryId, ModelTestName), PrevStatus>,
        ) -> IdOrdMap<RerunTestSuiteInfo> {
            let mut result: BTreeMap<ModelBinaryId, RerunTestSuiteInfo> = BTreeMap::new();

            for ((binary_id, test_name), status) in state {
                let suite = result
                    .entry(*binary_id)
                    .or_insert_with(|| RerunTestSuiteInfo::new(binary_id.rust_binary_id().clone()));

                match status {
                    PrevStatus::Passing => {
                        suite.passing.insert(test_name.test_case_name().clone());
                    }
                    PrevStatus::Outstanding => {
                        suite.outstanding.insert(test_name.test_case_name().clone());
                    }
                    PrevStatus::Unknown => {
                        // Not tracked: don't add.
                    }
                }
            }

            let mut id_map = IdOrdMap::new();
            for (_, suite) in result {
                id_map.insert_unique(suite).expect("unique binaries");
            }
            id_map
        }
    }

    // ---
    // Stress run accumulation tests.
    // ---

    /// Creates a `TestFinished` event for testing.
    ///
    /// Uses `()` as the output type since we don't need actual output data for
    /// these tests.
    fn make_test_finished(
        test_instance: OwnedTestInstanceId,
        stress_index: Option<(u32, Option<u32>)>,
        passed: bool,
    ) -> TestEventKindSummary<()> {
        let result = if passed {
            ExecutionResultDescription::Pass
        } else {
            ExecutionResultDescription::Fail {
                failure: FailureDescription::ExitCode { code: 1 },
                leaked: false,
            }
        };

        let execute_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: ChildExecutionOutputDescription::Output {
                result: Some(result.clone()),
                output: ChildOutputDescription::Split {
                    stdout: None,
                    stderr: None,
                },
                errors: None,
            },
            result,
            start_time: Utc::now().into(),
            time_taken: Duration::from_millis(100),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        TestEventKindSummary::Output(OutputEventKind::TestFinished {
            stress_index: stress_index.map(|(current, total)| StressIndexSummary {
                current,
                total: total.and_then(NonZero::new),
            }),
            test_instance,
            success_output: TestOutputDisplay::Never,
            failure_output: TestOutputDisplay::Never,
            junit_store_success_output: false,
            junit_store_failure_output: false,
            run_statuses: ExecutionStatuses::new(vec![execute_status]),
            current_stats: RunStats::default(),
            running: 0,
        })
    }

    /// Test stress run accumulation: if any iteration fails, the test is Failed.
    ///
    /// This tests the fix for the stress run accumulation logic. Multiple
    /// `TestFinished` events for the same test (one per stress iteration) should
    /// result in Failed if any iteration failed, regardless of order.
    #[test]
    fn stress_run_accumulation() {
        // [Pass, Fail, Pass] -> Failed.
        let test_pass_fail_pass = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("pass_fail_pass"),
        };

        // [Pass, Pass, Pass] -> Passed.
        let test_all_pass = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("all_pass"),
        };

        // [Fail, Fail, Fail] -> Failed.
        let test_all_fail = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("all_fail"),
        };

        // [Fail, Pass, Pass] -> Failed.
        let test_fail_first = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("fail_first"),
        };

        // Regular (non-stress) pass.
        let test_regular_pass = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("regular_pass"),
        };

        // Regular (non-stress) fail.
        let test_regular_fail = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("regular_fail"),
        };

        // Construct all events in one stream.
        let events = [
            // pass_fail_pass: [Pass, Fail, Pass]
            make_test_finished(test_pass_fail_pass.clone(), Some((0, Some(3))), true),
            make_test_finished(test_pass_fail_pass.clone(), Some((1, Some(3))), false),
            make_test_finished(test_pass_fail_pass.clone(), Some((2, Some(3))), true),
            // all_pass: [Pass, Pass, Pass]
            make_test_finished(test_all_pass.clone(), Some((0, Some(3))), true),
            make_test_finished(test_all_pass.clone(), Some((1, Some(3))), true),
            make_test_finished(test_all_pass.clone(), Some((2, Some(3))), true),
            // all_fail: [Fail, Fail, Fail]
            make_test_finished(test_all_fail.clone(), Some((0, Some(3))), false),
            make_test_finished(test_all_fail.clone(), Some((1, Some(3))), false),
            make_test_finished(test_all_fail.clone(), Some((2, Some(3))), false),
            // fail_first: [Fail, Pass, Pass]
            make_test_finished(test_fail_first.clone(), Some((0, Some(3))), false),
            make_test_finished(test_fail_first.clone(), Some((1, Some(3))), true),
            make_test_finished(test_fail_first.clone(), Some((2, Some(3))), true),
            // regular_pass: single pass (no stress index)
            make_test_finished(test_regular_pass.clone(), None, true),
            // regular_fail: single fail (no stress index)
            make_test_finished(test_regular_fail.clone(), None, false),
        ];

        let outcomes = collect_from_events(events.iter());

        assert_eq!(
            outcomes.get(&test_pass_fail_pass),
            Some(&TestOutcome::Failed),
            "[Pass, Fail, Pass] should be Failed"
        );
        assert_eq!(
            outcomes.get(&test_all_pass),
            Some(&TestOutcome::Passed),
            "[Pass, Pass, Pass] should be Passed"
        );
        assert_eq!(
            outcomes.get(&test_all_fail),
            Some(&TestOutcome::Failed),
            "[Fail, Fail, Fail] should be Failed"
        );
        assert_eq!(
            outcomes.get(&test_fail_first),
            Some(&TestOutcome::Failed),
            "[Fail, Pass, Pass] should be Failed"
        );
        assert_eq!(
            outcomes.get(&test_regular_pass),
            Some(&TestOutcome::Passed),
            "regular pass should be Passed"
        );
        assert_eq!(
            outcomes.get(&test_regular_fail),
            Some(&TestOutcome::Failed),
            "regular fail should be Failed"
        );
    }

    /// Test that multiple tests in a stress run are tracked independently.
    ///
    /// Interleaved events for different tests should not interfere with each
    /// other's outcome accumulation.
    #[test]
    fn stress_run_multiple_tests_independent() {
        let test_a = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("test_a"),
        };
        let test_b = OwnedTestInstanceId {
            binary_id: RustBinaryId::new("test-binary"),
            test_name: TestCaseName::new("test_b"),
        };

        // Interleaved stress run events for two tests:
        // test_a: [Pass, Pass] -> Passed
        // test_b: [Pass, Fail] -> Failed
        let events = [
            make_test_finished(test_a.clone(), Some((0, Some(2))), true),
            make_test_finished(test_b.clone(), Some((0, Some(2))), true),
            make_test_finished(test_a.clone(), Some((1, Some(2))), true),
            make_test_finished(test_b.clone(), Some((1, Some(2))), false),
        ];

        let outcomes = collect_from_events(events.iter());

        assert_eq!(
            outcomes.get(&test_a),
            Some(&TestOutcome::Passed),
            "test_a [Pass, Pass] should be Passed"
        );
        assert_eq!(
            outcomes.get(&test_b),
            Some(&TestOutcome::Failed),
            "test_b [Pass, Fail] should be Failed"
        );
    }
}
