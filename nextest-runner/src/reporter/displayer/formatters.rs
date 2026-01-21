// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display helpers for durations.

use crate::{
    config::overrides::CompiledDefaultFilter,
    helpers::plural,
    list::SkipCounts,
    reporter::{
        events::{CancelReason, FinalRunStats, RunStatsFailureKind, UnitKind},
        helpers::Styles,
    },
    run_mode::NextestRunMode,
    write_str::WriteStr,
};
use owo_colors::OwoColorize;
use std::{fmt, io, time::Duration};

pub(super) struct DisplayBracketedHhMmSs(pub(super) Duration);

impl fmt::Display for DisplayBracketedHhMmSs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This matches indicatif's elapsed_precise display.
        let total_secs = self.0.as_secs();
        let secs = total_secs % 60;
        let total_mins = total_secs / 60;
        let mins = total_mins % 60;
        let hours = total_mins / 60;

        // Buffer the output internally to provide padding.
        let out = format!("{hours:02}:{mins:02}:{secs:02}");
        write!(f, "[{out:>9}] ")
    }
}

pub(super) struct DisplayHhMmSs {
    pub(super) duration: Duration,
    // True if floor, false if ceiling.
    pub(super) floor: bool,
}

impl fmt::Display for DisplayHhMmSs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This matches indicatif's elapsed_precise display.
        let total_secs = self.duration.as_secs();
        let total_secs = if !self.floor && self.duration.subsec_millis() > 0 {
            total_secs + 1
        } else {
            total_secs
        };
        let secs = total_secs % 60;
        let total_mins = total_secs / 60;
        let mins = total_mins % 60;
        let hours = total_mins / 60;

        // Buffer the output internally to provide padding.
        let out = format!("{hours:02}:{mins:02}:{secs:02}");
        write!(f, "{out}")
    }
}

pub(super) struct DisplayBracketedDuration(pub(super) Duration);

impl fmt::Display for DisplayBracketedDuration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(f, "[{:>8.3?}s] ", self.0.as_secs_f64())
    }
}

pub(super) struct DisplayDurationBy(pub(super) Duration);

impl fmt::Display for DisplayDurationBy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(f, "by {:>7.3?}s ", self.0.as_secs_f64())
    }
}

pub(super) struct DisplaySlowDuration(pub(super) Duration);

impl fmt::Display for DisplaySlowDuration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Inside the curly braces:
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        //
        // The > outside the curly braces is printed literally.
        write!(f, "[>{:>7.3?}s] ", self.0.as_secs_f64())
    }
}

pub(super) fn write_skip_counts(
    mode: NextestRunMode,
    skip_counts: &SkipCounts,
    default_filter: &CompiledDefaultFilter,
    styles: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    // When running in benchmark mode, we don't display skip counts for
    // tests that were skipped but not benchmarks.
    let real_skipped_tests = if mode == NextestRunMode::Benchmark {
        skip_counts
            .skipped_tests
            .saturating_sub(skip_counts.skipped_tests_non_benchmark)
    } else {
        skip_counts.skipped_tests
    };

    if real_skipped_tests > 0 || skip_counts.skipped_binaries > 0 {
        write!(writer, " (")?;
        write_skip_counts_impl(
            mode,
            real_skipped_tests,
            skip_counts.skipped_binaries,
            styles,
            writer,
        )?;

        // Check if all skipped items are accounted for by a single category.
        let all_rerun = real_skipped_tests == skip_counts.skipped_tests_rerun
            && skip_counts.skipped_binaries == 0;
        let all_default_filter = real_skipped_tests == skip_counts.skipped_tests_default_filter
            && skip_counts.skipped_binaries == skip_counts.skipped_binaries_default_filter;

        if all_rerun {
            // All tests skipped because they already passed in a previous run.
            write!(
                writer,
                " {} that already passed",
                "skipped".style(styles.skip)
            )?;
        } else if all_default_filter {
            // All tests and binaries skipped due to default filter.
            write!(
                writer,
                " {} via {}",
                "skipped".style(styles.skip),
                default_filter.display_config(styles.count)
            )?;
        } else {
            write!(writer, " {}", "skipped".style(styles.skip))?;

            // Show "including" clause for rerun and/or default filter.
            let has_rerun = skip_counts.skipped_tests_rerun > 0;
            let has_default_filter = skip_counts.skipped_binaries_default_filter > 0
                || skip_counts.skipped_tests_default_filter > 0;

            if has_rerun || has_default_filter {
                write!(writer, ", including ")?;

                if has_rerun {
                    write!(
                        writer,
                        "{} {} that already passed",
                        skip_counts.skipped_tests_rerun.style(styles.count),
                        plural::tests_str(mode, skip_counts.skipped_tests_rerun),
                    )?;

                    if has_default_filter {
                        write!(writer, ", and ")?;
                    }
                }

                if has_default_filter {
                    write_skip_counts_impl(
                        mode,
                        skip_counts.skipped_tests_default_filter,
                        skip_counts.skipped_binaries_default_filter,
                        styles,
                        writer,
                    )?;
                    write!(
                        writer,
                        " via {}",
                        default_filter.display_config(styles.count)
                    )?;
                }
            }
        }
        write!(writer, ")")?;
    }

    Ok(())
}

fn write_skip_counts_impl(
    mode: NextestRunMode,
    skipped_tests: usize,
    skipped_binaries: usize,
    styles: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    // X tests and Y binaries skipped, or X tests skipped, or Y binaries skipped.
    if skipped_tests > 0 && skipped_binaries > 0 {
        write!(
            writer,
            "{} {} and {} {}",
            skipped_tests.style(styles.count),
            plural::tests_str(mode, skipped_tests),
            skipped_binaries.style(styles.count),
            plural::binaries_str(skipped_binaries),
        )?;
    } else if skipped_tests > 0 {
        write!(
            writer,
            "{} {}",
            skipped_tests.style(styles.count),
            plural::tests_str(mode, skipped_tests),
        )?;
    } else if skipped_binaries > 0 {
        write!(
            writer,
            "{} {}",
            skipped_binaries.style(styles.count),
            plural::binaries_str(skipped_binaries),
        )?;
    }

    Ok(())
}

pub(super) fn write_final_warnings(
    mode: NextestRunMode,
    final_stats: FinalRunStats,
    styles: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    match final_stats {
        FinalRunStats::Failed {
            kind:
                RunStatsFailureKind::Test {
                    initial_run_count,
                    not_run,
                },
        } if not_run > 0 => {
            write_final_warnings_for_failure(
                mode,
                initial_run_count,
                not_run,
                Some(CancelReason::TestFailure),
                styles,
                writer,
            )?;
        }
        FinalRunStats::Cancelled {
            reason,
            kind:
                RunStatsFailureKind::Test {
                    initial_run_count,
                    not_run,
                },
        } if not_run > 0 => {
            write_final_warnings_for_failure(
                mode,
                initial_run_count,
                not_run,
                reason,
                styles,
                writer,
            )?;
        }
        _ => {}
    }

    Ok(())
}

fn write_final_warnings_for_failure(
    mode: NextestRunMode,
    initial_run_count: usize,
    not_run: usize,
    reason: Option<CancelReason>,
    styles: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    match reason {
        Some(reason @ CancelReason::TestFailure | reason @ CancelReason::TestFailureImmediate) => {
            writeln!(
                writer,
                "{}: {}/{} {} {} not run due to {} (run with {} to run all tests, or run with {})",
                "warning".style(styles.skip),
                not_run.style(styles.count),
                initial_run_count.style(styles.count),
                plural::tests_plural_if(mode, initial_run_count != 1 || not_run != 1),
                plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                reason.to_static_str().style(styles.skip),
                "--no-fail-fast".style(styles.count),
                "--max-fail".style(styles.count),
            )?;
        }
        _ => {
            let due_to_reason = match reason {
                Some(reason) => {
                    format!(" due to {}", reason.to_static_str().style(styles.skip))
                }
                None => "".to_string(),
            };
            writeln!(
                writer,
                "{}: {}/{} {} {} not run{}",
                "warning".style(styles.skip),
                not_run.style(styles.count),
                initial_run_count.style(styles.count),
                plural::tests_plural_if(mode, initial_run_count != 1 || not_run != 1),
                plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                due_to_reason,
            )?;
        }
    }

    Ok(())
}

pub(crate) struct DisplayUnitKind {
    mode: NextestRunMode,
    kind: UnitKind,
}

impl DisplayUnitKind {
    pub(crate) fn new(mode: NextestRunMode, kind: UnitKind) -> Self {
        Self { mode, kind }
    }
}

impl fmt::Display for DisplayUnitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            UnitKind::Test => {
                if self.mode == NextestRunMode::Benchmark {
                    write!(f, "benchmark")
                } else {
                    write!(f, "test")
                }
            }
            UnitKind::Script => {
                write!(f, "script")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::overrides::CompiledDefaultFilterSection;
    use nextest_filtering::CompiledExpr;

    #[test]
    fn test_write_skip_counts() {
        // All tests skipped via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped via profile.my-profile.default-filter)");

        // Tests skipped for other reasons (not default filter or rerun).
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped)");

        // Binaries skipped via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, false), @" (1 binary skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 2,
        }, true), @" (2 binaries skipped via default-filter in profile.my-profile.overrides)");

        // Binaries skipped for other reasons.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 binaries skipped)");

        // Tests and binaries skipped via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 3,
        }, false), @" (2 tests and 3 binaries skipped via profile.my-profile.default-filter)");

        // Tests and binaries skipped for other reasons.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test and 1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests and 3 binaries skipped)");

        // Mixed: tests skipped for other reasons, binaries skipped via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped, including 1 binary via default-filter in profile.my-profile.overrides)");

        // Mixed: some tests via default filter, others not.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 3,
            skipped_tests_default_filter: 2,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (3 tests and 1 binary skipped, including 2 tests via profile.my-profile.default-filter)");

        // No tests or binaries skipped.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @"");

        // --- Rerun tests ---

        // All tests skipped due to rerun (already passed).
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped that already passed)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 5,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 5,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (5 tests skipped that already passed)");

        // Some tests skipped due to rerun, some for other reasons.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 3,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 5,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (5 tests skipped, including 3 tests that already passed)");

        // Tests skipped due to rerun with binaries skipped.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests and 1 binary skipped, including 2 tests that already passed)");

        // Tests skipped due to rerun with binaries skipped via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, false), @" (2 tests and 1 binary skipped, including 2 tests that already passed, and 1 binary via profile.my-profile.default-filter)");

        // Mixed: some tests rerun, some tests via default filter.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 5,
            skipped_tests_default_filter: 3,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (5 tests skipped, including 2 tests that already passed, and 3 tests via profile.my-profile.default-filter)");

        // Mixed: rerun, default filter, and binaries.
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests_rerun: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests: 6,
            skipped_tests_default_filter: 3,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 1,
        }, true), @" (6 tests and 2 binaries skipped, including 2 tests that already passed, and 3 tests and 1 binary via default-filter in profile.my-profile.overrides)");
    }

    fn skip_counts_str(skip_counts: &SkipCounts, override_section: bool) -> String {
        skip_counts_str_impl(NextestRunMode::Test, skip_counts, override_section)
    }

    fn skip_counts_str_impl(
        mode: NextestRunMode,
        skip_counts: &SkipCounts,
        override_section: bool,
    ) -> String {
        let mut buf = String::new();
        write_skip_counts(
            mode,
            skip_counts,
            &CompiledDefaultFilter {
                expr: CompiledExpr::ALL,
                profile: "my-profile".to_owned(),
                section: if override_section {
                    CompiledDefaultFilterSection::Override(0)
                } else {
                    CompiledDefaultFilterSection::Profile
                },
            },
            &Styles::default(),
            &mut buf,
        )
        .unwrap();
        buf
    }

    #[test]
    fn test_final_warnings() {
        let warnings = final_warnings_for(FinalRunStats::Failed {
            kind: RunStatsFailureKind::Test {
                initial_run_count: 3,
                not_run: 1,
            },
        });
        assert_eq!(
            warnings,
            "warning: 1/3 tests were not run due to test failure \
             (run with --no-fail-fast to run all tests, or run with --max-fail)\n"
        );

        let warnings = final_warnings_for(FinalRunStats::Cancelled {
            reason: Some(CancelReason::Signal),
            kind: RunStatsFailureKind::Test {
                initial_run_count: 8,
                not_run: 5,
            },
        });
        assert_eq!(warnings, "warning: 5/8 tests were not run due to signal\n");

        let warnings = final_warnings_for(FinalRunStats::Cancelled {
            reason: Some(CancelReason::Interrupt),
            kind: RunStatsFailureKind::Test {
                initial_run_count: 1,
                not_run: 1,
            },
        });
        assert_eq!(warnings, "warning: 1/1 test was not run due to interrupt\n");

        // This warning is taken care of by cargo-nextest.
        let warnings = final_warnings_for(FinalRunStats::NoTestsRun);
        assert_eq!(warnings, "");

        // No warnings for success.
        let warnings = final_warnings_for(FinalRunStats::Success);
        assert_eq!(warnings, "");

        // No warnings for setup script failure.
        let warnings = final_warnings_for(FinalRunStats::Failed {
            kind: RunStatsFailureKind::SetupScript,
        });
        assert_eq!(warnings, "");

        // No warnings for setup script cancellation.
        let warnings = final_warnings_for(FinalRunStats::Cancelled {
            reason: Some(CancelReason::Interrupt),
            kind: RunStatsFailureKind::SetupScript,
        });
        assert_eq!(warnings, "");
    }

    fn final_warnings_for(stats: FinalRunStats) -> String {
        let mut buf = String::new();
        let styles = Styles::default();
        write_final_warnings(NextestRunMode::Test, stats, &styles, &mut buf).unwrap();
        buf
    }
}
