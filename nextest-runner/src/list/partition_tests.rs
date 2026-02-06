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
    run_mode::NextestRunMode,
    test_filter::{FilterBound, RunIgnored, TestFilter, TestFilterPatterns},
};
use camino::Utf8PathBuf;
use nextest_metadata::{FilterMatch, MismatchReason, RustBinaryId, TestCaseName};

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
    use crate::record::{ComputedRerunInfo, RerunTestSuiteInfo};
    use std::collections::BTreeSet;

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
