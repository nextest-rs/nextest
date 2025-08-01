// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display helpers for durations.

use crate::{
    config::CompiledDefaultFilter,
    helpers::plural,
    list::SkipCounts,
    reporter::{
        events::{CancelReason, FinalRunStats, RunStatsFailureKind, UnitKind},
        helpers::Styles,
    },
    run_mode::NextestRunMode,
};
use owo_colors::OwoColorize;
use std::{
    fmt,
    io::{self, Write},
    time::Duration,
};

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
    writer: &mut dyn Write,
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

        // Were all tests and binaries that were skipped, skipped due to being in the
        // default set?
        if real_skipped_tests == skip_counts.skipped_tests_default_filter
            && skip_counts.skipped_binaries == skip_counts.skipped_binaries_default_filter
        {
            write!(
                writer,
                " {} via {}",
                "skipped".style(styles.skip),
                default_filter.display_config(styles.count)
            )?;
        } else {
            write!(writer, " {}", "skipped".style(styles.skip))?;
            // Were *any* tests in the default set?
            if skip_counts.skipped_binaries_default_filter > 0
                || skip_counts.skipped_tests_default_filter > 0
            {
                write!(writer, ", including ")?;
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
        write!(writer, ")")?;
    }

    Ok(())
}

fn write_skip_counts_impl(
    mode: NextestRunMode,
    skipped_tests: usize,
    skipped_binaries: usize,
    styles: &Styles,
    writer: &mut dyn Write,
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
    cancel_status: Option<CancelReason>,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    match final_stats {
        FinalRunStats::Failed(RunStatsFailureKind::Test {
            initial_run_count,
            not_run,
        })
        | FinalRunStats::Cancelled(RunStatsFailureKind::Test {
            initial_run_count,
            not_run,
        }) if not_run > 0 => {
            if cancel_status == Some(CancelReason::TestFailure) {
                writeln!(
                    writer,
                    "{}: {}/{} {} {} not run due to {} (run with {} to run all tests, or run with {})",
                    "warning".style(styles.skip),
                    not_run.style(styles.count),
                    initial_run_count.style(styles.count),
                    plural::tests_plural_if(mode, initial_run_count != 1 || not_run != 1),
                    plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                    CancelReason::TestFailure.to_static_str().style(styles.skip),
                    "--no-fail-fast".style(styles.count),
                    "--max-fail".style(styles.count),
                )?;
            } else {
                let due_to_reason = match cancel_status {
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
        _ => {}
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
    use crate::config::CompiledDefaultFilterSection;
    use nextest_filtering::CompiledExpr;

    #[test]
    fn test_write_skip_counts() {
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 1,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 2,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, false), @" (1 binary skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 2,
        }, true), @" (2 binaries skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 1,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 2,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 3,
        }, false), @" (2 tests and 3 binaries skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test and 1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests and 3 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped, including 1 binary via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 2,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (3 tests and 1 binary skipped, including 2 tests via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @"");

        // Benchmark mode skip counts.

        insta::assert_snapshot!(skip_counts_str_benchmark(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_non_benchmark: 3,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, true), @"");

        insta::assert_snapshot!(skip_counts_str_benchmark(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_non_benchmark: 3,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, true), @" (1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str_benchmark(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_non_benchmark: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, true), @" (2 benchmarks skipped, including 1 benchmark via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str_benchmark(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 2,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, true), @" (3 benchmarks and 1 binary skipped, including 2 benchmarks via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str_benchmark(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_non_benchmark: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @"");
    }

    fn skip_counts_str(skip_counts: &SkipCounts, override_section: bool) -> String {
        skip_counts_str_imp(NextestRunMode::Test, skip_counts, override_section)
    }

    fn skip_counts_str_benchmark(skip_counts: &SkipCounts, override_section: bool) -> String {
        skip_counts_str_imp(NextestRunMode::Benchmark, skip_counts, override_section)
    }

    fn skip_counts_str_imp(
        mode: NextestRunMode,
        skip_counts: &SkipCounts,
        override_section: bool,
    ) -> String {
        let mut buf = Vec::new();
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
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_final_warnings() {
        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 3,
                not_run: 1,
            }),
            Some(CancelReason::TestFailure),
        );
        assert_eq!(
            warnings,
            "warning: 1/3 tests were not run due to test failure \
             (run with --no-fail-fast to run all tests, or run with --max-fail)\n"
        );

        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 8,
                not_run: 5,
            }),
            Some(CancelReason::Signal),
        );
        assert_eq!(warnings, "warning: 5/8 tests were not run due to signal\n");

        let warnings = final_warnings_for(
            FinalRunStats::Cancelled(RunStatsFailureKind::Test {
                initial_run_count: 1,
                not_run: 1,
            }),
            Some(CancelReason::Interrupt),
        );
        assert_eq!(warnings, "warning: 1/1 test was not run due to interrupt\n");

        // These warnings are taken care of by cargo-nextest.
        let warnings = final_warnings_for(FinalRunStats::NoTestsRun, None);
        assert_eq!(warnings, "");
        let warnings = final_warnings_for(FinalRunStats::NoTestsRun, Some(CancelReason::Signal));
        assert_eq!(warnings, "");

        // No warnings for success.
        let warnings = final_warnings_for(FinalRunStats::Success, None);
        assert_eq!(warnings, "");

        // No warnings for setup script failure.
        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript),
            Some(CancelReason::SetupScriptFailure),
        );
        assert_eq!(warnings, "");

        // No warnings for setup script cancellation.
        let warnings = final_warnings_for(
            FinalRunStats::Cancelled(RunStatsFailureKind::SetupScript),
            Some(CancelReason::Interrupt),
        );
        assert_eq!(warnings, "");
    }

    fn final_warnings_for(stats: FinalRunStats, cancel_status: Option<CancelReason>) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let styles = Styles::default();
        write_final_warnings(
            NextestRunMode::Test,
            stats,
            cancel_status,
            &styles,
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }
}
