// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Run store management for nextest recordings.
//!
//! The run store is a directory that contains all recorded test runs. It provides:
//!
//! - A lock file for exclusive access during modifications.
//! - A JSON file listing all recorded runs.
//! - Individual directories for each run containing the archive and log.

use super::{
    format::{RecordedRunList, RunsJsonWritePermission},
    recorder::{RunRecorder, StoreSizes},
    retention::{
        PruneKind, PrunePlan, PruneResult, RecordRetentionPolicy, delete_orphaned_dirs, delete_runs,
    },
    run_id_index::{PrefixResolutionError, RunIdIndex, RunIdSelector},
};
use crate::{
    errors::{RunIdResolutionError, RunStoreError},
    helpers::{ThemeCharacters, plural},
    redact::Redactor,
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use debug_ignore::DebugIgnore;
use owo_colors::{OwoColorize, Style};
use quick_junit::ReportUuid;
use semver::Version;
use std::{
    collections::HashSet,
    fmt,
    fs::{File, TryLockError},
    io::{self, Write},
    num::NonZero,
    thread,
    time::{Duration, Instant},
};
use swrite::{SWrite, swrite};

static RUNS_LOCK_FILE_NAME: &str = "runs.lock";
static RUNS_JSON_FILE_NAME: &str = "runs.json";

/// A reference to the runs directory in a run store.
///
/// This provides methods to compute paths within the runs directory.
#[derive(Clone, Copy, Debug)]
pub struct StoreRunsDir<'a>(&'a Utf8Path);

impl<'a> StoreRunsDir<'a> {
    /// Returns the path to a specific run's directory.
    pub fn run_dir(self, run_id: ReportUuid) -> Utf8PathBuf {
        self.0.join(run_id.to_string())
    }

    /// Returns the underlying path to the runs directory.
    pub fn as_path(self) -> &'a Utf8Path {
        self.0
    }
}

/// Manages the storage of recorded test runs.
///
/// The run store is a directory containing a list of recorded runs and their data.
/// Use [`RunStore::lock_exclusive`] to acquire exclusive access before creating
/// new runs.
#[derive(Debug)]
pub struct RunStore {
    runs_dir: Utf8PathBuf,
}

impl RunStore {
    /// Creates a new `RunStore` at the given directory.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(store_dir: &Utf8Path) -> Result<Self, RunStoreError> {
        let runs_dir = store_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).map_err(|error| RunStoreError::RunDirCreate {
            run_dir: runs_dir.clone(),
            error,
        })?;

        Ok(Self { runs_dir })
    }

    /// Returns the runs directory.
    pub fn runs_dir(&self) -> StoreRunsDir<'_> {
        StoreRunsDir(&self.runs_dir)
    }

    /// Acquires a shared lock on the run store for reading.
    ///
    /// Multiple readers can hold the shared lock simultaneously, but the shared
    /// lock is exclusive with the exclusive lock (used for writing).
    ///
    /// Uses non-blocking lock attempts with retries to handle both brief
    /// contention and filesystems where locking may not work (e.g., NFS).
    pub fn lock_shared(&self) -> Result<SharedLockedRunStore<'_>, RunStoreError> {
        let lock_file_path = self.runs_dir.join(RUNS_LOCK_FILE_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_file_path)
            .map_err(|error| RunStoreError::FileLock {
                path: lock_file_path.clone(),
                error,
            })?;

        acquire_lock_with_retry(&file, &lock_file_path, LockKind::Shared)?;
        let result = read_runs_json(&self.runs_dir)?;
        let run_id_index = RunIdIndex::new(&result.runs);

        Ok(SharedLockedRunStore {
            runs_dir: StoreRunsDir(&self.runs_dir),
            locked_file: DebugIgnore(file),
            runs: result.runs,
            write_permission: result.write_permission,
            run_id_index,
        })
    }

    /// Acquires an exclusive lock on the run store.
    ///
    /// This lock should only be held for a short duration (just long enough to
    /// add a run to the list and create its directory).
    ///
    /// Uses non-blocking lock attempts with retries to handle both brief
    /// contention and filesystems where locking may not work (e.g., NFS).
    pub fn lock_exclusive(&self) -> Result<ExclusiveLockedRunStore<'_>, RunStoreError> {
        let lock_file_path = self.runs_dir.join(RUNS_LOCK_FILE_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_file_path)
            .map_err(|error| RunStoreError::FileLock {
                path: lock_file_path.clone(),
                error,
            })?;

        acquire_lock_with_retry(&file, &lock_file_path, LockKind::Exclusive)?;
        let result = read_runs_json(&self.runs_dir)?;

        Ok(ExclusiveLockedRunStore {
            runs_dir: StoreRunsDir(&self.runs_dir),
            locked_file: DebugIgnore(file),
            runs: result.runs,
            last_pruned_at: result.last_pruned_at,
            write_permission: result.write_permission,
        })
    }
}

/// A run store that has been locked for exclusive access.
///
/// The lifetime parameter ensures this isn't held for longer than the
/// corresponding [`RunStore`].
#[derive(Debug)]
pub struct ExclusiveLockedRunStore<'store> {
    runs_dir: StoreRunsDir<'store>,
    // Held for RAII lock semantics; the lock is released when this struct is dropped.
    #[expect(dead_code)]
    locked_file: DebugIgnore<File>,
    runs: Vec<RecordedRunInfo>,
    last_pruned_at: Option<DateTime<Utc>>,
    write_permission: RunsJsonWritePermission,
}

impl<'store> ExclusiveLockedRunStore<'store> {
    /// Returns the runs directory.
    pub fn runs_dir(&self) -> StoreRunsDir<'store> {
        self.runs_dir
    }

    /// Returns whether this nextest can write to the runs.json file.
    ///
    /// If the file has a newer format version than we support, writing is denied.
    pub fn write_permission(&self) -> RunsJsonWritePermission {
        self.write_permission
    }

    /// Marks a run as completed and persists the change to disk.
    ///
    /// Updates sizes, `status`, and `duration_secs` to the given values.
    /// Returns `true` if the run was found and updated, `false` if no run
    /// with the given ID exists (in which case nothing is persisted).
    ///
    /// Returns an error if writing is denied due to a format version mismatch.
    ///
    /// The status should not be `Incomplete` since we're completing the run.
    pub fn complete_run(
        &mut self,
        run_id: ReportUuid,
        sizes: StoreSizes,
        status: RecordedRunStatus,
        duration_secs: Option<f64>,
    ) -> Result<bool, RunStoreError> {
        if let RunsJsonWritePermission::Denied {
            file_version,
            max_supported_version,
        } = self.write_permission
        {
            return Err(RunStoreError::FormatVersionTooNew {
                file_version,
                max_supported_version,
            });
        }

        let found = self.mark_run_completed_inner(run_id, sizes, status, duration_secs);
        if found {
            write_runs_json(self.runs_dir.as_path(), &self.runs, self.last_pruned_at)?;
        }
        Ok(found)
    }

    /// Updates a run's metadata in memory.
    fn mark_run_completed_inner(
        &mut self,
        run_id: ReportUuid,
        sizes: StoreSizes,
        status: RecordedRunStatus,
        duration_secs: Option<f64>,
    ) -> bool {
        for run in &mut self.runs {
            if run.run_id == run_id {
                run.sizes = RecordedSizes {
                    log: ComponentSizes {
                        compressed: sizes.log.compressed,
                        uncompressed: sizes.log.uncompressed,
                        entries: sizes.log.entries,
                    },
                    store: ComponentSizes {
                        compressed: sizes.store.compressed,
                        uncompressed: sizes.store.uncompressed,
                        entries: sizes.store.entries,
                    },
                };
                run.status = status;
                run.duration_secs = duration_secs;
                run.last_written_at = Local::now().fixed_offset();
                return true;
            }
        }
        false
    }

    /// Prunes runs according to the given retention policy.
    ///
    /// This method:
    /// 1. Determines which runs to delete based on the policy
    /// 2. Deletes those run directories from disk
    /// 3. Deletes any orphaned directories not tracked in runs.json
    /// 4. Updates the run list in memory and on disk
    ///
    /// The `kind` parameter indicates whether this is explicit pruning (from a
    /// user command) or implicit pruning (automatic during recording). This
    /// affects how errors are displayed.
    ///
    /// Returns the result of the pruning operation, including any errors that
    /// occurred while deleting individual runs.
    ///
    /// Returns an error if writing is denied due to a format version mismatch.
    pub fn prune(
        &mut self,
        policy: &RecordRetentionPolicy,
        kind: PruneKind,
    ) -> Result<PruneResult, RunStoreError> {
        if let RunsJsonWritePermission::Denied {
            file_version,
            max_supported_version,
        } = self.write_permission
        {
            return Err(RunStoreError::FormatVersionTooNew {
                file_version,
                max_supported_version,
            });
        }

        let now = Utc::now();
        let to_delete: HashSet<_> = policy
            .compute_runs_to_delete(&self.runs, now)
            .into_iter()
            .collect();

        let runs_dir = self.runs_dir();
        let mut result = if to_delete.is_empty() {
            PruneResult::default()
        } else {
            delete_runs(runs_dir, &mut self.runs, &to_delete)
        };
        result.kind = kind;

        let known_runs: HashSet<_> = self.runs.iter().map(|r| r.run_id).collect();
        delete_orphaned_dirs(self.runs_dir, &known_runs, &mut result);

        if result.deleted_count > 0 || result.orphans_deleted > 0 {
            // Update last_pruned_at since we performed pruning.
            self.last_pruned_at = Some(now);
            write_runs_json(self.runs_dir.as_path(), &self.runs, self.last_pruned_at)?;
        }

        Ok(result)
    }

    /// Prunes runs if needed, based on time since last prune and limit thresholds.
    ///
    /// This method implements implicit pruning, which occurs:
    /// - If more than 1 day has passed since the last prune, OR
    /// - If any retention limit is exceeded by 1.5x.
    ///
    /// Use [`Self::prune`] for explicit pruning that always runs regardless of these conditions.
    ///
    /// Returns `Ok(None)` if pruning was skipped, `Ok(Some(result))` if pruning occurred.
    pub fn prune_if_needed(
        &mut self,
        policy: &RecordRetentionPolicy,
    ) -> Result<Option<PruneResult>, RunStoreError> {
        const PRUNE_INTERVAL: TimeDelta = match TimeDelta::try_days(1) {
            Some(d) => d,
            None => panic!("1 day should always be a valid TimeDelta"),
        };
        const LIMIT_EXCEEDED_FACTOR: f64 = 1.5;

        let now = Utc::now();

        // Check if pruning is needed.
        let time_since_last_prune = self
            .last_pruned_at
            .map(|last| now.signed_duration_since(last))
            .unwrap_or(TimeDelta::MAX);

        let should_prune = time_since_last_prune >= PRUNE_INTERVAL
            || policy.limits_exceeded_by_factor(&self.runs, LIMIT_EXCEEDED_FACTOR);

        if should_prune {
            Ok(Some(self.prune(policy, PruneKind::Implicit)?))
        } else {
            Ok(None)
        }
    }

    /// Creates a run recorder for a new run.
    ///
    /// Adds the run to the list and creates its directory. Consumes self,
    /// dropping the exclusive lock.
    ///
    /// `max_output_size` specifies the maximum size of a single output (stdout/stderr)
    /// before truncation.
    ///
    /// Returns an error if writing is denied due to a format version mismatch.
    pub fn create_run_recorder(
        mut self,
        run_id: ReportUuid,
        nextest_version: Version,
        started_at: DateTime<FixedOffset>,
        max_output_size: bytesize::ByteSize,
    ) -> Result<RunRecorder, RunStoreError> {
        if let RunsJsonWritePermission::Denied {
            file_version,
            max_supported_version,
        } = self.write_permission
        {
            return Err(RunStoreError::FormatVersionTooNew {
                file_version,
                max_supported_version,
            });
        }

        // Add to the list of runs before creating the directory. This ensures
        // that if creation fails, an empty run directory isn't left behind. (It
        // does mean that there may be spurious entries in the list of runs,
        // which will be dealt with during pruning.)

        let run = RecordedRunInfo {
            run_id,
            nextest_version,
            started_at,
            last_written_at: Local::now().fixed_offset(),
            duration_secs: None,
            sizes: RecordedSizes::default(),
            status: RecordedRunStatus::Incomplete,
        };
        self.runs.push(run);
        write_runs_json(self.runs_dir.as_path(), &self.runs, self.last_pruned_at)?;

        // Create the run directory while still holding the lock. This prevents
        // a race where another process could prune the newly-added run entry
        // before the directory exists, leaving an orphaned directory. The lock
        // is released when `self` is dropped.
        let run_dir = self.runs_dir().run_dir(run_id);

        RunRecorder::new(run_dir, max_output_size)
    }
}

/// Information about a recorded run.
#[derive(Clone, Debug)]
pub struct RecordedRunInfo {
    /// The unique identifier for this run.
    pub run_id: ReportUuid,
    /// The version of nextest that created this run.
    pub nextest_version: Version,
    /// When the run started.
    pub started_at: DateTime<FixedOffset>,
    /// When this run was last written to.
    ///
    /// Used for LRU eviction. Updated when the run is created, when the run
    /// completes, and in the future when operations like `rerun` reference
    /// this run.
    pub last_written_at: DateTime<FixedOffset>,
    /// Duration of the run in seconds.
    ///
    /// This is `None` for incomplete runs.
    pub duration_secs: Option<f64>,
    /// Sizes broken down by component (log and store).
    pub sizes: RecordedSizes,
    /// The status and statistics for this run.
    pub status: RecordedRunStatus,
}

/// Sizes broken down by component (log and store).
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordedSizes {
    /// Sizes for the run log (run.log.zst).
    pub log: ComponentSizes,
    /// Sizes for the store archive (store.zip).
    pub store: ComponentSizes,
}

/// Compressed and uncompressed sizes for a single component.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComponentSizes {
    /// Compressed size in bytes.
    pub compressed: u64,
    /// Uncompressed size in bytes.
    pub uncompressed: u64,
    /// Number of entries (records for log, files for store).
    pub entries: u64,
}

impl RecordedSizes {
    /// Returns the total compressed size (log + store).
    pub fn total_compressed(&self) -> u64 {
        self.log.compressed + self.store.compressed
    }

    /// Returns the total uncompressed size (log + store).
    pub fn total_uncompressed(&self) -> u64 {
        self.log.uncompressed + self.store.uncompressed
    }

    /// Returns the total number of entries (log records + store files).
    pub fn total_entries(&self) -> u64 {
        self.log.entries + self.store.entries
    }
}

/// Status and statistics for a recorded run.
#[derive(Clone, Debug)]
pub enum RecordedRunStatus {
    /// The run was interrupted before completion.
    Incomplete,
    /// A normal test run completed (all tests finished).
    Completed(CompletedRunStats),
    /// A normal test run was cancelled before all tests finished.
    Cancelled(CompletedRunStats),
    /// A stress test run completed (all iterations finished).
    StressCompleted(StressCompletedRunStats),
    /// A stress test run was cancelled before all iterations finished.
    StressCancelled(StressCompletedRunStats),
    /// An unknown status from a newer version of nextest.
    ///
    /// This variant is used for forward compatibility when reading runs.json
    /// files created by newer nextest versions that may have new status types.
    Unknown,
}

impl RecordedRunStatus {
    /// Returns a short status string for display.
    pub fn short_status_str(&self) -> &'static str {
        match self {
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
            Self::Completed(_) => "completed",
            Self::Cancelled(_) => "cancelled",
            Self::StressCompleted(_) => "stress completed",
            Self::StressCancelled(_) => "stress cancelled",
        }
    }

    /// Returns true if this run can be replayed.
    ///
    /// Incomplete runs may have an incomplete archive and cannot be replayed.
    /// Unknown status runs are treated as not replayable for safety.
    pub fn is_replayable(&self) -> bool {
        match self {
            Self::Incomplete | Self::Unknown => false,
            Self::Completed(_)
            | Self::Cancelled(_)
            | Self::StressCompleted(_)
            | Self::StressCancelled(_) => true,
        }
    }
}

/// Statistics for a normal test run that finished (completed or cancelled).
#[derive(Clone, Copy, Debug)]
pub struct CompletedRunStats {
    /// The number of tests that were expected to run.
    pub initial_run_count: usize,
    /// The number of tests that passed.
    pub passed: usize,
    /// The number of tests that failed (including exec failures and timeouts).
    pub failed: usize,
}

/// Statistics for a stress test run that finished (completed or cancelled).
#[derive(Clone, Copy, Debug)]
pub struct StressCompletedRunStats {
    /// The number of stress iterations that were expected to run, if known.
    ///
    /// This is `None` when the stress test was run without a fixed iteration count
    /// (e.g., `--stress-duration`).
    pub initial_iteration_count: Option<NonZero<u32>>,
    /// The number of stress iterations that succeeded.
    pub success_count: u32,
    /// The number of stress iterations that failed.
    pub failed_count: u32,
}

/// Result of looking up the most recent replayable run.
#[derive(Clone, Copy, Debug)]
pub struct ResolveRunIdResult {
    /// The run ID of the most recent replayable run.
    pub run_id: ReportUuid,
    /// The number of newer runs that are not replayable (incomplete or unknown
    /// status).
    pub newer_incomplete_count: usize,
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

impl RecordedRunStatus {
    /// Returns the width (in decimal digits) needed to display the "passed" count.
    ///
    /// For non-completed runs (Incomplete, Unknown), returns 0 since they don't
    /// display a passed count.
    pub fn passed_count_width(&self) -> usize {
        use crate::helpers::usize_decimal_char_width;

        match self {
            Self::Incomplete | Self::Unknown => 0,
            Self::Completed(stats) | Self::Cancelled(stats) => {
                usize_decimal_char_width(stats.passed)
            }
            Self::StressCompleted(stats) | Self::StressCancelled(stats) => {
                // Stress tests use u32, convert to usize for width calculation.
                usize_decimal_char_width(stats.success_count as usize)
            }
        }
    }
}

impl RecordedRunInfo {
    /// Returns a display wrapper for this run.
    ///
    /// The `run_id_index` is used for computing shortest unique prefixes,
    /// which are highlighted differently in the output (similar to jj).
    ///
    /// The `alignment` parameter controls column alignment when displaying a
    /// list of runs. Use [`RunListAlignment::from_runs`] to precompute
    /// alignment for a set of runs.
    ///
    /// The `redactor` parameter, if provided, redacts timestamps, durations,
    /// and sizes for snapshot testing while preserving column alignment.
    pub fn display<'a>(
        &'a self,
        run_id_index: &'a RunIdIndex,
        alignment: RunListAlignment,
        styles: &'a Styles,
        redactor: Option<&'a Redactor>,
    ) -> DisplayRecordedRunInfo<'a> {
        DisplayRecordedRunInfo::new(self, run_id_index, alignment, styles, redactor)
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

/// A display wrapper for [`RecordedRunInfo`].
#[derive(Clone, Debug)]
pub struct DisplayRecordedRunInfo<'a> {
    run: &'a RecordedRunInfo,
    run_id_index: &'a RunIdIndex,
    alignment: RunListAlignment,
    styles: &'a Styles,
    redactor: Option<&'a Redactor>,
}

impl<'a> DisplayRecordedRunInfo<'a> {
    fn new(
        run: &'a RecordedRunInfo,
        run_id_index: &'a RunIdIndex,
        alignment: RunListAlignment,
        styles: &'a Styles,
        redactor: Option<&'a Redactor>,
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

        // Format timestamp, duration, and size with optional redaction.
        // When redacted, use fixed-width placeholders to preserve column alignment.
        let size_kb = run.sizes.total_compressed() / 1024;

        if let Some(redactor) = self.redactor {
            let timestamp_display = redactor.redact_timestamp(&run.started_at);
            let duration_display = redactor.redact_store_duration(run.duration_secs);
            let size_display = redactor.redact_size_kb(size_kb);

            write!(
                f,
                "  {}  {}  {}  {:>6} KB  {}",
                run_id_display,
                timestamp_display.style(self.styles.timestamp),
                duration_display.style(self.styles.duration),
                size_display.style(self.styles.size),
                status_display,
            )
        } else {
            let timestamp_display = run.started_at.format("%Y-%m-%d %H:%M:%S");
            let duration_display = match run.duration_secs {
                Some(secs) => format!("{secs:>9.3}s"),
                None => format!("{:>10}", "-"),
            };

            write!(
                f,
                "  {}  {}  {}  {:>6} KB  {}",
                run_id_display,
                timestamp_display.style(self.styles.timestamp),
                duration_display.style(self.styles.duration),
                size_kb.style(self.styles.size),
                status_display,
            )
        }
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
    redactor: Option<&'a Redactor>,
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
        redactor: Option<&'a Redactor>,
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

        // Optionally redact the total size for snapshot testing.
        if let Some(redactor) = self.redactor {
            let size_display = redactor.redact_size_kb(total_size_kb);
            writeln!(
                f,
                "                                             {:>6} KB",
                size_display.style(self.styles.size),
            )?;
        } else {
            writeln!(
                f,
                "                                             {:>6} KB",
                total_size_kb.style(self.styles.size),
            )?;
        }

        Ok(())
    }
}

/// Result of reading runs.json.
struct ReadRunsJsonResult {
    runs: Vec<RecordedRunInfo>,
    last_pruned_at: Option<DateTime<Utc>>,
    write_permission: RunsJsonWritePermission,
}

/// Reads and deserializes `runs.json`, converting to the internal
/// representation.
fn read_runs_json(runs_dir: &Utf8Path) -> Result<ReadRunsJsonResult, RunStoreError> {
    let runs_json_path = runs_dir.join(RUNS_JSON_FILE_NAME);
    match std::fs::read_to_string(&runs_json_path) {
        Ok(runs_json) => {
            let list: RecordedRunList = serde_json::from_str(&runs_json).map_err(|error| {
                RunStoreError::RunListDeserialize {
                    path: runs_json_path,
                    error,
                }
            })?;
            let write_permission = list.write_permission();
            let data = list.into_data();
            Ok(ReadRunsJsonResult {
                runs: data.runs,
                last_pruned_at: data.last_pruned_at,
                write_permission,
            })
        }
        Err(error) => {
            if error.kind() == io::ErrorKind::NotFound {
                // The file doesn't exist yet, so we can write a new one.
                Ok(ReadRunsJsonResult {
                    runs: Vec::new(),
                    last_pruned_at: None,
                    write_permission: RunsJsonWritePermission::Allowed,
                })
            } else {
                Err(RunStoreError::RunListRead {
                    path: runs_json_path,
                    error,
                })
            }
        }
    }
}

/// Serializes and writes runs.json from internal representation.
fn write_runs_json(
    runs_dir: &Utf8Path,
    runs: &[RecordedRunInfo],
    last_pruned_at: Option<DateTime<Utc>>,
) -> Result<(), RunStoreError> {
    let runs_json_path = runs_dir.join(RUNS_JSON_FILE_NAME);
    let list = RecordedRunList::from_data(runs, last_pruned_at);
    let runs_json =
        serde_json::to_string_pretty(&list).map_err(|error| RunStoreError::RunListSerialize {
            path: runs_json_path.clone(),
            error,
        })?;

    atomicwrites::AtomicFile::new(&runs_json_path, atomicwrites::AllowOverwrite)
        .write(|file| file.write_all(runs_json.as_bytes()))
        .map_err(|error| RunStoreError::RunListWrite {
            path: runs_json_path,
            error,
        })?;

    Ok(())
}

/// A run store that has been locked for shared (read-only) access.
///
/// Multiple readers can hold this lock simultaneously, but it is exclusive
/// with the exclusive lock used for writing.
#[derive(Debug)]
pub struct SharedLockedRunStore<'store> {
    runs_dir: StoreRunsDir<'store>,
    #[expect(dead_code, reason = "held for lock duration")]
    locked_file: DebugIgnore<File>,
    runs: Vec<RecordedRunInfo>,
    write_permission: RunsJsonWritePermission,
    run_id_index: RunIdIndex,
}

impl<'store> SharedLockedRunStore<'store> {
    /// Returns a snapshot of the runs data, consuming self and releasing the
    /// lock.
    pub fn into_snapshot(self) -> RunStoreSnapshot {
        RunStoreSnapshot {
            runs_dir: self.runs_dir.as_path().to_owned(),
            runs: self.runs,
            write_permission: self.write_permission,
            run_id_index: self.run_id_index,
        }
    }
}

/// A snapshot of run store data.
#[derive(Debug)]
pub struct RunStoreSnapshot {
    runs_dir: Utf8PathBuf,
    runs: Vec<RecordedRunInfo>,
    write_permission: RunsJsonWritePermission,
    run_id_index: RunIdIndex,
}

impl RunStoreSnapshot {
    /// Returns the runs directory.
    pub fn runs_dir(&self) -> StoreRunsDir<'_> {
        StoreRunsDir(&self.runs_dir)
    }

    /// Returns whether this nextest can write to the runs.json file.
    ///
    /// If the file has a newer format version than we support, writing is denied.
    pub fn write_permission(&self) -> RunsJsonWritePermission {
        self.write_permission
    }

    /// Returns a list of recorded runs.
    pub fn runs(&self) -> &[RecordedRunInfo] {
        &self.runs
    }

    /// Returns a mutable reference to the recorded runs.
    ///
    /// Useful for sorting runs before display.
    pub fn runs_mut(&mut self) -> &mut Vec<RecordedRunInfo> {
        &mut self.runs
    }

    /// Returns the number of recorded runs.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Returns the total compressed size of all recorded runs in bytes.
    pub fn total_size(&self) -> u64 {
        self.runs.iter().map(|r| r.sizes.total_compressed()).sum()
    }

    /// Resolves a run ID selector to a run result.
    ///
    /// For [`RunIdSelector::Latest`], returns the most recent replayable run.
    /// For [`RunIdSelector::Prefix`], resolves the prefix to a specific run.
    ///
    /// Returns a [`ResolveRunIdResult`] containing the run ID and, for
    /// `Latest`, the count of newer incomplete runs that were skipped.
    pub fn resolve_run_id(
        &self,
        selector: &RunIdSelector,
    ) -> Result<ResolveRunIdResult, RunIdResolutionError> {
        match selector {
            RunIdSelector::Latest => self.most_recent_run(),
            RunIdSelector::Prefix(prefix) => {
                let run_id = self.resolve_run_id_prefix(prefix)?;
                Ok(ResolveRunIdResult {
                    run_id,
                    newer_incomplete_count: 0,
                })
            }
        }
    }

    /// Resolves a run ID prefix to a full UUID.
    ///
    /// The prefix must be a valid hexadecimal string. If the prefix matches
    /// exactly one run, that run's UUID is returned. Otherwise, an error is
    /// returned indicating whether no runs matched or multiple runs matched.
    fn resolve_run_id_prefix(&self, prefix: &str) -> Result<ReportUuid, RunIdResolutionError> {
        self.run_id_index.resolve_prefix(prefix).map_err(|err| {
            match err {
                PrefixResolutionError::NotFound => RunIdResolutionError::NotFound {
                    prefix: prefix.to_string(),
                },
                PrefixResolutionError::Ambiguous { count, candidates } => {
                    // Convert UUIDs to full RecordedRunInfo and sort by start time (most recent first).
                    let mut candidates: Vec<_> = candidates
                        .into_iter()
                        .filter_map(|run_id| self.get_run(run_id).cloned())
                        .collect();
                    candidates.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                    RunIdResolutionError::Ambiguous {
                        prefix: prefix.to_string(),
                        count,
                        candidates,
                        run_id_index: self.run_id_index.clone(),
                    }
                }
                PrefixResolutionError::InvalidPrefix => RunIdResolutionError::InvalidPrefix {
                    prefix: prefix.to_string(),
                },
            }
        })
    }

    /// Returns the run ID index for computing shortest unique prefixes.
    pub fn run_id_index(&self) -> &RunIdIndex {
        &self.run_id_index
    }

    /// Looks up a run by its exact UUID.
    pub fn get_run(&self, run_id: ReportUuid) -> Option<&RecordedRunInfo> {
        self.runs.iter().find(|r| r.run_id == run_id)
    }

    /// Returns the most recent replayable run.
    ///
    /// If there are newer runs that are not replayable (incomplete or unknown
    /// status), those are counted and returned in the result.
    ///
    /// Returns an error if there are no runs at all, or if there are runs but
    /// none are replayable.
    pub fn most_recent_run(&self) -> Result<ResolveRunIdResult, RunIdResolutionError> {
        if self.runs.is_empty() {
            return Err(RunIdResolutionError::NoRuns);
        }

        // Sort runs by started_at in descending order (most recent first).
        let mut sorted_runs: Vec<_> = self.runs.iter().collect();
        sorted_runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));

        // Find the first replayable run and count non-replayable runs before it.
        let mut newer_incomplete_count = 0;
        for run in sorted_runs {
            if run.status.is_replayable() {
                return Ok(ResolveRunIdResult {
                    run_id: run.run_id,
                    newer_incomplete_count,
                });
            }
            newer_incomplete_count += 1;
        }

        Err(RunIdResolutionError::NoCompletedRuns {
            incomplete_count: newer_incomplete_count,
        })
    }

    /// Computes which runs would be deleted by a prune operation.
    ///
    /// This is used for dry-run mode to show what would be deleted without
    /// actually deleting anything. Returns a [`PrunePlan`] containing the runs
    /// that would be deleted, sorted by start time (oldest first).
    pub fn compute_prune_plan(&self, policy: &RecordRetentionPolicy) -> PrunePlan {
        PrunePlan::compute(&self.runs, policy)
    }
}

/// The kind of lock to acquire.
#[derive(Clone, Copy)]
enum LockKind {
    Shared,
    Exclusive,
}

/// Acquires a file lock with retries, timing out after 5 seconds.
///
/// This handles both brief contention (another nextest process finishing up)
/// and filesystems where locking may not work properly (e.g., NFS).
fn acquire_lock_with_retry(
    file: &File,
    lock_file_path: &Utf8Path,
    kind: LockKind,
) -> Result<(), RunStoreError> {
    const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
    const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(100);

    let start = Instant::now();
    loop {
        let result = match kind {
            LockKind::Shared => file.try_lock_shared(),
            LockKind::Exclusive => file.try_lock(),
        };

        match result {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {
                // Lock is held by another process. Retry if we haven't timed out.
                if start.elapsed() >= LOCK_TIMEOUT {
                    return Err(RunStoreError::FileLockTimeout {
                        path: lock_file_path.to_owned(),
                        timeout_secs: LOCK_TIMEOUT.as_secs(),
                    });
                }
                thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(TryLockError::Error(error)) => {
                // Some other error (e.g., locking not supported on this filesystem).
                return Err(RunStoreError::FileLock {
                    path: lock_file_path.to_owned(),
                    error,
                });
            }
        }
    }
}
