// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display helpers for durations.

use crate::{
    config::CompiledDefaultFilter, helpers::plural, list::SkipCounts, reporter::helpers::Styles,
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
        write!(f, "[{:>9}] ", out)
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
    skip_counts: &SkipCounts,
    default_filter: &CompiledDefaultFilter,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    if skip_counts.skipped_tests > 0 || skip_counts.skipped_binaries > 0 {
        write!(writer, " (")?;
        write_skip_counts_impl(
            skip_counts.skipped_tests,
            skip_counts.skipped_binaries,
            styles,
            writer,
        )?;

        // Were all tests and binaries that were skipped, skipped due to being in the
        // default set?
        if skip_counts.skipped_tests == skip_counts.skipped_tests_default_filter
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
            plural::tests_str(skipped_tests),
            skipped_binaries.style(styles.count),
            plural::binaries_str(skipped_binaries),
        )?;
    } else if skipped_tests > 0 {
        write!(
            writer,
            "{} {}",
            skipped_tests.style(styles.count),
            plural::tests_str(skipped_tests),
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

#[cfg(test)]
mod tests {
    use nextest_filtering::CompiledExpr;

    use crate::config::CompiledDefaultFilterSection;

    use super::*;

    #[test]
    fn test_write_skip_counts() {
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, false), @" (1 binary skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 2,
        }, true), @" (2 binaries skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 3,
        }, false), @" (2 tests and 3 binaries skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test and 1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests and 3 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped, including 1 binary via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_default_filter: 2,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (3 tests and 1 binary skipped, including 2 tests via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @"");
    }

    fn skip_counts_str(skip_counts: &SkipCounts, override_section: bool) -> String {
        let mut buf = Vec::new();
        write_skip_counts(
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
}
