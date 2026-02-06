// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Partition-related tests for `TestList`.

use super::{
    test_helpers::{collect_matching_tests, make_test_artifact, simple_build_meta, simple_ecx},
    *,
};
use crate::{
    cargo_config::EnvironmentMap,
    partition::PartitionerBuilder,
    record::{ComputedRerunInfo, RerunTestSuiteInfo},
    run_mode::NextestRunMode,
    test_filter::{FilterBound, RunIgnored, TestFilter, TestFilterPatterns},
};
use camino::Utf8PathBuf;
use nextest_metadata::{FilterMatch, MismatchReason, RustBinaryId, TestCaseName};
use proptest::prelude::*;
use std::collections::BTreeSet;
use test_strategy::proptest;

/// Input for partition property-based tests.
///
/// The `binary_specs` field is a `Vec` of `(non_ignored, ignored)` pairs,
/// one per binary. Using a single `Vec` of pairs (rather than two
/// parallel `Vec`s) ensures the lengths are always consistent.
#[derive(Debug)]
struct PartitionTestInput {
    /// Per-binary test counts: `(non_ignored, ignored)`.
    binary_specs: Vec<(usize, usize)>,

    /// Total number of shards (>= 1).
    total_shards: u64,

    /// Selected shard (1-based, <= `total_shards`).
    shard: u64,
}

impl Arbitrary for PartitionTestInput {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
        (
            prop::collection::vec((0..=8usize, 0..=4usize), 1..=5),
            1..=6u64,
            any::<proptest::sample::Index>(),
        )
            .prop_map(|(binary_specs, total_shards, shard_idx)| {
                let shard = shard_idx.index(total_shards as usize) as u64 + 1;
                PartitionTestInput {
                    binary_specs,
                    total_shards,
                    shard,
                }
            })
            .boxed()
    }
}

impl PartitionTestInput {
    /// Returns specs with only non-ignored tests (ignored counts zeroed).
    fn non_ignored_only_specs(&self) -> Vec<(usize, usize)> {
        self.binary_specs.iter().map(|&(n, _)| (n, 0)).collect()
    }

    /// Total number of non-ignored tests across all binaries.
    fn total_non_ignored(&self) -> usize {
        self.binary_specs.iter().map(|(n, _)| n).sum()
    }
}

/// Standard 4-test output used by most partition tests.
const FOUR_TEST_OUTPUT: &str = indoc::indoc! {"
    alpha: test
    beta: test
    gamma: test
    delta: test
"};

/// Creates a default filter: non-ignored tests, no name patterns, no filtersets.
fn default_filter() -> TestFilter {
    TestFilter::new(
        NextestRunMode::Test,
        RunIgnored::Default,
        TestFilterPatterns::default(),
        Vec::new(),
    )
    .unwrap()
}

/// Creates the standard count:1/2 partitioner.
fn count_1_of_2() -> PartitionerBuilder {
    PartitionerBuilder::Count {
        shard: 1,
        total_shards: 2,
    }
}

/// Builds a `TestList` from `(binary_id, non_ignored_output, ignored_output)`
/// tuples with standard defaults for workspace root, build meta, environment,
/// and eval context.
fn build_test_list(
    outputs: impl IntoIterator<Item = (&'static str, &'static str, &'static str)>,
    filter: &TestFilter,
    partitioner: &PartitionerBuilder,
) -> TestList<'static> {
    let ecx = simple_ecx();
    let artifacts: Vec<_> = outputs
        .into_iter()
        .map(|(id, non_ign, ign)| (make_test_artifact(id), non_ign, ign))
        .collect();
    TestList::new_with_outputs(
        artifacts,
        Utf8PathBuf::from("/fake/path"),
        simple_build_meta(),
        filter,
        Some(partitioner),
        EnvironmentMap::empty(),
        &ecx,
        FilterBound::All,
    )
    .expect("valid output")
}

/// Verifies that per-binary partitioning produces the expected results:
/// count:1/2 selects every other test, independently within each binary.
#[test]
fn test_apply_per_binary_partitioning_count() {
    let binary2_output = indoc::indoc! {"
        one: test
        two: test
        three: test
    "};

    let filter = default_filter();
    let partitioner = count_1_of_2();
    let test_list = build_test_list(
        [
            ("pkg::binary1", FOUR_TEST_OUTPUT, ""),
            ("pkg::binary2", binary2_output, ""),
        ],
        &filter,
        &partitioner,
    );

    // Binary 1 (sorted: alpha, beta, delta, gamma): shard 1 gets alpha, delta.
    // Binary 2 (sorted: one, three, two): shard 1 gets one, two.
    let binary1_suite = test_list
        .get_suite(&RustBinaryId::new("pkg::binary1"))
        .expect("binary1 should exist");
    assert_eq!(
        collect_matching_tests(binary1_suite),
        vec!["alpha", "delta"]
    );

    let binary2_suite = test_list
        .get_suite(&RustBinaryId::new("pkg::binary2"))
        .expect("binary2 should exist");
    assert_eq!(collect_matching_tests(binary2_suite), vec!["one", "two"]);
}

/// Verifies that non-ignored and ignored tests are partitioned
/// independently: adding or removing an ignored test does not change which
/// non-ignored tests are selected, and vice versa.
#[test]
fn test_partition_ignored_independent() {
    let ignored_output = indoc::indoc! {"
        ig_one: test
        ig_two: test
        ig_three: test
    "};

    let filter = TestFilter::new(
        NextestRunMode::Test,
        RunIgnored::All,
        TestFilterPatterns::default(),
        Vec::new(),
    )
    .unwrap();
    let partitioner = count_1_of_2();
    let test_list = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, ignored_output)],
        &filter,
        &partitioner,
    );

    let suite = test_list
        .get_suite(&RustBinaryId::new("pkg::binary"))
        .expect("binary should exist");

    // Non-ignored (sorted: alpha, beta, delta, gamma): shard 1 picks
    // indices 0, 2 => alpha, delta.
    let non_ignored_matches: Vec<_> = suite
        .status
        .test_cases()
        .filter(|tc| !tc.test_info.ignored && tc.test_info.filter_match.is_match())
        .map(|tc| tc.name.as_str())
        .collect();
    assert_eq!(non_ignored_matches, vec!["alpha", "delta"]);

    // Ignored (sorted: ig_one, ig_three, ig_two): shard 1 picks indices
    // 0, 2 => ig_one, ig_two. Independent of the non-ignored partitioner.
    let ignored_matches: Vec<_> = suite
        .status
        .test_cases()
        .filter(|tc| tc.test_info.ignored && tc.test_info.filter_match.is_match())
        .map(|tc| tc.name.as_str())
        .collect();
    assert_eq!(ignored_matches, vec!["ig_one", "ig_two"]);
}

/// Verifies that `RerunAlreadyPassed` tests participate in partition
/// counting (to maintain stable shard assignment) but their status is
/// preserved as `RerunAlreadyPassed`, not changed to `Partition`.
#[test]
fn test_partition_rerun_already_passed() {
    let binary_id = RustBinaryId::new("pkg::binary");
    let partitioner = count_1_of_2();

    // Mark "beta" as already passed in a prior rerun. "beta" is at sorted
    // index 1. The filter will mark it RerunAlreadyPassed before
    // partitioning runs.
    let rerun_suite = RerunTestSuiteInfo {
        binary_id: binary_id.clone(),
        passing: BTreeSet::from([TestCaseName::new("beta")]),
        outstanding: BTreeSet::from([
            TestCaseName::new("alpha"),
            TestCaseName::new("delta"),
            TestCaseName::new("gamma"),
        ]),
    };
    let mut rerun_filter = default_filter();
    rerun_filter.set_outstanding_tests(ComputedRerunInfo {
        test_suites: iddqd::id_ord_map! { rerun_suite },
    });

    let rerun_list = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, "")],
        &rerun_filter,
        &partitioner,
    );

    let suite = rerun_list
        .get_suite(&binary_id)
        .expect("binary should exist");

    // After filtering, test states are (sorted: alpha, beta, delta, gamma):
    //   alpha (0): Matches -> kept by partitioner
    //   beta  (1): RerunAlreadyPassed -> counted, status preserved
    //   delta (2): Matches -> kept by partitioner
    //   gamma (3): Matches -> excluded by partitioner (Partition mismatch)
    let rerun_results: Vec<_> = suite
        .status
        .test_cases()
        .map(|tc| (tc.name.as_str(), tc.test_info.filter_match))
        .collect();

    assert_eq!(
        rerun_results,
        vec![
            ("alpha", FilterMatch::Matches),
            (
                "beta",
                FilterMatch::Mismatch {
                    reason: MismatchReason::RerunAlreadyPassed,
                }
            ),
            ("delta", FilterMatch::Matches),
            (
                "gamma",
                FilterMatch::Mismatch {
                    reason: MismatchReason::Partition,
                }
            ),
        ]
    );

    // Counterfactual: without rerun info, the partitioner sees all 4 tests
    // as Matches. Shard 1 still picks indices 0 and 2, confirming that the
    // RerunAlreadyPassed test participated in counting and kept shard
    // assignments stable.
    let no_rerun_filter = default_filter();
    let no_rerun_list = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, "")],
        &no_rerun_filter,
        &partitioner,
    );

    let suite = no_rerun_list
        .get_suite(&binary_id)
        .expect("binary should exist");

    let no_rerun_results: Vec<_> = suite
        .status
        .test_cases()
        .map(|tc| (tc.name.as_str(), tc.test_info.filter_match))
        .collect();

    assert_eq!(
        no_rerun_results,
        vec![
            ("alpha", FilterMatch::Matches),
            (
                "beta",
                FilterMatch::Mismatch {
                    reason: MismatchReason::Partition,
                }
            ),
            ("delta", FilterMatch::Matches),
            (
                "gamma",
                FilterMatch::Mismatch {
                    reason: MismatchReason::Partition,
                }
            ),
        ]
    );

    // The key property: the set of Matches tests that actually run is
    // {alpha, delta} in both cases.
    let rerun_running: Vec<_> = rerun_results
        .iter()
        .filter(|(_, fm)| fm.is_match())
        .map(|(name, _)| *name)
        .collect();
    let no_rerun_running: Vec<_> = no_rerun_results
        .iter()
        .filter(|(_, fm)| fm.is_match())
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(rerun_running, no_rerun_running);
}

/// Verifies that tests filtered out by name patterns do not participate
/// in partition counting. If pre-filtered tests were counted, shard
/// assignments would shift when name filters change.
#[test]
fn test_partition_prefiltered_excluded_from_counting() {
    let five_test_output = indoc::indoc! {"
        alpha: test
        beta: test
        gamma: test
        delta: test
        epsilon: test
    "};

    // Filter to only tests containing "a" (alpha, beta, gamma, delta match;
    // epsilon does not).
    let name_filter = TestFilter::new(
        NextestRunMode::Test,
        RunIgnored::Default,
        TestFilterPatterns::new(vec!["a".to_string()]),
        Vec::new(),
    )
    .unwrap();
    let partitioner = count_1_of_2();

    let filtered_list = build_test_list(
        [("pkg::binary", five_test_output, "")],
        &name_filter,
        &partitioner,
    );

    let suite = filtered_list
        .get_suite(&RustBinaryId::new("pkg::binary"))
        .expect("binary should exist");

    // Pre-filter: sorted tests are alpha, beta, delta, epsilon, gamma.
    // After name filter, epsilon is Mismatch(String). The remaining 4
    // matching tests are partitioned by count:1/2 => alpha, delta.
    assert_eq!(collect_matching_tests(suite), vec!["alpha", "delta"]);

    // Verify epsilon was filtered out by name, not by partition.
    let epsilon = suite
        .status
        .get(&TestCaseName::new("epsilon"))
        .expect("epsilon should exist");
    assert_eq!(
        epsilon.test_info.filter_match,
        FilterMatch::Mismatch {
            reason: MismatchReason::String,
        }
    );

    // Without name filters, all 5 tests are Matches, and count:1/2 picks
    // indices 0, 2, 4 => alpha, delta, gamma. This differs from the
    // filtered case, proving epsilon did not participate in counting.
    let no_name_filter = default_filter();
    let unfiltered_list = build_test_list(
        [("pkg::binary", five_test_output, "")],
        &no_name_filter,
        &partitioner,
    );

    let suite = unfiltered_list
        .get_suite(&RustBinaryId::new("pkg::binary"))
        .expect("binary should exist");

    assert_eq!(
        collect_matching_tests(suite),
        vec!["alpha", "delta", "gamma"]
    );
}

/// Verifies that hash-based partitioning works correctly: hash:1/2 and
/// hash:2/2 should partition all tests between them without overlap.
#[test]
fn test_partition_hash() {
    let filter = default_filter();
    let partitioner_shard1 = PartitionerBuilder::Hash {
        shard: 1,
        total_shards: 2,
    };
    let partitioner_shard2 = PartitionerBuilder::Hash {
        shard: 2,
        total_shards: 2,
    };

    let list_shard1 = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, "")],
        &filter,
        &partitioner_shard1,
    );
    let list_shard2 = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, "")],
        &filter,
        &partitioner_shard2,
    );

    let suite1 = list_shard1
        .get_suite(&RustBinaryId::new("pkg::binary"))
        .expect("binary should exist");
    let suite2 = list_shard2
        .get_suite(&RustBinaryId::new("pkg::binary"))
        .expect("binary should exist");

    let shard1_matches = collect_matching_tests(suite1);
    let shard2_matches = collect_matching_tests(suite2);

    // The two shards should be disjoint and together cover all tests.
    let mut all_tests: Vec<&str> = shard1_matches
        .iter()
        .chain(shard2_matches.iter())
        .copied()
        .collect();
    all_tests.sort();
    assert_eq!(all_tests, vec!["alpha", "beta", "delta", "gamma"]);

    // Each shard should have at least one test (with 4 tests and 2 shards,
    // a proper hash function won't put all tests in one shard).
    assert!(
        !shard1_matches.is_empty() && !shard2_matches.is_empty(),
        "both shards should have tests: shard1={shard1_matches:?}, shard2={shard2_matches:?}"
    );

    // The shards must not overlap.
    for test in &shard1_matches {
        assert!(
            !shard2_matches.contains(test),
            "test {test} should not appear in both shards"
        );
    }
}

/// Verifies that `test_count` and `run_count` are correct after
/// partitioning.
#[test]
fn test_partition_test_count_and_run_count() {
    let filter = default_filter();
    let partitioner = count_1_of_2();
    let test_list = build_test_list(
        [("pkg::binary", FOUR_TEST_OUTPUT, "")],
        &filter,
        &partitioner,
    );

    // test_count includes all tests (matching and non-matching).
    assert_eq!(test_list.test_count(), 4);

    // count:1/2 selects 2 out of 4 tests (alpha, delta).
    assert_eq!(test_list.run_count(), 2);

    // Verify the invariant: run_count + skip_count == test_count.
    assert_eq!(
        test_list.run_count() + test_list.skip_counts().skipped_tests,
        test_list.test_count()
    );
}

// --- Slice partitioning tests ---

/// Standard 3-test output used by slice tests for a second binary.
const THREE_TEST_OUTPUT: &str = indoc::indoc! {"
    delta: test
    epsilon: test
    zeta: test
"};

/// Creates a `slice:M/N` partitioner builder.
fn slice_partitioner(shard: u64, total_shards: u64) -> PartitionerBuilder {
    PartitionerBuilder::Slice {
        shard,
        total_shards,
    }
}

/// Collects all matching test names across all binaries in a `TestList`,
/// returned as `(binary_id, test_name)` pairs for unambiguous assertions.
fn collect_all_matching_tests<'a>(test_list: &'a TestList<'a>) -> Vec<(&'a str, &'a str)> {
    test_list
        .iter()
        .flat_map(|suite| {
            let binary_id = suite.binary_id.as_str();
            collect_matching_tests(suite)
                .into_iter()
                .map(move |name| (binary_id, name))
        })
        .collect()
}

/// Collects all matching non-ignored test names across all binaries,
/// returned as `(binary_id, test_name)` pairs.
fn collect_non_ignored_matching_tests<'a>(test_list: &'a TestList<'a>) -> Vec<(&'a str, &'a str)> {
    test_list
        .iter()
        .flat_map(|suite| {
            let binary_id = suite.binary_id.as_str();
            suite
                .status
                .test_cases()
                .filter(|tc| !tc.test_info.ignored && tc.test_info.filter_match.is_match())
                .map(move |tc| (binary_id, tc.name.as_str()))
        })
        .collect()
}

/// Verifies that slice partitioning distributes tests across binaries as a
/// single pool (cross-binary round-robin), unlike count which resets per
/// binary.
#[test]
fn test_slice_cross_binary_distribution() {
    let filter = default_filter();
    let partitioner = slice_partitioner(1, 2);

    // Two binaries: binary1 has 3 tests, binary2 has 4 tests (7 total).
    // With count:1/2, each binary is partitioned independently (counter
    // resets per binary). With slice:1/2, all 7 tests form a single
    // cross-binary pool, so the counter carries across binaries.
    let test_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &filter,
        &partitioner,
    );

    // Global sorted order across binaries (binary1 first, then binary2):
    //   binary1: delta(0), epsilon(1), zeta(2)
    //   binary2: alpha(3), beta(4), delta(5), gamma(6)
    //
    // slice:1/2 picks even indices: 0, 2, 4, 6.
    let binary1_suite = test_list
        .get_suite(&RustBinaryId::new("pkg::binary1"))
        .expect("binary1 should exist");
    assert_eq!(collect_matching_tests(binary1_suite), vec!["delta", "zeta"],);

    let binary2_suite = test_list
        .get_suite(&RustBinaryId::new("pkg::binary2"))
        .expect("binary2 should exist");
    assert_eq!(collect_matching_tests(binary2_suite), vec!["beta", "gamma"],);

    // For comparison, count:1/2 would give different results because each
    // binary's partitioner resets independently.
    let count_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &filter,
        &count_1_of_2(),
    );

    let count_binary1 = count_list
        .get_suite(&RustBinaryId::new("pkg::binary1"))
        .expect("binary1 should exist");
    let count_binary2 = count_list
        .get_suite(&RustBinaryId::new("pkg::binary2"))
        .expect("binary2 should exist");

    // count:1/2 per-binary: binary1 indices 0,2 → delta, zeta;
    // binary2 indices 0,2 → alpha, delta. Total: 4.
    assert_eq!(collect_matching_tests(count_binary1), vec!["delta", "zeta"],);
    assert_eq!(
        collect_matching_tests(count_binary2),
        vec!["alpha", "delta"],
    );

    // The key difference: slice shard 1 gets 4 tests, count shard 1 also
    // gets 4 here, but the *distribution across shards* differs. Verify
    // shard 2 to see the asymmetry with count.
    let count_shard2_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &filter,
        &PartitionerBuilder::Count {
            shard: 2,
            total_shards: 2,
        },
    );
    let slice_shard2_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &filter,
        &slice_partitioner(2, 2),
    );

    // count:2/2 gets 1 from binary1 + 2 from binary2 = 3 tests.
    // slice:2/2 gets 3 tests too (with 7 total, shards are 4/3).
    // But the specific tests differ because of cross-binary numbering.
    let count_shard2_all = collect_all_matching_tests(&count_shard2_list);
    let slice_shard2_all = collect_all_matching_tests(&slice_shard2_list);

    assert_eq!(
        count_shard2_all,
        vec![
            ("pkg::binary1", "epsilon"),
            ("pkg::binary2", "beta"),
            ("pkg::binary2", "gamma"),
        ],
    );
    assert_eq!(
        slice_shard2_all,
        vec![
            ("pkg::binary1", "epsilon"),
            ("pkg::binary2", "alpha"),
            ("pkg::binary2", "delta"),
        ],
    );
}

/// Verifies that `RerunAlreadyPassed` tests participate in the
/// cross-binary counter (to maintain stable shard assignment) but
/// their status is preserved. This is the cross-binary analog of
/// `test_partition_rerun_already_passed`: because `slice` uses a
/// single counter across all binaries, a `RerunAlreadyPassed` test
/// in binary1 affects the counter for binary2.
#[test]
fn test_slice_rerun_already_passed_cross_binary() {
    let binary1_id = RustBinaryId::new("pkg::binary1");
    let binary2_id = RustBinaryId::new("pkg::binary2");
    let partitioner = slice_partitioner(1, 2);

    // Mark binary1's "epsilon" as already passed. Global sorted order:
    //   binary1: delta(0), epsilon(1), zeta(2)
    //   binary2: alpha(3), beta(4), delta(5), gamma(6)
    //
    // epsilon at global index 1 is counted but preserves its status.
    let rerun_suite = RerunTestSuiteInfo {
        binary_id: binary1_id.clone(),
        passing: BTreeSet::from([TestCaseName::new("epsilon")]),
        outstanding: BTreeSet::from([TestCaseName::new("delta"), TestCaseName::new("zeta")]),
    };
    let mut rerun_filter = default_filter();
    rerun_filter.set_outstanding_tests(ComputedRerunInfo {
        test_suites: iddqd::id_ord_map! { rerun_suite },
    });

    let rerun_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &rerun_filter,
        &partitioner,
    );

    // binary1: delta(0)=shard1, epsilon(1)=RerunAlreadyPassed(counted
    // as shard2), zeta(2)=shard1.
    let binary1_suite = rerun_list
        .get_suite(&binary1_id)
        .expect("binary1 should exist");
    let binary1_results: Vec<_> = binary1_suite
        .status
        .test_cases()
        .map(|tc| (tc.name.as_str(), tc.test_info.filter_match))
        .collect();
    assert_eq!(
        binary1_results,
        vec![
            ("delta", FilterMatch::Matches),
            (
                "epsilon",
                FilterMatch::Mismatch {
                    reason: MismatchReason::RerunAlreadyPassed,
                }
            ),
            ("zeta", FilterMatch::Matches),
        ]
    );

    // binary2: alpha(3)=Partition, beta(4)=shard1, delta(5)=Partition,
    // gamma(6)=shard1.
    let binary2_suite = rerun_list
        .get_suite(&binary2_id)
        .expect("binary2 should exist");
    let binary2_results: Vec<_> = binary2_suite
        .status
        .test_cases()
        .map(|tc| (tc.name.as_str(), tc.test_info.filter_match))
        .collect();
    assert_eq!(
        binary2_results,
        vec![
            (
                "alpha",
                FilterMatch::Mismatch {
                    reason: MismatchReason::Partition,
                }
            ),
            ("beta", FilterMatch::Matches),
            (
                "delta",
                FilterMatch::Mismatch {
                    reason: MismatchReason::Partition,
                }
            ),
            ("gamma", FilterMatch::Matches),
        ]
    );

    // Counterfactual: without rerun info, shard 1 still picks global
    // even indices (0, 2, 4, 6), so the running set is identical.
    let no_rerun_list = build_test_list(
        [
            ("pkg::binary1", THREE_TEST_OUTPUT, ""),
            ("pkg::binary2", FOUR_TEST_OUTPUT, ""),
        ],
        &default_filter(),
        &partitioner,
    );

    // The running tests must be identical: {delta, zeta} from binary1,
    // {beta, gamma} from binary2.
    let rerun_running = collect_all_matching_tests(&rerun_list);
    let no_rerun_running = collect_all_matching_tests(&no_rerun_list);
    assert_eq!(
        rerun_running, no_rerun_running,
        "RerunAlreadyPassed test must participate in cross-binary counting",
    );
}

/// Verifies that tests filtered out by name patterns do not participate
/// in the cross-binary counter. This is the cross-binary analog of
/// `test_partition_prefiltered_excluded_from_counting`: because `slice`
/// uses a single counter, a name-filtered test in one binary would
/// shift assignments in subsequent binaries if it were counted.
#[test]
fn test_slice_prefiltered_excluded_from_cross_binary_counting() {
    // binary1 (FOUR_TEST_OUTPUT): alpha, beta, delta, gamma — all
    // contain "a".
    // binary2 (THREE_TEST_OUTPUT): delta, epsilon, zeta — epsilon
    // does not contain "a".
    //
    // With name filter "a":
    //   binary1: alpha(0), beta(1), delta(2), gamma(3) — 4 matching.
    //   binary2: delta(4), zeta(5) — 2 matching; epsilon filtered.
    //   Total matching: 6. slice:1/2 picks even indices: 0, 2, 4.
    //
    // Without name filter:
    //   binary1: alpha(0), beta(1), delta(2), gamma(3) — 4.
    //   binary2: delta(4), epsilon(5), zeta(6) — 3.
    //   Total: 7. slice:1/2 picks even indices: 0, 2, 4, 6.
    let name_filter = TestFilter::new(
        NextestRunMode::Test,
        RunIgnored::Default,
        TestFilterPatterns::new(vec!["a".to_string()]),
        Vec::new(),
    )
    .unwrap();
    let partitioner = slice_partitioner(1, 2);

    let filtered_list = build_test_list(
        [
            ("pkg::binary1", FOUR_TEST_OUTPUT, ""),
            ("pkg::binary2", THREE_TEST_OUTPUT, ""),
        ],
        &name_filter,
        &partitioner,
    );

    // Verify epsilon was filtered out by name, not by partition.
    let binary2_suite = filtered_list
        .get_suite(&RustBinaryId::new("pkg::binary2"))
        .expect("binary2 should exist");
    let epsilon = binary2_suite
        .status
        .get(&TestCaseName::new("epsilon"))
        .expect("epsilon should exist");
    assert_eq!(
        epsilon.test_info.filter_match,
        FilterMatch::Mismatch {
            reason: MismatchReason::String,
        }
    );

    // With name filter: shard 1 gets alpha, delta from binary1 and
    // delta from binary2.
    let filtered_running = collect_all_matching_tests(&filtered_list);
    assert_eq!(
        filtered_running,
        vec![
            ("pkg::binary1", "alpha"),
            ("pkg::binary1", "delta"),
            ("pkg::binary2", "delta"),
        ],
    );

    // Without name filter: shard 1 gets alpha, delta from binary1 and
    // delta, zeta from binary2. The sets differ because the name-filtered
    // epsilon did not participate in the global counter. If it had been
    // counted, binary2's delta would be at global index 5 (odd, shard 2)
    // rather than 4 (even, shard 1).
    let no_filter_list = build_test_list(
        [
            ("pkg::binary1", FOUR_TEST_OUTPUT, ""),
            ("pkg::binary2", THREE_TEST_OUTPUT, ""),
        ],
        &default_filter(),
        &partitioner,
    );
    let unfiltered_running = collect_all_matching_tests(&no_filter_list);
    assert_eq!(
        unfiltered_running,
        vec![
            ("pkg::binary1", "alpha"),
            ("pkg::binary1", "delta"),
            ("pkg::binary2", "delta"),
            ("pkg::binary2", "zeta"),
        ],
    );
}

// --- Slice partitioning property-based tests ---

/// Generates a test output string with `count` tests named `t_000`,
/// `t_001`, etc., starting from `offset`. Zero-padded names ensure
/// stable lexicographic sort order.
fn make_test_output(count: usize, offset: usize) -> String {
    (0..count)
        .map(|i| format!("t_{:03}: test", offset + i))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds a `TestList` from per-binary `(non_ignored_count,
/// ignored_count)` specs. Generates deterministic binary IDs
/// (`pkg::bin_00`, `pkg::bin_01`, ...) and test names (`t_000`,
/// `t_001`, ...) with non-ignored names first, then ignored names
/// starting at `offset = non_ignored_count` to avoid collisions.
fn build_test_list_from_specs(
    specs: &[(usize, usize)],
    filter: &TestFilter,
    partitioner: &PartitionerBuilder,
) -> TestList<'static> {
    let ecx = simple_ecx();
    let artifacts: Vec<_> = specs
        .iter()
        .enumerate()
        .map(|(i, &(non_ign_count, ign_count))| {
            let binary_id = format!("pkg::bin_{i:02}");
            let non_ign_output = make_test_output(non_ign_count, 0);
            let ign_output = make_test_output(ign_count, non_ign_count);
            (make_test_artifact(&binary_id), non_ign_output, ign_output)
        })
        .collect();
    TestList::new_with_outputs(
        artifacts,
        Utf8PathBuf::from("/fake/path"),
        simple_build_meta(),
        filter,
        Some(partitioner),
        EnvironmentMap::empty(),
        &ecx,
        FilterBound::All,
    )
    .expect("valid output")
}

/// All N shards of `slice:M/N` together cover every test exactly once:
/// no duplicates (disjoint) and no omissions (complete).
#[proptest(cases = 64)]
fn proptest_slice_shards_complete_and_disjoint(input: PartitionTestInput) {
    let filter = default_filter();
    let specs = input.non_ignored_only_specs();
    let total_tests = input.total_non_ignored();

    let shard_lists: Vec<_> = (1..=input.total_shards)
        .map(|shard| {
            build_test_list_from_specs(
                &specs,
                &filter,
                &slice_partitioner(shard, input.total_shards),
            )
        })
        .collect();

    let mut all_tests: Vec<(String, String)> = Vec::new();
    for test_list in &shard_lists {
        for suite in test_list.iter() {
            let binary_id = suite.binary_id.as_str();
            for tc in suite.status.test_cases() {
                if tc.test_info.filter_match.is_match() {
                    all_tests.push((binary_id.to_owned(), tc.name.as_str().to_owned()));
                }
            }
        }
    }

    // Total matched tests across all shards should equal the total test count.
    prop_assert_eq!(
        all_tests.len(),
        total_tests,
        "union of all shards should cover all {} tests",
        total_tests,
    );

    // No duplicates: sorting + dedup should not reduce the count.
    all_tests.sort();
    let len_before_dedup = all_tests.len();
    all_tests.dedup();
    prop_assert_eq!(
        all_tests.len(),
        len_before_dedup,
        "shards must be disjoint (no duplicate tests)",
    );
}

/// Each shard of `slice:M/N` gets either `floor(T/N)` or `ceil(T/N)`
/// tests (even distribution), and the `run_count + skip_count ==
/// test_count` invariant holds.
#[proptest(cases = 64)]
fn proptest_slice_even_distribution(input: PartitionTestInput) {
    let filter = default_filter();
    let specs = input.non_ignored_only_specs();
    let total_tests = input.total_non_ignored();

    let test_list = build_test_list_from_specs(
        &specs,
        &filter,
        &slice_partitioner(input.shard, input.total_shards),
    );

    // test_count reflects all tests, not just the selected shard.
    prop_assert_eq!(test_list.test_count(), total_tests);

    // run_count + skip_count == test_count.
    prop_assert_eq!(
        test_list.run_count() + test_list.skip_counts().skipped_tests,
        test_list.test_count(),
        "run_count + skip_count must equal test_count",
    );

    // Each shard should get floor(T/N) or ceil(T/N) tests.
    let n = input.total_shards as usize;
    let min_per_shard = total_tests / n;
    let max_per_shard = min_per_shard + usize::from(total_tests % n != 0);

    prop_assert!(
        test_list.run_count() >= min_per_shard && test_list.run_count() <= max_per_shard,
        "shard {}/{} with {} total tests: run_count={} not in [{}, {}]",
        input.shard,
        input.total_shards,
        total_tests,
        test_list.run_count(),
        min_per_shard,
        max_per_shard,
    );
}

/// Ignored tests do not affect which non-ignored tests are selected:
/// each group gets independent cross-binary partitioner state.
#[proptest(cases = 64)]
fn proptest_slice_ignored_independence(input: PartitionTestInput) {
    let partitioner = slice_partitioner(input.shard, input.total_shards);

    // With ignored tests present (RunIgnored::All so both groups appear).
    let filter_all = TestFilter::new(
        NextestRunMode::Test,
        RunIgnored::All,
        TestFilterPatterns::default(),
        Vec::new(),
    )
    .unwrap();
    let list_with_ign = build_test_list_from_specs(&input.binary_specs, &filter_all, &partitioner);

    // Without ignored tests (RunIgnored::Default, zero ignored output).
    let filter_default = default_filter();
    let specs_no_ign = input.non_ignored_only_specs();
    let list_no_ign = build_test_list_from_specs(&specs_no_ign, &filter_default, &partitioner);

    // The non-ignored matching tests must be identical in both cases.
    prop_assert_eq!(
        collect_non_ignored_matching_tests(&list_with_ign),
        collect_non_ignored_matching_tests(&list_no_ign),
        "non-ignored test selection must be independent of ignored tests",
    );
}
