// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display wrappers for record store types.
//!
//! This module provides formatting and display utilities for recorded run
//! information, prune operations, and run lists.

use super::{
    PruneKind, PrunePlan, PruneResult, RecordedRunInfo, RecordedRunStatus, RunStoreSnapshot,
    run_id_index::RunIdIndex,
    store::{CompletedRunStats, StressCompletedRunStats},
};
use crate::{
    helpers::{ThemeCharacters, plural},
    redact::Redactor,
};
use camino::Utf8Path;
use owo_colors::{OwoColorize, Style};
use std::{error::Error, fmt};
use swrite::{SWrite, swrite};

/// Formats a byte count as a human-readable size string (KB or MB).
pub fn format_size_display(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// Styles for displaying record store information.
#[derive(Clone, Debug, Default)]
pub struct Styles {
    /// Style for the unique prefix portion of run IDs.
    pub run_id_prefix: Style,
    /// Style for the non-unique rest portion of run IDs.
    pub run_id_rest: Style,
    /// Style for timestamps.
    pub timestamp: Style,
    /// Style for duration values.
    pub duration: Style,
    /// Style for size values.
    pub size: Style,
    /// Style for counts and numbers.
    pub count: Style,
    /// Style for "passed" status.
    pub passed: Style,
    /// Style for "failed" status.
    pub failed: Style,
    /// Style for "cancelled" or "incomplete" status.
    pub cancelled: Style,
}

impl Styles {
    /// Colorizes the styles for terminal output.
    pub fn colorize(&mut self) {
        self.run_id_prefix = Style::new().bold().purple();
        self.run_id_rest = Style::new().bright_black();
        self.timestamp = Style::new();
        self.duration = Style::new();
        self.size = Style::new();
        self.count = Style::new().bold();
        self.passed = Style::new().bold().green();
        self.failed = Style::new().bold().red();
        self.cancelled = Style::new().bold().yellow();
    }
}

/// Alignment information for displaying a list of runs.
///
/// This struct precomputes the maximum widths needed for aligned display of
/// run statistics. Use [`RunListAlignment::from_runs`] to create an instance
/// from a slice of runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunListAlignment {
    /// Maximum width of the "passed" count across all runs.
    pub passed_width: usize,
}

impl RunListAlignment {
    /// Creates alignment information from a slice of runs.
    ///
    /// Computes the maximum widths needed to align the statistics columns.
    pub fn from_runs(runs: &[RecordedRunInfo]) -> Self {
        let passed_width = runs
            .iter()
            .map(|run| run.status.passed_count_width())
            .max()
            .unwrap_or(1);

        Self { passed_width }
    }
}

/// A display wrapper for [`PruneResult`].
///
/// This wrapper implements [`fmt::Display`] to format the prune result as a
/// human-readable summary.
#[derive(Clone, Debug)]
pub struct DisplayPruneResult<'a> {
    pub(super) result: &'a PruneResult,
    pub(super) styles: &'a Styles,
}

impl fmt::Display for DisplayPruneResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let result = self.result;
        if result.deleted_count == 0 && result.orphans_deleted == 0 {
            if result.errors.is_empty() {
                writeln!(f, "no runs to prune")?;
            } else {
                writeln!(
                    f,
                    "no runs pruned ({} {} occurred)",
                    result.errors.len().style(self.styles.count),
                    plural::errors_str(result.errors.len()),
                )?;
            }
        } else {
            let orphan_suffix = if result.orphans_deleted > 0 {
                format!(
                    ", {} {}",
                    result.orphans_deleted.style(self.styles.count),
                    plural::orphans_str(result.orphans_deleted)
                )
            } else {
                String::new()
            };
            let error_suffix = if result.errors.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} {} occurred)",
                    result.errors.len().style(self.styles.count),
                    plural::errors_str(result.errors.len()),
                )
            };
            writeln!(
                f,
                "pruned {} {}{}, freed {}{}",
                result.deleted_count.style(self.styles.count),
                plural::runs_str(result.deleted_count),
                orphan_suffix,
                format_size_display(result.freed_bytes),
                error_suffix,
            )?;
        }

        // For explicit pruning, show error details as a bulleted list.
        if result.kind == PruneKind::Explicit && !result.errors.is_empty() {
            writeln!(f)?;
            writeln!(f, "errors:")?;
            for error in &result.errors {
                write!(f, "  - {error}")?;
                let mut curr = error.source();
                while let Some(source) = curr {
                    write!(f, ": {source}")?;
                    curr = source.source();
                }
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

/// A display wrapper for [`PrunePlan`].
///
/// This wrapper implements [`fmt::Display`] to format the prune plan as a
/// human-readable summary showing what would be deleted.
#[derive(Clone, Debug)]
pub struct DisplayPrunePlan<'a> {
    pub(super) plan: &'a PrunePlan,
    pub(super) run_id_index: &'a RunIdIndex,
    pub(super) styles: &'a Styles,
    pub(super) redactor: &'a Redactor,
}

impl fmt::Display for DisplayPrunePlan<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plan = self.plan;
        if plan.runs().is_empty() {
            writeln!(f, "no runs would be pruned")
        } else {
            writeln!(
                f,
                "would prune {} {}, freeing {}:\n",
                plan.runs().len().style(self.styles.count),
                plural::runs_str(plan.runs().len()),
                format_size_display(plan.total_bytes())
            )?;

            let alignment = RunListAlignment::from_runs(plan.runs());
            for run in plan.runs() {
                writeln!(
                    f,
                    "{}",
                    run.display(self.run_id_index, alignment, self.styles, self.redactor)
                )?;
            }
            Ok(())
        }
    }
}

/// A display wrapper for [`RecordedRunInfo`].
#[derive(Clone, Debug)]
pub struct DisplayRecordedRunInfo<'a> {
    run: &'a RecordedRunInfo,
    run_id_index: &'a RunIdIndex,
    alignment: RunListAlignment,
    styles: &'a Styles,
    redactor: &'a Redactor,
}

impl<'a> DisplayRecordedRunInfo<'a> {
    pub(super) fn new(
        run: &'a RecordedRunInfo,
        run_id_index: &'a RunIdIndex,
        alignment: RunListAlignment,
        styles: &'a Styles,
        redactor: &'a Redactor,
    ) -> Self {
        Self {
            run,
            run_id_index,
            alignment,
            styles,
            redactor,
        }
    }
}

impl fmt::Display for DisplayRecordedRunInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let run = self.run;

        // Get the shortest unique prefix for jj-style highlighting.
        let run_id_display =
            if let Some(prefix_info) = self.run_id_index.shortest_unique_prefix(run.run_id) {
                // Show the first 8 characters of the UUID with the unique
                // prefix highlighted.
                let full_short: String = run.run_id.to_string().chars().take(8).collect();
                let prefix_len = prefix_info.prefix.len().min(8);
                let (prefix_part, rest_part) = full_short.split_at(prefix_len);
                format!(
                    "{}{}",
                    prefix_part.style(self.styles.run_id_prefix),
                    rest_part.style(self.styles.run_id_rest),
                )
            } else {
                // Fallback if run ID not in index.
                let short_id: String = run.run_id.to_string().chars().take(8).collect();
                short_id.style(self.styles.run_id_rest).to_string()
            };

        let status_display = self.format_status();

        let size_kb = run.sizes.total_compressed() / 1024;

        let timestamp_display = self.redactor.redact_timestamp(&run.started_at);
        let duration_display = self.redactor.redact_store_duration(run.duration_secs);
        let size_display = self.redactor.redact_size_kb(size_kb);

        write!(
            f,
            "  {}  {}  {}  {:>6} KB  {}",
            run_id_display,
            timestamp_display.style(self.styles.timestamp),
            duration_display.style(self.styles.duration),
            size_display.style(self.styles.size),
            status_display,
        )
    }
}

impl DisplayRecordedRunInfo<'_> {
    /// Formats the status portion of the display.
    fn format_status(&self) -> String {
        match &self.run.status {
            RecordedRunStatus::Incomplete => {
                format!(
                    "{:>width$} {}",
                    "",
                    "incomplete".style(self.styles.cancelled),
                    width = self.alignment.passed_width,
                )
            }
            RecordedRunStatus::Unknown => {
                format!(
                    "{:>width$} {}",
                    "",
                    "unknown".style(self.styles.cancelled),
                    width = self.alignment.passed_width,
                )
            }
            RecordedRunStatus::Completed(stats) => self.format_normal_stats(stats, false),
            RecordedRunStatus::Cancelled(stats) => self.format_normal_stats(stats, true),
            RecordedRunStatus::StressCompleted(stats) => self.format_stress_stats(stats, false),
            RecordedRunStatus::StressCancelled(stats) => self.format_stress_stats(stats, true),
        }
    }

    /// Formats statistics for a normal test run.
    fn format_normal_stats(&self, stats: &CompletedRunStats, cancelled: bool) -> String {
        // When no tests are run, show "passed" in yellow since it'll result in
        // a failure most of the time.
        if stats.initial_run_count == 0 {
            return format!(
                "{:>width$} {}",
                0.style(self.styles.count),
                "passed".style(self.styles.cancelled),
                width = self.alignment.passed_width,
            );
        }

        let mut result = String::new();

        // Right-align the passed count based on max width, then "passed".
        swrite!(
            result,
            "{:>width$} {}",
            stats.passed.style(self.styles.count),
            "passed".style(self.styles.passed),
            width = self.alignment.passed_width,
        );

        if stats.failed > 0 {
            swrite!(
                result,
                " / {} {}",
                stats.failed.style(self.styles.count),
                "failed".style(self.styles.failed),
            );
        }

        // Calculate tests that were not run (neither passed nor failed).
        let not_run = stats
            .initial_run_count
            .saturating_sub(stats.passed)
            .saturating_sub(stats.failed);
        if not_run > 0 {
            swrite!(
                result,
                " / {} {}",
                not_run.style(self.styles.count),
                "not run".style(self.styles.cancelled),
            );
        }

        if cancelled {
            swrite!(result, " {}", "(cancelled)".style(self.styles.cancelled));
        }

        result
    }

    /// Formats statistics for a stress test run.
    fn format_stress_stats(&self, stats: &StressCompletedRunStats, cancelled: bool) -> String {
        let mut result = String::new();

        // Right-align the passed count based on max width, then "passed iterations".
        swrite!(
            result,
            "{:>width$} {} {}",
            stats.success_count.style(self.styles.count),
            "passed".style(self.styles.passed),
            plural::iterations_str(stats.success_count),
            width = self.alignment.passed_width,
        );

        if stats.failed_count > 0 {
            swrite!(
                result,
                " / {} {}",
                stats.failed_count.style(self.styles.count),
                "failed".style(self.styles.failed),
            );
        }

        // Calculate iterations that were not run (neither passed nor failed).
        // Only shown when initial_iteration_count is known.
        if let Some(initial) = stats.initial_iteration_count {
            let not_run = initial
                .get()
                .saturating_sub(stats.success_count)
                .saturating_sub(stats.failed_count);
            if not_run > 0 {
                swrite!(
                    result,
                    " / {} {}",
                    not_run.style(self.styles.count),
                    "not run".style(self.styles.cancelled),
                );
            }
        }

        if cancelled {
            swrite!(result, " {}", "(cancelled)".style(self.styles.cancelled));
        }

        result
    }
}

/// A display wrapper for a list of recorded runs.
///
/// This struct handles the full table display including:
/// - Optional store path header (when verbose)
/// - Run count header
/// - Individual run rows
/// - Total size footer with separator
pub struct DisplayRunList<'a> {
    snapshot: &'a RunStoreSnapshot,
    store_path: Option<&'a Utf8Path>,
    styles: &'a Styles,
    theme_characters: &'a ThemeCharacters,
    redactor: &'a Redactor,
}

impl<'a> DisplayRunList<'a> {
    /// Creates a new display wrapper for a run list.
    ///
    /// If `store_path` is provided, it will be displayed at the top of the output.
    ///
    /// If `redactor` is provided, timestamps, durations, and sizes will be
    /// redacted for snapshot testing while preserving column alignment.
    pub fn new(
        snapshot: &'a RunStoreSnapshot,
        store_path: Option<&'a Utf8Path>,
        styles: &'a Styles,
        theme_characters: &'a ThemeCharacters,
        redactor: &'a Redactor,
    ) -> Self {
        Self {
            snapshot,
            store_path,
            styles,
            theme_characters,
            redactor,
        }
    }
}

impl fmt::Display for DisplayRunList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Show store path if provided.
        if let Some(path) = self.store_path {
            writeln!(f, "{}: {}\n", "store".style(self.styles.count), path)?;
        }

        if self.snapshot.run_count() == 0 {
            // No runs to display; the caller should handle the "no recorded runs" message
            // via logging or other means.
            return Ok(());
        }

        writeln!(
            f,
            "{} recorded {}:\n",
            self.snapshot.run_count().style(self.styles.count),
            plural::runs_str(self.snapshot.run_count()),
        )?;

        // Compute the latest (most recent replayable) run ID for marking.
        let latest_run_id = self.snapshot.most_recent_run().ok().map(|r| r.run_id);

        let alignment = RunListAlignment::from_runs(self.snapshot.runs());
        for run in self.snapshot.runs() {
            let display = run.display(
                self.snapshot.run_id_index(),
                alignment,
                self.styles,
                self.redactor,
            );
            if Some(run.run_id) == latest_run_id {
                writeln!(f, "{}  *{}", display, "latest".style(self.styles.count))?;
            } else {
                writeln!(f, "{}", display)?;
            }
        }

        // Display total size at the bottom.
        // Column positions: 2 (indent) + 8 (run_id) + 2 + 19 (timestamp)
        // + 2 + 10 (duration) + 2 = 45 chars before the size column.
        let total_size_kb = self.snapshot.total_size() / 1024;
        writeln!(
            f,
            "                                             {}",
            self.theme_characters.hbar(6),
        )?;

        let size_display = self.redactor.redact_size_kb(total_size_kb);
        writeln!(
            f,
            "                                             {:>6} KB",
            size_display.style(self.styles.size),
        )?;

        Ok(())
    }
}
