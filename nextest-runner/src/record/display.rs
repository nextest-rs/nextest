// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display wrappers for record store types.
//!
//! This module provides formatting and display utilities for recorded run
//! information, prune operations, and run lists.

use super::{
    PruneKind, PrunePlan, PruneResult, RecordedRunInfo, RecordedRunStatus,
    SnapshotWithReplayability,
    run_id_index::RunIdIndex,
    store::{CompletedRunStats, ReplayabilityStatus, StressCompletedRunStats},
    tree::{RunInfo, RunTree, TreeIterItem},
};
use crate::{
    helpers::{ThemeCharacters, plural},
    redact::{Redactor, SizeDisplay},
};
use camino::Utf8Path;
use chrono::{DateTime, Utc};
use owo_colors::{OwoColorize, Style};
use quick_junit::ReportUuid;
use std::{collections::HashMap, error::Error, fmt};
use swrite::{SWrite, swrite};

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
    /// Style for field labels in detailed view.
    pub label: Style,
    /// Style for section headers in detailed view.
    pub section: Style,
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
        self.label = Style::new().bold();
        self.section = Style::new().bold();
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
    /// Maximum width of the size column across all runs.
    ///
    /// This is dynamically computed based on the formatted size display width
    /// (e.g., "123 KB" or "1.5 MB"). The minimum width is 6 to maintain
    /// alignment for typical sizes.
    pub size_width: usize,
    /// Maximum tree prefix width across all nodes, in units of 2-char segments.
    ///
    /// Used to align columns when displaying runs in a tree structure. Nodes
    /// with smaller prefix widths get additional padding to align the timestamp
    /// column.
    pub tree_prefix_width: usize,
}

impl RunListAlignment {
    /// Minimum width for the size column to maintain visual consistency.
    const MIN_SIZE_WIDTH: usize = 9;

    /// Creates alignment information from a slice of runs.
    ///
    /// Computes the maximum widths needed to align the statistics columns.
    pub fn from_runs(runs: &[RecordedRunInfo]) -> Self {
        let passed_width = runs
            .iter()
            .map(|run| run.status.passed_count_width())
            .max()
            .unwrap_or(1);

        let size_width = runs
            .iter()
            .map(|run| SizeDisplay(run.sizes.total_compressed()).display_width())
            .max()
            .unwrap_or(Self::MIN_SIZE_WIDTH)
            .max(Self::MIN_SIZE_WIDTH);

        Self {
            passed_width,
            size_width,
            tree_prefix_width: 0,
        }
    }

    /// Creates alignment information from a slice of runs, also considering
    /// the total size for the size column width.
    ///
    /// This should be used when displaying a run list with a total, to ensure
    /// the total size aligns properly with individual run sizes.
    pub fn from_runs_with_total(runs: &[RecordedRunInfo], total_size_bytes: u64) -> Self {
        let passed_width = runs
            .iter()
            .map(|run| run.status.passed_count_width())
            .max()
            .unwrap_or(1);

        let max_run_size_width = runs
            .iter()
            .map(|run| SizeDisplay(run.sizes.total_compressed()).display_width())
            .max()
            .unwrap_or(0);

        let total_size_width = SizeDisplay(total_size_bytes).display_width();

        let size_width = max_run_size_width
            .max(total_size_width)
            .max(Self::MIN_SIZE_WIDTH);

        Self {
            passed_width,
            size_width,
            tree_prefix_width: 0,
        }
    }

    /// Sets the tree prefix width from a [`RunTree`].
    ///
    /// Used when displaying runs in a tree structure to ensure proper column
    /// alignment across nodes with different tree depths.
    pub(super) fn with_tree(mut self, tree: &RunTree) -> Self {
        self.tree_prefix_width = tree
            .iter()
            .map(|item| item.tree_prefix_width())
            .max()
            .unwrap_or(0);
        self
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
                SizeDisplay(result.freed_bytes),
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
                self.redactor.redact_size(plan.total_bytes())
            )?;

            let alignment = RunListAlignment::from_runs(plan.runs());
            // For prune display, we don't show replayability status.
            let replayable = ReplayabilityStatus::Replayable;
            for run in plan.runs() {
                writeln!(
                    f,
                    "{}",
                    run.display(
                        self.run_id_index,
                        &replayable,
                        alignment,
                        self.styles,
                        self.redactor
                    )
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
    replayability: &'a ReplayabilityStatus,
    alignment: RunListAlignment,
    styles: &'a Styles,
    redactor: &'a Redactor,
    /// Prefix to display before the run ID. Defaults to "  " (base indent).
    prefix: &'a str,
    /// Padding to add after the run ID to align columns. Defaults to 0.
    run_id_padding: usize,
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

        let timestamp_display = self.redactor.redact_timestamp(&run.started_at);
        let duration_display = self.redactor.redact_store_duration(run.duration_secs);
        let size_display = self.redactor.redact_size(run.sizes.total_compressed());

        write!(
            f,
            "{}{}{:padding$}  {}  {}  {:>width$}  {}",
            self.prefix,
            run_id_display,
            "",
            timestamp_display.style(self.styles.timestamp),
            duration_display.style(self.styles.duration),
            size_display.style(self.styles.size),
            status_display,
            padding = self.run_id_padding,
            width = self.alignment.size_width,
        )?;

        // Show replayability status if not replayable.
        match self.replayability {
            ReplayabilityStatus::Replayable => {}
            ReplayabilityStatus::NotReplayable(_) => {
                write!(f, "  ({})", "not replayable".style(self.styles.failed))?;
            }
            ReplayabilityStatus::Incomplete => {
                // Don't show "incomplete" here because we already show that in
                // the status column.
            }
        }

        Ok(())
    }
}

impl<'a> DisplayRecordedRunInfo<'a> {
    pub(super) fn new(
        run: &'a RecordedRunInfo,
        run_id_index: &'a RunIdIndex,
        replayability: &'a ReplayabilityStatus,
        alignment: RunListAlignment,
        styles: &'a Styles,
        redactor: &'a Redactor,
    ) -> Self {
        Self {
            run,
            run_id_index,
            replayability,
            alignment,
            styles,
            redactor,
            prefix: "  ",
            run_id_padding: 0,
        }
    }

    /// Sets the prefix and run ID padding for tree display.
    ///
    /// Used by [`DisplayRunList`] for tree-formatted output.
    pub(super) fn with_tree_formatting(mut self, prefix: &'a str, run_id_padding: usize) -> Self {
        self.prefix = prefix;
        self.run_id_padding = run_id_padding;
        self
    }

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

/// A detailed display wrapper for [`RecordedRunInfo`].
///
/// Unlike [`DisplayRecordedRunInfo`] which produces a compact table row,
/// this produces a multi-line detailed view with labeled fields.
pub struct DisplayRecordedRunInfoDetailed<'a> {
    run: &'a RecordedRunInfo,
    run_id_index: &'a RunIdIndex,
    replayability: &'a ReplayabilityStatus,
    now: DateTime<Utc>,
    styles: &'a Styles,
    theme_characters: &'a ThemeCharacters,
    redactor: &'a Redactor,
}

impl<'a> DisplayRecordedRunInfoDetailed<'a> {
    pub(super) fn new(
        run: &'a RecordedRunInfo,
        run_id_index: &'a RunIdIndex,
        replayability: &'a ReplayabilityStatus,
        now: DateTime<Utc>,
        styles: &'a Styles,
        theme_characters: &'a ThemeCharacters,
        redactor: &'a Redactor,
    ) -> Self {
        Self {
            run,
            run_id_index,
            replayability,
            now,
            styles,
            theme_characters,
            redactor,
        }
    }

    /// Formats the run ID header with jj-style prefix highlighting.
    fn format_run_id(&self) -> String {
        self.format_run_id_with_prefix(self.run.run_id)
    }

    /// Formats a run ID with jj-style prefix highlighting.
    fn format_run_id_with_prefix(&self, run_id: ReportUuid) -> String {
        let run_id_str = run_id.to_string();
        if let Some(prefix_info) = self.run_id_index.shortest_unique_prefix(run_id) {
            let prefix_len = prefix_info.prefix.len().min(run_id_str.len());
            let (prefix_part, rest_part) = run_id_str.split_at(prefix_len);
            format!(
                "{}{}",
                prefix_part.style(self.styles.run_id_prefix),
                rest_part.style(self.styles.run_id_rest),
            )
        } else {
            run_id_str.style(self.styles.run_id_rest).to_string()
        }
    }

    /// Writes a labeled field.
    fn write_field(
        &self,
        f: &mut fmt::Formatter<'_>,
        label: &str,
        value: impl fmt::Display,
    ) -> fmt::Result {
        writeln!(
            f,
            "  {:18}{}",
            format!("{}:", label).style(self.styles.label),
            value,
        )
    }

    /// Writes the status field with exit code for completed runs.
    fn write_status_field(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status_str = self.run.status.short_status_str();
        let exit_code = self.run.status.exit_code();

        match exit_code {
            Some(code) => {
                let exit_code_style = if code == 0 {
                    self.styles.passed
                } else {
                    self.styles.failed
                };
                self.write_field(
                    f,
                    "status",
                    format!("{} (exit code {})", status_str, code.style(exit_code_style)),
                )
            }
            None => self.write_field(f, "status", status_str),
        }
    }

    /// Writes the replayable field with yes/no/maybe styling and reasons.
    fn write_replayable(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.replayability {
            ReplayabilityStatus::Replayable => {
                self.write_field(f, "replayable", "yes".style(self.styles.passed))
            }
            ReplayabilityStatus::NotReplayable(reasons) => {
                let mut reasons_str = String::new();
                for reason in reasons {
                    if !reasons_str.is_empty() {
                        swrite!(reasons_str, ", {reason}");
                    } else {
                        swrite!(reasons_str, "{reason}");
                    }
                }
                self.write_field(
                    f,
                    "replayable",
                    format!("{}: {}", "no".style(self.styles.failed), reasons_str),
                )
            }
            ReplayabilityStatus::Incomplete => self.write_field(
                f,
                "replayable",
                format!(
                    "{}: run is incomplete (archive may be partial)",
                    "maybe".style(self.styles.cancelled)
                ),
            ),
        }
    }

    /// Writes the stats section (tests or iterations).
    fn write_stats_section(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.run.status {
            RecordedRunStatus::Incomplete | RecordedRunStatus::Unknown => {
                // No stats to show.
                Ok(())
            }
            RecordedRunStatus::Completed(stats) | RecordedRunStatus::Cancelled(stats) => {
                writeln!(f, "  {}:", "tests".style(self.styles.section))?;
                writeln!(
                    f,
                    "    {:16}{}",
                    "passed:".style(self.styles.label),
                    stats.passed.style(self.styles.passed),
                )?;
                if stats.failed > 0 {
                    writeln!(
                        f,
                        "    {:16}{}",
                        "failed:".style(self.styles.label),
                        stats.failed.style(self.styles.failed),
                    )?;
                }
                let not_run = stats
                    .initial_run_count
                    .saturating_sub(stats.passed)
                    .saturating_sub(stats.failed);
                if not_run > 0 {
                    writeln!(
                        f,
                        "    {:16}{}",
                        "not run:".style(self.styles.label),
                        not_run.style(self.styles.cancelled),
                    )?;
                }
                writeln!(f)
            }
            RecordedRunStatus::StressCompleted(stats)
            | RecordedRunStatus::StressCancelled(stats) => {
                writeln!(f, "  {}:", "iterations".style(self.styles.section))?;
                writeln!(
                    f,
                    "    {:16}{}",
                    "passed:".style(self.styles.label),
                    stats.success_count.style(self.styles.passed),
                )?;
                if stats.failed_count > 0 {
                    writeln!(
                        f,
                        "    {:16}{}",
                        "failed:".style(self.styles.label),
                        stats.failed_count.style(self.styles.failed),
                    )?;
                }
                if let Some(initial) = stats.initial_iteration_count {
                    let not_run = initial
                        .get()
                        .saturating_sub(stats.success_count)
                        .saturating_sub(stats.failed_count);
                    if not_run > 0 {
                        writeln!(
                            f,
                            "    {:16}{}",
                            "not run:".style(self.styles.label),
                            not_run.style(self.styles.cancelled),
                        )?;
                    }
                }
                writeln!(f)
            }
        }
    }

    /// Writes the sizes section with compressed/uncompressed breakdown.
    fn write_sizes_section(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Put "sizes:" on the same line as the column headers, using the same
        // format as data rows for alignment.
        writeln!(
            f,
            "  {:18}{:>10}  {:>12}  {:>7}",
            "sizes:".style(self.styles.section),
            "compressed".style(self.styles.label),
            "uncompressed".style(self.styles.label),
            "entries".style(self.styles.label),
        )?;

        let sizes = &self.run.sizes;

        writeln!(
            f,
            "    {:16}{:>10}  {:>12}  {:>7}",
            "log".style(self.styles.label),
            self.redactor
                .redact_size(sizes.log.compressed)
                .style(self.styles.size),
            self.redactor
                .redact_size(sizes.log.uncompressed)
                .style(self.styles.size),
            sizes.log.entries.style(self.styles.size),
        )?;

        writeln!(
            f,
            "    {:16}{:>10}  {:>12}  {:>7}",
            "store".style(self.styles.label),
            self.redactor
                .redact_size(sizes.store.compressed)
                .style(self.styles.size),
            self.redactor
                .redact_size(sizes.store.uncompressed)
                .style(self.styles.size),
            sizes.store.entries.style(self.styles.size),
        )?;

        // Draw a horizontal line before "total".
        writeln!(
            f,
            "    {:16}{}  {}  {}",
            "",
            self.theme_characters.hbar(10),
            self.theme_characters.hbar(12),
            self.theme_characters.hbar(7),
        )?;

        writeln!(
            f,
            "    {:16}{:>10}  {:>12}  {:>7}",
            "total".style(self.styles.section),
            self.redactor
                .redact_size(sizes.total_compressed())
                .style(self.styles.size),
            self.redactor
                .redact_size(sizes.total_uncompressed())
                .style(self.styles.size),
            // Format total entries similar to KB and MB sizes.
            sizes.total_entries().style(self.styles.size),
        )
    }

    /// Formats env vars with redaction.
    fn format_env_vars(&self) -> String {
        self.redactor.redact_env_vars(&self.run.env_vars)
    }

    /// Formats CLI args with redaction.
    fn format_cli_args(&self) -> String {
        self.redactor.redact_cli_args(&self.run.cli_args)
    }
}

impl fmt::Display for DisplayRecordedRunInfoDetailed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let run = self.run;

        // Header with full run ID.
        writeln!(
            f,
            "{} {}",
            "Run".style(self.styles.section),
            self.format_run_id()
        )?;
        writeln!(f)?;

        // Basic info fields.
        self.write_field(
            f,
            "nextest version",
            self.redactor.redact_version(&run.nextest_version),
        )?;

        // Parent run ID (if this is a rerun).
        if let Some(parent_run_id) = run.parent_run_id {
            self.write_field(
                f,
                "parent run",
                self.format_run_id_with_prefix(parent_run_id),
            )?;
        }

        // CLI args (if present).
        if !run.cli_args.is_empty() {
            self.write_field(f, "command", self.format_cli_args())?;
        }

        // Environment variables (if present).
        if !run.env_vars.is_empty() {
            self.write_field(f, "env", self.format_env_vars())?;
        }

        self.write_status_field(f)?;
        // Compute and display started at with relative duration.
        let timestamp = self.redactor.redact_detailed_timestamp(&run.started_at);
        let relative_duration = self
            .now
            .signed_duration_since(run.started_at.with_timezone(&Utc))
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        let relative_display = self.redactor.redact_relative_duration(relative_duration);
        self.write_field(
            f,
            "started at",
            format!(
                "{} ({} ago)",
                timestamp,
                relative_display.style(self.styles.count)
            ),
        )?;
        // Compute and display last written at with relative duration.
        let last_written_timestamp = self
            .redactor
            .redact_detailed_timestamp(&run.last_written_at);
        let last_written_relative_duration = self
            .now
            .signed_duration_since(run.last_written_at.with_timezone(&Utc))
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        let last_written_relative_display = self
            .redactor
            .redact_relative_duration(last_written_relative_duration);
        self.write_field(
            f,
            "last written at",
            format!(
                "{} ({} ago)",
                last_written_timestamp,
                last_written_relative_display.style(self.styles.count)
            ),
        )?;
        self.write_field(
            f,
            "duration",
            self.redactor.redact_detailed_duration(run.duration_secs),
        )?;

        self.write_replayable(f)?;
        writeln!(f)?;

        // Stats section (tests or iterations).
        self.write_stats_section(f)?;

        // Sizes section.
        self.write_sizes_section(f)?;

        Ok(())
    }
}

/// A display wrapper for a list of recorded runs.
///
/// This struct handles the full table display including:
/// - Optional store path header (when verbose)
/// - Run count header
/// - Individual run rows with replayability status
/// - Total size footer with separator
pub struct DisplayRunList<'a> {
    snapshot_with_replayability: &'a SnapshotWithReplayability<'a>,
    store_path: Option<&'a Utf8Path>,
    styles: &'a Styles,
    theme_characters: &'a ThemeCharacters,
    redactor: &'a Redactor,
}

impl<'a> DisplayRunList<'a> {
    /// Creates a new display wrapper for a run list.
    ///
    /// The `snapshot_with_replayability` provides the runs and their precomputed
    /// replayability status. Non-replayable runs will show a suffix with the reason.
    /// The most recent run by start time will be marked with `*latest`.
    ///
    /// If `store_path` is provided, it will be displayed at the top of the output.
    ///
    /// If `redactor` is provided, timestamps, durations, and sizes will be
    /// redacted for snapshot testing while preserving column alignment.
    pub fn new(
        snapshot_with_replayability: &'a SnapshotWithReplayability<'a>,
        store_path: Option<&'a Utf8Path>,
        styles: &'a Styles,
        theme_characters: &'a ThemeCharacters,
        redactor: &'a Redactor,
    ) -> Self {
        Self {
            snapshot_with_replayability,
            store_path,
            styles,
            theme_characters,
            redactor,
        }
    }
}

impl fmt::Display for DisplayRunList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.snapshot_with_replayability.snapshot();

        // Show store path if provided.
        if let Some(path) = self.store_path {
            writeln!(f, "{}: {}\n", "store".style(self.styles.count), path)?;
        }

        if snapshot.run_count() == 0 {
            // No runs to display; the caller should handle the "no recorded runs" message
            // via logging or other means.
            return Ok(());
        }

        writeln!(
            f,
            "{} recorded {}:\n",
            snapshot.run_count().style(self.styles.count),
            plural::runs_str(snapshot.run_count()),
        )?;

        let tree = RunTree::build(
            &snapshot
                .runs()
                .iter()
                .map(|run| RunInfo {
                    run_id: run.run_id,
                    parent_run_id: run.parent_run_id,
                    started_at: run.started_at,
                })
                .collect::<Vec<_>>(),
        );

        let alignment =
            RunListAlignment::from_runs_with_total(snapshot.runs(), snapshot.total_size())
                .with_tree(&tree);
        let latest_run_id = self.snapshot_with_replayability.latest_run_id();

        let run_map: HashMap<_, _> = snapshot.runs().iter().map(|r| (r.run_id, r)).collect();

        for item in tree.iter() {
            let prefix = format_tree_prefix(self.theme_characters, item);

            // Nodes with smaller tree_prefix_width need more padding to align the timestamp column.
            let run_id_padding = (alignment.tree_prefix_width - item.tree_prefix_width()) * 2;

            match item.run_id {
                Some(run_id) => {
                    let run = run_map
                        .get(&run_id)
                        .expect("run ID from tree should exist in snapshot");
                    let replayability = self
                        .snapshot_with_replayability
                        .get_replayability(run.run_id);

                    let display = DisplayRecordedRunInfo::new(
                        run,
                        snapshot.run_id_index(),
                        replayability,
                        alignment,
                        self.styles,
                        self.redactor,
                    )
                    .with_tree_formatting(&prefix, run_id_padding);

                    if Some(run.run_id) == latest_run_id {
                        writeln!(f, "{}  *{}", display, "latest".style(self.styles.count))?;
                    } else {
                        writeln!(f, "{}", display)?;
                    }
                }
                None => {
                    // "???" is 3 chars, run_id is 8 chars, so we need 5 extra padding.
                    let virtual_padding = run_id_padding + 5;
                    writeln!(
                        f,
                        "{}{}{:padding$}  {}",
                        prefix,
                        "???".style(self.styles.run_id_rest),
                        "",
                        "(pruned parent)".style(self.styles.cancelled),
                        padding = virtual_padding,
                    )?;
                }
            }
        }

        // Column positions: base_indent (2) + tree_prefix_width * 2 (tree chars) + run_id (8)
        // + separator (2) + timestamp (19) + separator (2) + duration (10) + separator (2).
        let first_col_width = 2 + alignment.tree_prefix_width * 2 + 8;
        let total_line_spacing = first_col_width + 2 + 19 + 2 + 10 + 2;

        writeln!(
            f,
            "{:spacing$}{}",
            "",
            self.theme_characters.hbar(alignment.size_width),
            spacing = total_line_spacing,
        )?;

        let size_display = self.redactor.redact_size(snapshot.total_size());
        let size_formatted = format!("{:>width$}", size_display, width = alignment.size_width);
        writeln!(
            f,
            "{:spacing$}{}",
            "",
            size_formatted.style(self.styles.size),
            spacing = total_line_spacing,
        )?;

        Ok(())
    }
}

/// Formats the tree prefix for a given tree item.
///
/// The prefix includes:
/// - Base indent (2 spaces)
/// - Continuation lines for ancestor levels (`│ ` or `  `)
/// - Branch character (`├─` or `└─`) unless is_only_child
fn format_tree_prefix(theme_characters: &ThemeCharacters, item: &TreeIterItem) -> String {
    let mut prefix = String::new();

    // Base indent (2 spaces, matching original format).
    prefix.push_str("  ");

    if item.depth == 0 {
        return prefix;
    }

    for &has_continuation in &item.continuation_flags {
        if has_continuation {
            prefix.push_str(theme_characters.tree_continuation());
        } else {
            prefix.push_str(theme_characters.tree_space());
        }
    }

    if !item.is_only_child {
        if item.is_last {
            prefix.push_str(theme_characters.tree_last());
        } else {
            prefix.push_str(theme_characters.tree_branch());
        }
    }

    prefix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::RecordPruneError,
        helpers::ThemeCharacters,
        record::{
            CompletedRunStats, ComponentSizes, NonReplayableReason, PruneKind, PrunePlan,
            PruneResult, RecordedRunStatus, RecordedSizes, RunStoreSnapshot,
            SnapshotWithReplayability, StressCompletedRunStats, format::RECORD_FORMAT_VERSION,
            run_id_index::RunIdIndex,
        },
        redact::Redactor,
    };
    use chrono::{DateTime, Utc};
    use semver::Version;
    use std::{collections::BTreeMap, num::NonZero};

    /// Returns a fixed "now" time for testing relative duration display.
    ///
    /// This time is 30 seconds after the latest test timestamp used in the tests,
    /// which is "2024-06-25T13:00:00+00:00".
    fn test_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2024-06-25T13:00:30+00:00")
            .expect("valid datetime")
            .with_timezone(&Utc)
    }

    /// Creates a `RecordedRunInfo` for testing display functions.
    fn make_run_info(
        uuid: &str,
        version: &str,
        started_at: &str,
        total_compressed_size: u64,
        status: RecordedRunStatus,
    ) -> RecordedRunInfo {
        make_run_info_with_duration(
            uuid,
            version,
            started_at,
            total_compressed_size,
            1.0,
            status,
        )
    }

    /// Creates a `RecordedRunInfo` with a custom duration for testing.
    fn make_run_info_with_duration(
        uuid: &str,
        version: &str,
        started_at: &str,
        total_compressed_size: u64,
        duration_secs: f64,
        status: RecordedRunStatus,
    ) -> RecordedRunInfo {
        let started_at = DateTime::parse_from_rfc3339(started_at).expect("valid datetime");
        // For simplicity in tests, put all size in the store component.
        RecordedRunInfo {
            run_id: uuid.parse().expect("valid UUID"),
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version: Version::parse(version).expect("valid version"),
            started_at,
            last_written_at: started_at,
            duration_secs: Some(duration_secs),
            cli_args: Vec::new(),
            build_scope_args: Vec::new(),
            env_vars: BTreeMap::new(),
            parent_run_id: None,
            sizes: RecordedSizes {
                log: ComponentSizes::default(),
                store: ComponentSizes {
                    compressed: total_compressed_size,
                    uncompressed: total_compressed_size * 3,
                    entries: 0,
                },
            },
            status,
        }
    }

    /// Creates a `RecordedRunInfo` for testing with cli_args and env_vars.
    fn make_run_info_with_cli_env(
        uuid: &str,
        version: &str,
        started_at: &str,
        cli_args: Vec<String>,
        env_vars: BTreeMap<String, String>,
        status: RecordedRunStatus,
    ) -> RecordedRunInfo {
        make_run_info_with_parent(uuid, version, started_at, cli_args, env_vars, None, status)
    }

    /// Creates a `RecordedRunInfo` for testing with parent_run_id support.
    fn make_run_info_with_parent(
        uuid: &str,
        version: &str,
        started_at: &str,
        cli_args: Vec<String>,
        env_vars: BTreeMap<String, String>,
        parent_run_id: Option<&str>,
        status: RecordedRunStatus,
    ) -> RecordedRunInfo {
        let started_at = DateTime::parse_from_rfc3339(started_at).expect("valid datetime");
        RecordedRunInfo {
            run_id: uuid.parse().expect("valid UUID"),
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version: Version::parse(version).expect("valid version"),
            started_at,
            last_written_at: started_at,
            duration_secs: Some(12.345),
            cli_args,
            build_scope_args: Vec::new(),
            env_vars,
            parent_run_id: parent_run_id.map(|s| s.parse().expect("valid UUID")),
            sizes: RecordedSizes {
                log: ComponentSizes {
                    compressed: 1024,
                    uncompressed: 4096,
                    entries: 100,
                },
                store: ComponentSizes {
                    compressed: 51200,
                    uncompressed: 204800,
                    entries: 42,
                },
            },
            status,
        }
    }

    #[test]
    fn test_display_prune_result_nothing_to_prune() {
        let result = PruneResult::default();
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"no runs to prune");
    }

    #[test]
    fn test_display_prune_result_nothing_pruned_with_error() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 0,
            orphans_deleted: 0,
            freed_bytes: 0,
            errors: vec![RecordPruneError::DeleteOrphan {
                path: "/some/path".into(),
                error: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            }],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"no runs pruned (1 error occurred)");
    }

    #[test]
    fn test_display_prune_result_single_run() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 1,
            orphans_deleted: 0,
            freed_bytes: 1024,
            errors: vec![],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 1 run, freed 1 KB");
    }

    #[test]
    fn test_display_prune_result_multiple_runs() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 3,
            orphans_deleted: 0,
            freed_bytes: 5 * 1024 * 1024,
            errors: vec![],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 3 runs, freed 5.0 MB");
    }

    #[test]
    fn test_display_prune_result_with_orphan() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 2,
            orphans_deleted: 1,
            freed_bytes: 3 * 1024 * 1024,
            errors: vec![],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 2 runs, 1 orphan, freed 3.0 MB");
    }

    #[test]
    fn test_display_prune_result_with_multiple_orphans() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 1,
            orphans_deleted: 3,
            freed_bytes: 2 * 1024 * 1024,
            errors: vec![],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 1 run, 3 orphans, freed 2.0 MB");
    }

    #[test]
    fn test_display_prune_result_with_errors_implicit() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 2,
            orphans_deleted: 0,
            freed_bytes: 1024 * 1024,
            errors: vec![
                RecordPruneError::DeleteOrphan {
                    path: "/path1".into(),
                    error: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                },
                RecordPruneError::DeleteOrphan {
                    path: "/path2".into(),
                    error: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
                },
            ],
        };
        // Implicit pruning shows summary only, no error details.
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 2 runs, freed 1.0 MB (2 errors occurred)");
    }

    #[test]
    fn test_display_prune_result_with_errors_explicit() {
        let result = PruneResult {
            kind: PruneKind::Explicit,
            deleted_count: 2,
            orphans_deleted: 0,
            freed_bytes: 1024 * 1024,
            errors: vec![
                RecordPruneError::DeleteOrphan {
                    path: "/path1".into(),
                    error: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                },
                RecordPruneError::DeleteOrphan {
                    path: "/path2".into(),
                    error: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
                },
            ],
        };
        // Explicit pruning shows error details as a bulleted list.
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"
        pruned 2 runs, freed 1.0 MB (2 errors occurred)

        errors:
          - error deleting orphaned directory `/path1`: denied
          - error deleting orphaned directory `/path2`: not found
        ");
    }

    #[test]
    fn test_display_prune_result_full() {
        let result = PruneResult {
            kind: PruneKind::Implicit,
            deleted_count: 5,
            orphans_deleted: 2,
            freed_bytes: 10 * 1024 * 1024,
            errors: vec![RecordPruneError::DeleteOrphan {
                path: "/orphan".into(),
                error: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            }],
        };
        insta::assert_snapshot!(result.display(&Styles::default()).to_string(), @"pruned 5 runs, 2 orphans, freed 10.0 MB (1 error occurred)");
    }

    #[test]
    fn test_display_recorded_run_info_completed() {
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "0.9.100",
            "2024-06-15T10:30:00+00:00",
            102400,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 100,
                passed: 95,
                failed: 5,
                exit_code: 100,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-15 10:30:00      1.000s     100 KB  95 passed / 5 failed"
        );
    }

    #[test]
    fn test_display_recorded_run_info_incomplete() {
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440001",
            "0.9.101",
            "2024-06-16T11:00:00+00:00",
            51200,
            RecordedRunStatus::Incomplete,
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Incomplete, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-16 11:00:00      1.000s      50 KB   incomplete"
        );
    }

    #[test]
    fn test_display_recorded_run_info_not_run() {
        // Test case where some tests are not run (neither passed nor failed).
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440005",
            "0.9.105",
            "2024-06-20T15:00:00+00:00",
            75000,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 17,
                passed: 10,
                failed: 6,
                exit_code: 100,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-20 15:00:00      1.000s      73 KB  10 passed / 6 failed / 1 not run"
        );
    }

    #[test]
    fn test_display_recorded_run_info_no_tests() {
        // Test case where no tests were expected to run.
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-44665544000c",
            "0.9.112",
            "2024-06-23T16:00:00+00:00",
            5000,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 0,
                passed: 0,
                failed: 0,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-23 16:00:00      1.000s       4 KB  0 passed"
        );
    }

    #[test]
    fn test_display_recorded_run_info_stress_completed() {
        // Test StressCompleted with all iterations passing.
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440010",
            "0.9.120",
            "2024-06-25T10:00:00+00:00",
            150000,
            RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 100,
                failed_count: 0,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-25 10:00:00      1.000s     146 KB  100 passed iterations"
        );

        // Test StressCompleted with some failures.
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440011",
            "0.9.120",
            "2024-06-25T11:00:00+00:00",
            150000,
            RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 95,
                failed_count: 5,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-25 11:00:00      1.000s     146 KB  95 passed iterations / 5 failed"
        );
    }

    #[test]
    fn test_display_recorded_run_info_stress_cancelled() {
        // Test StressCancelled with some iterations not run.
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440012",
            "0.9.120",
            "2024-06-25T12:00:00+00:00",
            100000,
            RecordedRunStatus::StressCancelled(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 50,
                failed_count: 10,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-25 12:00:00      1.000s      97 KB  50 passed iterations / 10 failed / 40 not run (cancelled)"
        );

        // Test StressCancelled without initial_iteration_count (not run count unknown).
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440013",
            "0.9.120",
            "2024-06-25T13:00:00+00:00",
            100000,
            RecordedRunStatus::StressCancelled(StressCompletedRunStats {
                initial_iteration_count: None,
                success_count: 50,
                failed_count: 10,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let alignment = RunListAlignment::from_runs(runs);
        insta::assert_snapshot!(
            run.display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-25 13:00:00      1.000s      97 KB  50 passed iterations / 10 failed (cancelled)"
        );
    }

    #[test]
    fn test_display_alignment_multiple_runs() {
        // Test that alignment works correctly when runs have different passed counts.
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440006",
                "0.9.106",
                "2024-06-21T10:00:00+00:00",
                100000,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 559,
                    passed: 559,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440007",
                "0.9.107",
                "2024-06-21T11:00:00+00:00",
                50000,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 51,
                    passed: 51,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440008",
                "0.9.108",
                "2024-06-21T12:00:00+00:00",
                30000,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 17,
                    passed: 10,
                    failed: 6,
                    exit_code: 0,
                }),
            ),
        ];
        let index = RunIdIndex::new(&runs);
        let alignment = RunListAlignment::from_runs(&runs);

        // All passed counts should be right-aligned to 3 digits (width of 559).
        insta::assert_snapshot!(
            runs[0]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-21 10:00:00      1.000s      97 KB  559 passed"
        );
        insta::assert_snapshot!(
            runs[1]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-21 11:00:00      1.000s      48 KB   51 passed"
        );
        insta::assert_snapshot!(
            runs[2]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-21 12:00:00      1.000s      29 KB   10 passed / 6 failed / 1 not run"
        );
    }

    #[test]
    fn test_display_stress_stats_alignment() {
        // Test that stress test alignment works correctly.
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440009",
                "0.9.109",
                "2024-06-22T10:00:00+00:00",
                200000,
                RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                    initial_iteration_count: NonZero::new(1000),
                    success_count: 1000,
                    failed_count: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-44665544000a",
                "0.9.110",
                "2024-06-22T11:00:00+00:00",
                100000,
                RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                    initial_iteration_count: NonZero::new(100),
                    success_count: 95,
                    failed_count: 5,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-44665544000b",
                "0.9.111",
                "2024-06-22T12:00:00+00:00",
                80000,
                RecordedRunStatus::StressCancelled(StressCompletedRunStats {
                    initial_iteration_count: NonZero::new(500),
                    success_count: 45,
                    failed_count: 5,
                    exit_code: 0,
                }),
            ),
        ];
        let index = RunIdIndex::new(&runs);
        let alignment = RunListAlignment::from_runs(&runs);

        // Passed counts should be right-aligned to 4 digits (width of 1000).
        insta::assert_snapshot!(
            runs[0]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-22 10:00:00      1.000s     195 KB  1000 passed iterations"
        );
        insta::assert_snapshot!(
            runs[1]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-22 11:00:00      1.000s      97 KB    95 passed iterations / 5 failed"
        );
        insta::assert_snapshot!(
            runs[2]
                .display(&index, &ReplayabilityStatus::Replayable, alignment, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"  550e8400  2024-06-22 12:00:00      1.000s      78 KB    45 passed iterations / 5 failed / 450 not run (cancelled)"
        );
    }

    #[test]
    fn test_display_prune_plan_empty() {
        let plan = PrunePlan::new(vec![]);
        let index = RunIdIndex::new(&[]);
        insta::assert_snapshot!(
            plan.display(&index, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"no runs would be pruned"
        );
    }

    #[test]
    fn test_display_prune_plan_single_run() {
        let runs = vec![make_run_info(
            "550e8400-e29b-41d4-a716-446655440002",
            "0.9.102",
            "2024-06-17T12:00:00+00:00",
            2048 * 1024,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 50,
                passed: 50,
                failed: 0,
                exit_code: 0,
            }),
        )];
        let index = RunIdIndex::new(&runs);
        let plan = PrunePlan::new(runs);
        insta::assert_snapshot!(
            plan.display(&index, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"
        would prune 1 run, freeing 2.0 MB:

          550e8400  2024-06-17 12:00:00      1.000s     2.0 MB  50 passed
        "
        );
    }

    #[test]
    fn test_display_prune_plan_multiple_runs() {
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440003",
                "0.9.103",
                "2024-06-18T13:00:00+00:00",
                1024 * 1024,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 100,
                    passed: 100,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440004",
                "0.9.104",
                "2024-06-19T14:00:00+00:00",
                512 * 1024,
                RecordedRunStatus::Incomplete,
            ),
        ];
        let index = RunIdIndex::new(&runs);
        let plan = PrunePlan::new(runs);
        insta::assert_snapshot!(
            plan.display(&index, &Styles::default(), &Redactor::noop())
                .to_string(),
            @"
        would prune 2 runs, freeing 1.5 MB:

          550e8400  2024-06-18 13:00:00      1.000s     1.0 MB  100 passed
          550e8400  2024-06-19 14:00:00      1.000s     512 KB      incomplete
        "
        );
    }

    #[test]
    fn test_display_run_list() {
        let theme_characters = ThemeCharacters::default();

        // Test 1: Normal sizes (typical case with multiple runs).
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440001",
                "0.9.101",
                "2024-06-15T10:00:00+00:00",
                50 * 1024,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 10,
                    passed: 10,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440002",
                "0.9.102",
                "2024-06-16T11:00:00+00:00",
                75 * 1024,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 20,
                    passed: 18,
                    failed: 2,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440003",
                "0.9.103",
                "2024-06-17T12:00:00+00:00",
                100 * 1024,
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 30,
                    passed: 30,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        insta::assert_snapshot!(
            "normal_sizes",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );

        // Test 2: Small sizes (1-2 digits, below the 6-char minimum width).
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440001",
                "0.9.101",
                "2024-06-15T10:00:00+00:00",
                1024, // 1 KB
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 5,
                    passed: 5,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440002",
                "0.9.102",
                "2024-06-16T11:00:00+00:00",
                99 * 1024, // 99 KB
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 10,
                    passed: 10,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        insta::assert_snapshot!(
            "small_sizes",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );

        // Test 3: Large sizes (7-8 digits, exceeding the 6-char minimum width).
        // 1,000,000 KB = ~1 GB, 10,000,000 KB = ~10 GB.
        // The size column and horizontal bar should dynamically expand.
        let runs = vec![
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440001",
                "0.9.101",
                "2024-06-15T10:00:00+00:00",
                1_000_000 * 1024, // 1,000,000 KB (~1 GB)
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 100,
                    passed: 100,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info(
                "550e8400-e29b-41d4-a716-446655440002",
                "0.9.102",
                "2024-06-16T11:00:00+00:00",
                10_000_000 * 1024, // 10,000,000 KB (~10 GB)
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 200,
                    passed: 200,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        insta::assert_snapshot!(
            "large_sizes",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );

        // Test 4: Varying durations (sub-second, seconds, tens, hundreds, thousands).
        // Duration is formatted as {:>9.3}s, so all should right-align properly.
        let runs = vec![
            make_run_info_with_duration(
                "550e8400-e29b-41d4-a716-446655440001",
                "0.9.101",
                "2024-06-15T10:00:00+00:00",
                50 * 1024,
                0.123, // sub-second
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 5,
                    passed: 5,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info_with_duration(
                "550e8400-e29b-41d4-a716-446655440002",
                "0.9.102",
                "2024-06-16T11:00:00+00:00",
                75 * 1024,
                9.876, // single digit seconds
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 10,
                    passed: 10,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info_with_duration(
                "550e8400-e29b-41d4-a716-446655440003",
                "0.9.103",
                "2024-06-17T12:00:00+00:00",
                100 * 1024,
                42.5, // tens of seconds
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 20,
                    passed: 20,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
            make_run_info_with_duration(
                "550e8400-e29b-41d4-a716-446655440004",
                "0.9.104",
                "2024-06-18T13:00:00+00:00",
                125 * 1024,
                987.654, // hundreds of seconds
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 30,
                    passed: 28,
                    failed: 2,
                    exit_code: 0,
                }),
            ),
            make_run_info_with_duration(
                "550e8400-e29b-41d4-a716-446655440005",
                "0.9.105",
                "2024-06-19T14:00:00+00:00",
                150 * 1024,
                12345.678, // thousands of seconds (~3.4 hours)
                RecordedRunStatus::Completed(CompletedRunStats {
                    initial_run_count: 50,
                    passed: 50,
                    failed: 0,
                    exit_code: 0,
                }),
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        insta::assert_snapshot!(
            "varying_durations",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_display_detailed() {
        // Test with CLI args and env vars populated.
        let cli_args = vec![
            "cargo".to_string(),
            "nextest".to_string(),
            "run".to_string(),
            "--workspace".to_string(),
            "--features".to_string(),
            "foo bar".to_string(),
        ];
        let env_vars = BTreeMap::from([
            ("CARGO_HOME".to_string(), "/home/user/.cargo".to_string()),
            ("NEXTEST_PROFILE".to_string(), "ci".to_string()),
        ]);
        let run_with_cli_env = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440000",
            "0.9.122",
            "2024-06-15T10:30:00+00:00",
            cli_args,
            env_vars,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 100,
                passed: 95,
                failed: 5,
                exit_code: 100,
            }),
        );

        // Test with empty CLI args and env vars.
        let run_empty = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440001",
            "0.9.122",
            "2024-06-16T11:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::Incomplete,
        );

        // Test StressCompleted with all iterations passing.
        let stress_all_passed = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440010",
            "0.9.122",
            "2024-06-25T10:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 100,
                failed_count: 0,
                exit_code: 0,
            }),
        );

        // Test StressCompleted with some failures.
        let stress_with_failures = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440011",
            "0.9.122",
            "2024-06-25T11:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::StressCompleted(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 95,
                failed_count: 5,
                exit_code: 0,
            }),
        );

        // Test StressCancelled with some iterations not run.
        let stress_cancelled = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440012",
            "0.9.122",
            "2024-06-25T12:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::StressCancelled(StressCompletedRunStats {
                initial_iteration_count: NonZero::new(100),
                success_count: 50,
                failed_count: 10,
                exit_code: 0,
            }),
        );

        // Test StressCancelled without initial_iteration_count.
        let stress_cancelled_no_initial = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440013",
            "0.9.122",
            "2024-06-25T13:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::StressCancelled(StressCompletedRunStats {
                initial_iteration_count: None,
                success_count: 50,
                failed_count: 10,
                exit_code: 0,
            }),
        );

        let runs = [
            run_with_cli_env,
            run_empty,
            stress_all_passed,
            stress_with_failures,
            stress_cancelled,
            stress_cancelled_no_initial,
        ];
        let index = RunIdIndex::new(&runs);
        let theme_characters = ThemeCharacters::default();
        let redactor = Redactor::noop();
        let now = test_now();
        // Use default (definitely replayable) status for most tests.
        let replayable = ReplayabilityStatus::Replayable;
        // Use incomplete status for the incomplete run.
        let incomplete = ReplayabilityStatus::Incomplete;

        insta::assert_snapshot!(
            "with_cli_and_env",
            runs[0]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
        insta::assert_snapshot!(
            "empty_cli_and_env",
            runs[1]
                .display_detailed(
                    &index,
                    &incomplete,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
        insta::assert_snapshot!(
            "stress_all_passed",
            runs[2]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
        insta::assert_snapshot!(
            "stress_with_failures",
            runs[3]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
        insta::assert_snapshot!(
            "stress_cancelled",
            runs[4]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
        insta::assert_snapshot!(
            "stress_cancelled_no_initial",
            runs[5]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
    }

    #[test]
    fn test_display_detailed_with_parent_run() {
        // Test displaying a run with a parent run ID (rerun scenario).
        let parent_run = make_run_info_with_cli_env(
            "550e8400-e29b-41d4-a716-446655440000",
            "0.9.122",
            "2024-06-15T10:00:00+00:00",
            Vec::new(),
            BTreeMap::new(),
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 100,
                passed: 95,
                failed: 5,
                exit_code: 100,
            }),
        );

        let child_run = make_run_info_with_parent(
            "660e8400-e29b-41d4-a716-446655440001",
            "0.9.122",
            "2024-06-15T11:00:00+00:00",
            vec![
                "cargo".to_string(),
                "nextest".to_string(),
                "run".to_string(),
                "--rerun".to_string(),
            ],
            BTreeMap::new(),
            Some("550e8400-e29b-41d4-a716-446655440000"),
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 5,
                passed: 5,
                failed: 0,
                exit_code: 0,
            }),
        );

        // Include both runs in the index so the parent run ID can be resolved.
        let runs = [parent_run, child_run];
        let index = RunIdIndex::new(&runs);
        let theme_characters = ThemeCharacters::default();
        let redactor = Redactor::noop();
        let now = test_now();
        let replayable = ReplayabilityStatus::Replayable;

        insta::assert_snapshot!(
            "with_parent_run",
            runs[1]
                .display_detailed(
                    &index,
                    &replayable,
                    now,
                    &Styles::default(),
                    &theme_characters,
                    &redactor
                )
                .to_string()
        );
    }

    #[test]
    fn test_display_replayability_statuses() {
        // Create a simple run for testing replayability display.
        let run = make_run_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "0.9.100",
            "2024-06-15T10:30:00+00:00",
            102400,
            RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 100,
                passed: 100,
                failed: 0,
                exit_code: 0,
            }),
        );
        let runs = std::slice::from_ref(&run);
        let index = RunIdIndex::new(runs);
        let theme_characters = ThemeCharacters::default();
        let redactor = Redactor::noop();
        let now = test_now();

        // Test: definitely replayable (no issues).
        let definitely_replayable = ReplayabilityStatus::Replayable;
        insta::assert_snapshot!(
            "replayability_yes",
            run.display_detailed(
                &index,
                &definitely_replayable,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: store format too new.
        let format_too_new =
            ReplayabilityStatus::NotReplayable(vec![NonReplayableReason::StoreFormatTooNew {
                run_version: 5,
                max_supported: 1,
            }]);
        insta::assert_snapshot!(
            "replayability_format_too_new",
            run.display_detailed(
                &index,
                &format_too_new,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: missing store.zip.
        let missing_store =
            ReplayabilityStatus::NotReplayable(vec![NonReplayableReason::MissingStoreZip]);
        insta::assert_snapshot!(
            "replayability_missing_store_zip",
            run.display_detailed(
                &index,
                &missing_store,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: missing run.log.zst.
        let missing_log =
            ReplayabilityStatus::NotReplayable(vec![NonReplayableReason::MissingRunLog]);
        insta::assert_snapshot!(
            "replayability_missing_run_log",
            run.display_detailed(
                &index,
                &missing_log,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: status unknown.
        let status_unknown =
            ReplayabilityStatus::NotReplayable(vec![NonReplayableReason::StatusUnknown]);
        insta::assert_snapshot!(
            "replayability_status_unknown",
            run.display_detailed(
                &index,
                &status_unknown,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: incomplete (maybe replayable).
        let incomplete = ReplayabilityStatus::Incomplete;
        insta::assert_snapshot!(
            "replayability_incomplete",
            run.display_detailed(
                &index,
                &incomplete,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );

        // Test: multiple blocking reasons.
        let multiple_blocking = ReplayabilityStatus::NotReplayable(vec![
            NonReplayableReason::MissingStoreZip,
            NonReplayableReason::MissingRunLog,
        ]);
        insta::assert_snapshot!(
            "replayability_multiple_blocking",
            run.display_detailed(
                &index,
                &multiple_blocking,
                now,
                &Styles::default(),
                &theme_characters,
                &redactor
            )
            .to_string()
        );
    }

    /// Creates a `RecordedRunInfo` for tree display tests with custom size.
    fn make_run_for_tree(
        uuid: &str,
        started_at: &str,
        parent_run_id: Option<&str>,
        size_kb: u64,
        passed: usize,
        failed: usize,
    ) -> RecordedRunInfo {
        let started_at = DateTime::parse_from_rfc3339(started_at).expect("valid datetime");
        RecordedRunInfo {
            run_id: uuid.parse().expect("valid UUID"),
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version: Version::parse("0.9.100").expect("valid version"),
            started_at,
            last_written_at: started_at,
            duration_secs: Some(1.0),
            cli_args: Vec::new(),
            build_scope_args: Vec::new(),
            env_vars: BTreeMap::new(),
            parent_run_id: parent_run_id.map(|s| s.parse().expect("valid UUID")),
            sizes: RecordedSizes {
                log: ComponentSizes::default(),
                store: ComponentSizes {
                    compressed: size_kb * 1024,
                    uncompressed: size_kb * 1024 * 3,
                    entries: 0,
                },
            },
            status: RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: passed + failed,
                passed,
                failed,
                exit_code: if failed > 0 { 1 } else { 0 },
            }),
        }
    }

    #[test]
    fn test_tree_linear_chain() {
        // parent -> child -> grandchild (compressed chain display).
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                8,
                2,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                10,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_linear_chain",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_branching() {
        // parent -> child1, parent -> child2.
        // Children sorted by started_at descending (child2 newer, comes first).
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                5,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                70,
                3,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_branching",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_pruned_parent() {
        // Run whose parent doesn't exist (pruned).
        // Should show virtual parent with "???" indicator.
        let runs = vec![make_run_for_tree(
            "50000002-0000-0000-0000-000000000002",
            "2024-06-15T11:00:00+00:00",
            Some("50000001-0000-0000-0000-000000000001"), // Parent doesn't exist.
            60,
            5,
            0,
        )];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_pruned_parent",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_mixed_independent_and_chain() {
        // Independent run followed by a parent-child chain.
        // Blank line between them since the chain has structure.
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                None,
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                5,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_mixed_independent_and_chain",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_deep_chain() {
        // 5-level deep chain to test continuation lines.
        // a -> b -> c -> d -> e
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                10,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                10,
                0,
            ),
            make_run_for_tree(
                "50000004-0000-0000-0000-000000000004",
                "2024-06-15T13:00:00+00:00",
                Some("50000003-0000-0000-0000-000000000003"),
                80,
                10,
                0,
            ),
            make_run_for_tree(
                "50000005-0000-0000-0000-000000000005",
                "2024-06-15T14:00:00+00:00",
                Some("50000004-0000-0000-0000-000000000004"),
                90,
                10,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_deep_chain",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_branching_with_chains() {
        // parent -> child1 -> grandchild1
        // parent -> child2
        // child1 is older, child2 is newer, so order: parent, child2, child1, grandchild1.
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                5,
                0,
            ),
            make_run_for_tree(
                "50000004-0000-0000-0000-000000000004",
                "2024-06-15T13:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                80,
                3,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_branching_with_chains",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_continuation_flags() {
        // Test that continuation lines (│) appear correctly.
        // parent -> child1 -> grandchild1 (child1 is not last, needs │ continuation)
        // parent -> child2
        // child1 is newer, child2 is older, so child1 is not last.
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T13:00:00+00:00", // Newer than child2.
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T14:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                5,
                0,
            ),
            make_run_for_tree(
                "50000004-0000-0000-0000-000000000004",
                "2024-06-15T11:00:00+00:00", // Older than child1.
                Some("50000001-0000-0000-0000-000000000001"),
                80,
                3,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_continuation_flags",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_pruned_parent_with_chain() {
        // Pruned parent with a chain of descendants.
        // ??? (pruned) -> child -> grandchild
        let runs = vec![
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"), // Parent doesn't exist.
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                5,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_pruned_parent_with_chain",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_pruned_parent_with_multiple_children() {
        // Virtual (pruned) parent with multiple direct children.
        // Both children should show branch characters (|- and \-).
        let runs = vec![
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"), // Parent doesn't exist.
                60,
                5,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"), // Same pruned parent.
                70,
                3,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_pruned_parent_with_multiple_children",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_unicode_characters() {
        // Test with Unicode characters (├─, └─, │).
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T12:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                70,
                5,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        // Create ThemeCharacters with Unicode enabled.
        let mut theme_characters = ThemeCharacters::default();
        theme_characters.use_unicode();

        insta::assert_snapshot!(
            "tree_unicode_characters",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }

    #[test]
    fn test_tree_compressed_with_branching() {
        // Tests the case where a compressed (only-child) node has multiple
        // children, and one child also has children.
        //
        // root (1) -> X (2, only child - compressed)
        //             ├─ Z1 (3, newer, not last)
        //             │  └─ W (4, only child - compressed)
        //             └─ Z2 (5, older, last)
        //
        // Expected display:
        //   1
        //   \-2        <- compressed (no branch for 2's only-child status)
        //     |-3      <- 2's first child (Z1)
        //     | 4      <- Z1's only child (W), compressed, with continuation for Z1
        //     \-5      <- 2's last child (Z2)
        let runs = vec![
            make_run_for_tree(
                "50000001-0000-0000-0000-000000000001",
                "2024-06-15T10:00:00+00:00",
                None,
                50,
                10,
                0,
            ),
            make_run_for_tree(
                "50000002-0000-0000-0000-000000000002",
                "2024-06-15T11:00:00+00:00",
                Some("50000001-0000-0000-0000-000000000001"),
                60,
                8,
                0,
            ),
            make_run_for_tree(
                "50000003-0000-0000-0000-000000000003",
                "2024-06-15T14:00:00+00:00", // Newer - will be first
                Some("50000002-0000-0000-0000-000000000002"),
                70,
                5,
                0,
            ),
            make_run_for_tree(
                "50000004-0000-0000-0000-000000000004",
                "2024-06-15T15:00:00+00:00",
                Some("50000003-0000-0000-0000-000000000003"),
                80,
                3,
                0,
            ),
            make_run_for_tree(
                "50000005-0000-0000-0000-000000000005",
                "2024-06-15T12:00:00+00:00", // Older - will be last
                Some("50000002-0000-0000-0000-000000000002"),
                90,
                2,
                0,
            ),
        ];
        let snapshot = RunStoreSnapshot::new_for_test(runs);
        let snapshot_with_replayability = SnapshotWithReplayability::new_for_test(&snapshot);
        let theme_characters = ThemeCharacters::default();

        insta::assert_snapshot!(
            "tree_compressed_with_branching",
            DisplayRunList::new(
                &snapshot_with_replayability,
                None,
                &Styles::default(),
                &theme_characters,
                &Redactor::noop()
            )
            .to_string()
        );
    }
}
