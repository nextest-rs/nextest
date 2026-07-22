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

    /// Only emit tests that were skipped because they are ignored (via
    /// `#[ignore]` and not selected by the run-ignored option).
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
        // Match `MismatchReason` variants explicitly (rather than a bare
        // wildcard) so that the behavior for each known reason is spelled out.
        //
        // `MismatchReason` is `#[non_exhaustive]`, so the compiler requires a
        // wildcard arm here; a new variant therefore will not produce a compile
        // error in this crate. The `should_report_covers_all_variants` test
        // below iterates `MismatchReason::ALL_VARIANTS` to guard against a new
        // variant being handled unintentionally.
        match self {
            ReportSkipPolicy::None => false,
            ReportSkipPolicy::Ignored => match reason {
                MismatchReason::Ignored => true,
                MismatchReason::NotBenchmark
                | MismatchReason::String
                | MismatchReason::Expression
                | MismatchReason::Partition
                | MismatchReason::RerunAlreadyPassed
                | MismatchReason::DefaultFilter => false,
                _ => false,
            },
            ReportSkipPolicy::All => match reason {
                MismatchReason::NotBenchmark => false,
                MismatchReason::Ignored
                | MismatchReason::String
                | MismatchReason::Expression
                | MismatchReason::Partition
                | MismatchReason::RerunAlreadyPassed
                | MismatchReason::DefaultFilter => true,
                _ => true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_covers_all_variants() {
        // Guard against a new `MismatchReason` variant being added without an
        // explicit decision in `should_report`. `MismatchReason` is
        // `#[non_exhaustive]`, so exhaustive matching cannot be enforced at
        // compile time from this crate; this test iterates over the known
        // variants instead.
        for &reason in MismatchReason::ALL_VARIANTS {
            // `None` never reports anything.
            assert!(!ReportSkipPolicy::None.should_report(reason));

            // `Ignored` reports only ignored tests.
            assert_eq!(
                ReportSkipPolicy::Ignored.should_report(reason),
                matches!(reason, MismatchReason::Ignored),
            );

            // `All` reports everything except non-benchmark skips.
            assert_eq!(
                ReportSkipPolicy::All.should_report(reason),
                !matches!(reason, MismatchReason::NotBenchmark),
            );
        }
    }
}
