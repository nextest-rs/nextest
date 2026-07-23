// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use nextest_metadata::MismatchReason;
use serde::{Deserialize, Serialize};

/// Controls which skipped tests are emitted as `<testcase>` elements with a
/// `<skipped>` child in machine-readable reports such as JUnit XML output.
///
/// Skipped tests are not emitted by default to keep machine-readable output
/// stable. This setting opts in to reporting some or all of them.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum ReportSkipPolicy {
    /// Do not emit any skipped tests. This is the default and keeps
    /// machine-readable output stable.
    #[default]
    None,

    /// Only emit tests whose ignore status did not match the run's run-ignored
    /// selection. In a default run these are the tests marked `#[ignore]`;
    /// under `--run-ignored only` these are the tests that are not ignored.
    Ignored,

    /// Emit all skipped tests, regardless of why they were skipped, except
    /// tests skipped because they are not benchmarks.
    ///
    /// Note: because filtered-out tests (for example, tests in a different
    /// partition) are reported as skipped, using `all` with partitioned runs
    /// causes each partition's report to contain the tests skipped in that
    /// partition. Merging those reports will produce duplicate skipped entries.
    All,
}

impl ReportSkipPolicy {
    /// Returns true if a test skipped for the given reason should be reported
    /// as a skipped test under this policy.
    ///
    /// Tests skipped because they are not benchmarks
    /// ([`MismatchReason::NotBenchmark`]) are never reported.
    pub fn should_report(self, reason: MismatchReason) -> bool {
        match self {
            ReportSkipPolicy::None => false,
            ReportSkipPolicy::Ignored => reason.is_ignore_mismatch(),
            ReportSkipPolicy::All => reason.is_substantive_skip(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_policy_behavior() {
        assert!(!ReportSkipPolicy::None.should_report(MismatchReason::Ignored));
        assert!(!ReportSkipPolicy::None.should_report(MismatchReason::DefaultFilter));
        assert!(!ReportSkipPolicy::None.should_report(MismatchReason::NotBenchmark));

        assert!(ReportSkipPolicy::Ignored.should_report(MismatchReason::Ignored));
        assert!(!ReportSkipPolicy::Ignored.should_report(MismatchReason::DefaultFilter));
        assert!(!ReportSkipPolicy::Ignored.should_report(MismatchReason::NotBenchmark));

        assert!(ReportSkipPolicy::All.should_report(MismatchReason::Ignored));
        assert!(ReportSkipPolicy::All.should_report(MismatchReason::DefaultFilter));
        assert!(!ReportSkipPolicy::All.should_report(MismatchReason::NotBenchmark));
    }

    #[test]
    fn should_report_mirrors_reason_predicates() {
        for &reason in MismatchReason::ALL_VARIANTS {
            assert!(
                !ReportSkipPolicy::None.should_report(reason),
                "None never reports {reason:?}"
            );
            assert_eq!(
                ReportSkipPolicy::Ignored.should_report(reason),
                reason.is_ignore_mismatch(),
                "Ignored policy must mirror is_ignore_mismatch for {reason:?}"
            );
            assert_eq!(
                ReportSkipPolicy::All.should_report(reason),
                reason.is_substantive_skip(),
                "All policy must mirror is_substantive_skip for {reason:?}"
            );
        }
    }
}
