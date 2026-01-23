// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Run store management for nextest recordings.
//!
//! The run store is a directory that contains all recorded test runs. It provides:
//!
//! - A lock file for exclusive access during modifications.
//! - A zstd-compressed JSON file (`runs.json.zst`) listing all recorded runs.
//! - Individual directories for each run containing the archive and log.

use super::{
    display::{DisplayRecordedRunInfo, DisplayRecordedRunInfoDetailed, RunListAlignment, Styles},
    format::{
        RECORD_FORMAT_VERSION, RUN_LOG_FILE_NAME, RecordedRunList, RunsJsonWritePermission,
        STORE_ZIP_FILE_NAME,
    },
    recorder::{RunRecorder, StoreSizes},
    retention::{
        PruneKind, PrunePlan, PruneResult, RecordRetentionPolicy, delete_orphaned_dirs, delete_runs,
    },
    run_id_index::{PrefixResolutionError, RunIdIndex, RunIdSelector, ShortestRunIdPrefix},
};
use crate::{
    errors::{RunIdResolutionError, RunStoreError},
    helpers::{ThemeCharacters, u32_decimal_char_width, usize_decimal_char_width},
    redact::Redactor,
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, FixedOffset, Local, TimeDelta, Utc};
use debug_ignore::DebugIgnore;
use quick_junit::ReportUuid;
use semver::Version;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    fs::{File, TryLockError},
    io,
    num::NonZero,
    thread,
    time::{Duration, Instant},
};

static RUNS_LOCK_FILE_NAME: &str = "runs.lock";
static RUNS_JSON_FILE_NAME: &str = "runs.json.zst";

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

    /// Returns whether this nextest can write to the runs.json.zst file.
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
    /// 3. Deletes any orphaned directories not tracked in runs.json.zst
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
    /// Returns the recorder and the shortest unique prefix for the run ID (for
    /// display purposes), or an error if writing is denied due to a format
    /// version mismatch.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn create_run_recorder(
        mut self,
        run_id: ReportUuid,
        nextest_version: Version,
        started_at: DateTime<FixedOffset>,
        cli_args: Vec<String>,
        build_scope_args: Vec<String>,
        env_vars: BTreeMap<String, String>,
        max_output_size: bytesize::ByteSize,
        parent_run_id: Option<ReportUuid>,
    ) -> Result<(RunRecorder, ShortestRunIdPrefix), RunStoreError> {
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

        let now = Local::now().fixed_offset();
        let run = RecordedRunInfo {
            run_id,
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version,
            started_at,
            last_written_at: now,
            duration_secs: None,
            cli_args,
            build_scope_args,
            env_vars,
            parent_run_id,
            sizes: RecordedSizes::default(),
            status: RecordedRunStatus::Incomplete,
        };
        self.runs.push(run);

        // If the parent run ID is set, update its last written at time.
        if let Some(parent_run_id) = parent_run_id
            && let Some(parent_run) = self.runs.iter_mut().find(|r| r.run_id == parent_run_id)
        {
            parent_run.last_written_at = now;
        }

        write_runs_json(self.runs_dir.as_path(), &self.runs, self.last_pruned_at)?;

        // Compute the unique prefix now that the run is in the list.
        let index = RunIdIndex::new(&self.runs);
        let unique_prefix = index
            .shortest_unique_prefix(run_id)
            .expect("run was just added to the list");

        // Create the run directory while still holding the lock. This prevents
        // a race where another process could prune the newly-added run entry
        // before the directory exists, leaving an orphaned directory. The lock
        // is released when `self` is dropped.
        let run_dir = self.runs_dir().run_dir(run_id);

        let recorder = RunRecorder::new(run_dir, max_output_size)?;
        Ok((recorder, unique_prefix))
    }
}

/// Information about a recorded run.
#[derive(Clone, Debug)]
pub struct RecordedRunInfo {
    /// The unique identifier for this run.
    pub run_id: ReportUuid,
    /// The format version of this run's store.zip archive.
    ///
    /// This allows checking replayability without opening the archive.
    pub store_format_version: u32,
    /// The version of nextest that created this run.
    pub nextest_version: Version,
    /// When the run started.
    pub started_at: DateTime<FixedOffset>,
    /// When this run was last written to.
    ///
    /// Used for LRU eviction. Updated when the run is created, when the run
    /// completes, and when a rerun references this run.
    pub last_written_at: DateTime<FixedOffset>,
    /// Duration of the run in seconds.
    ///
    /// This is `None` for incomplete runs.
    pub duration_secs: Option<f64>,
    /// The command-line arguments used to invoke nextest.
    pub cli_args: Vec<String>,
    /// Build scope arguments (package and target selection).
    ///
    /// These determine which packages and targets are built. In a rerun chain,
    /// these are inherited from the original run unless explicitly overridden.
    pub build_scope_args: Vec<String>,
    /// Environment variables that affect nextest behavior (NEXTEST_* and CARGO_*).
    pub env_vars: BTreeMap<String, String>,
    /// If this is a rerun, the ID of the parent run.
    ///
    /// This forms a chain for iterative fix-and-rerun workflows.
    pub parent_run_id: Option<ReportUuid>,
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
    /// This variant is used for forward compatibility when reading runs.json.zst
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

    /// Returns the exit code for completed runs, or `None` for incomplete/unknown runs.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Incomplete | Self::Unknown => None,
            Self::Completed(stats) | Self::Cancelled(stats) => Some(stats.exit_code),
            Self::StressCompleted(stats) | Self::StressCancelled(stats) => Some(stats.exit_code),
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
    /// The exit code from the run.
    pub exit_code: i32,
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
    /// The exit code from the run.
    pub exit_code: i32,
}

// ---
// Replayability checking
// ---

/// The result of checking whether a run can be replayed.
#[derive(Clone, Debug)]
pub enum ReplayabilityStatus {
    /// The run is definitely replayable.
    ///
    /// No blocking reasons and no uncertain conditions.
    Replayable,
    /// The run is definitely not replayable.
    ///
    /// Contains at least one blocking reason.
    NotReplayable(Vec<NonReplayableReason>),
    /// The run might be replayable but is incomplete.
    ///
    /// The archive might be usable, but we'd need to open `store.zip` to
    /// verify all expected files are present.
    Incomplete,
}

/// A definite reason why a run cannot be replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonReplayableReason {
    /// The run's store format version is newer than this nextest supports.
    ///
    /// This nextest version cannot read the archive format.
    StoreFormatTooNew {
        /// The format version in the run's archive.
        run_version: u32,
        /// The maximum format version this nextest supports.
        max_supported: u32,
    },
    /// The `store.zip` file is missing from the run directory.
    MissingStoreZip,
    /// The `run.log.zst` file is missing from the run directory.
    MissingRunLog,
    /// The run status is `Unknown` (from a newer nextest version).
    ///
    /// We cannot safely replay since we don't understand the run's state.
    StatusUnknown,
}

impl fmt::Display for NonReplayableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreFormatTooNew {
                run_version,
                max_supported,
            } => {
                write!(
                    f,
                    "store format version {} is newer than supported (version {})",
                    run_version, max_supported
                )
            }
            Self::MissingStoreZip => {
                write!(f, "store.zip is missing")
            }
            Self::MissingRunLog => {
                write!(f, "run.log.zst is missing")
            }
            Self::StatusUnknown => {
                write!(f, "run status is unknown (from a newer nextest version)")
            }
        }
    }
}

/// Result of looking up a run by selector.
#[derive(Clone, Copy, Debug)]
pub struct ResolveRunIdResult {
    /// The run ID.
    pub run_id: ReportUuid,
}

impl RecordedRunStatus {
    /// Returns the width (in decimal digits) needed to display the "passed" count.
    ///
    /// For non-completed runs (Incomplete, Unknown), returns 0 since they don't
    /// display a passed count.
    pub fn passed_count_width(&self) -> usize {
        match self {
            Self::Incomplete | Self::Unknown => 0,
            Self::Completed(stats) | Self::Cancelled(stats) => {
                usize_decimal_char_width(stats.passed)
            }
            Self::StressCompleted(stats) | Self::StressCancelled(stats) => {
                // Stress tests use u32, convert to usize for width calculation.
                u32_decimal_char_width(stats.success_count)
            }
        }
    }
}

impl RecordedRunInfo {
    /// Checks whether this run can be replayed.
    ///
    /// This performs a comprehensive check of all conditions that might prevent
    /// replay, including:
    /// - Store format version compatibility
    /// - Presence of required files (store.zip, run.log.zst)
    /// - Run status (unknown, incomplete)
    ///
    /// The `runs_dir` parameter is used to check for file existence on disk.
    pub fn check_replayability(&self, runs_dir: StoreRunsDir<'_>) -> ReplayabilityStatus {
        let mut blocking = Vec::new();
        let mut is_incomplete = false;

        // Check store format version.
        if self.store_format_version > RECORD_FORMAT_VERSION {
            blocking.push(NonReplayableReason::StoreFormatTooNew {
                run_version: self.store_format_version,
                max_supported: RECORD_FORMAT_VERSION,
            });
        }
        // Note: When we bump format versions, add a similar StoreFormatTooOld
        // check here.

        // Check for required files on disk.
        let run_dir = runs_dir.run_dir(self.run_id);
        let store_zip_path = run_dir.join(STORE_ZIP_FILE_NAME);
        let run_log_path = run_dir.join(RUN_LOG_FILE_NAME);

        if !store_zip_path.exists() {
            blocking.push(NonReplayableReason::MissingStoreZip);
        }
        if !run_log_path.exists() {
            blocking.push(NonReplayableReason::MissingRunLog);
        }

        // Check run status.
        match &self.status {
            RecordedRunStatus::Unknown => {
                blocking.push(NonReplayableReason::StatusUnknown);
            }
            RecordedRunStatus::Incomplete => {
                is_incomplete = true;
            }
            RecordedRunStatus::Completed(_)
            | RecordedRunStatus::Cancelled(_)
            | RecordedRunStatus::StressCompleted(_)
            | RecordedRunStatus::StressCancelled(_) => {
                // These statuses are fine for replay.
            }
        }

        // Return the appropriate variant based on what we found.
        if !blocking.is_empty() {
            ReplayabilityStatus::NotReplayable(blocking)
        } else if is_incomplete {
            ReplayabilityStatus::Incomplete
        } else {
            ReplayabilityStatus::Replayable
        }
    }

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
        replayability: &'a ReplayabilityStatus,
        alignment: RunListAlignment,
        styles: &'a Styles,
        redactor: &'a Redactor,
    ) -> DisplayRecordedRunInfo<'a> {
        DisplayRecordedRunInfo::new(
            self,
            run_id_index,
            replayability,
            alignment,
            styles,
            redactor,
        )
    }

    /// Returns a detailed display wrapper for this run.
    ///
    /// Unlike [`Self::display`] which shows a compact table row, this provides
    /// a multi-line detailed view suitable for the `store info` command.
    ///
    /// The `replayability` parameter should be computed by the caller using
    /// [`Self::check_replayability`].
    ///
    /// The `now` parameter is the current time, used to compute relative
    /// durations (e.g. "30s ago").
    ///
    /// The `redactor` parameter redacts paths, timestamps, durations, and sizes
    /// for snapshot testing. Use `Redactor::noop()` if no redaction is needed.
    pub fn display_detailed<'a>(
        &'a self,
        run_id_index: &'a RunIdIndex,
        replayability: &'a ReplayabilityStatus,
        now: DateTime<Utc>,
        styles: &'a Styles,
        theme_characters: &'a ThemeCharacters,
        redactor: &'a Redactor,
    ) -> DisplayRecordedRunInfoDetailed<'a> {
        DisplayRecordedRunInfoDetailed::new(
            self,
            run_id_index,
            replayability,
            now,
            styles,
            theme_characters,
            redactor,
        )
    }
}

/// Result of reading runs.json.zst.
struct ReadRunsJsonResult {
    runs: Vec<RecordedRunInfo>,
    last_pruned_at: Option<DateTime<Utc>>,
    write_permission: RunsJsonWritePermission,
}

/// Reads and deserializes `runs.json.zst`, converting to the internal
/// representation.
fn read_runs_json(runs_dir: &Utf8Path) -> Result<ReadRunsJsonResult, RunStoreError> {
    let runs_json_path = runs_dir.join(RUNS_JSON_FILE_NAME);
    let file = match File::open(&runs_json_path) {
        Ok(file) => file,
        Err(error) => {
            if error.kind() == io::ErrorKind::NotFound {
                // The file doesn't exist yet, so we can write a new one.
                return Ok(ReadRunsJsonResult {
                    runs: Vec::new(),
                    last_pruned_at: None,
                    write_permission: RunsJsonWritePermission::Allowed,
                });
            } else {
                return Err(RunStoreError::RunListRead {
                    path: runs_json_path,
                    error,
                });
            }
        }
    };

    let decoder = zstd::stream::Decoder::new(file).map_err(|error| RunStoreError::RunListRead {
        path: runs_json_path.clone(),
        error,
    })?;

    let list: RecordedRunList =
        serde_json::from_reader(decoder).map_err(|error| RunStoreError::RunListDeserialize {
            path: runs_json_path,
            error,
        })?;
    let write_permission = list.write_permission();
    let data = list.into_data();
    Ok(ReadRunsJsonResult {
        runs: data.runs,
        last_pruned_at: data.last_pruned_at,
        write_permission,
    })
}

/// Serializes and writes runs.json.zst from internal representation.
fn write_runs_json(
    runs_dir: &Utf8Path,
    runs: &[RecordedRunInfo],
    last_pruned_at: Option<DateTime<Utc>>,
) -> Result<(), RunStoreError> {
    let runs_json_path = runs_dir.join(RUNS_JSON_FILE_NAME);
    let list = RecordedRunList::from_data(runs, last_pruned_at);

    atomicwrites::AtomicFile::new(&runs_json_path, atomicwrites::AllowOverwrite)
        .write(|file| {
            // Use compression level 3, consistent with other zstd usage in the crate.
            let mut encoder = zstd::stream::Encoder::new(file, 3)?;
            serde_json::to_writer_pretty(&mut encoder, &list)?;
            encoder.finish()?;
            Ok(())
        })
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

    /// Returns whether this nextest can write to the runs.json.zst file.
    ///
    /// If the file has a newer format version than we support, writing is denied.
    pub fn write_permission(&self) -> RunsJsonWritePermission {
        self.write_permission
    }

    /// Returns a list of recorded runs.
    pub fn runs(&self) -> &[RecordedRunInfo] {
        &self.runs
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
    /// For [`RunIdSelector::Latest`], returns the most recent run by start
    /// time.
    /// For [`RunIdSelector::Prefix`], resolves the prefix to a specific run.
    pub fn resolve_run_id(
        &self,
        selector: &RunIdSelector,
    ) -> Result<ResolveRunIdResult, RunIdResolutionError> {
        match selector {
            RunIdSelector::Latest => self.most_recent_run(),
            RunIdSelector::Prefix(prefix) => {
                let run_id = self.resolve_run_id_prefix(prefix)?;
                Ok(ResolveRunIdResult { run_id })
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

    /// Returns the most recent run by start time.
    ///
    /// Returns an error if there are no runs at all.
    pub fn most_recent_run(&self) -> Result<ResolveRunIdResult, RunIdResolutionError> {
        self.runs
            .iter()
            .max_by_key(|r| r.started_at)
            .map(|r| ResolveRunIdResult { run_id: r.run_id })
            .ok_or(RunIdResolutionError::NoRuns)
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

/// A snapshot paired with precomputed replayability status for all runs.
///
/// This struct maintains the invariant that every run in the snapshot has a
/// corresponding entry in the replayability map. Use [`Self::new`] to compute
/// replayability for all runs, or `Self::new_for_test` for testing.
#[derive(Debug)]
pub struct SnapshotWithReplayability<'a> {
    snapshot: &'a RunStoreSnapshot,
    replayability: HashMap<ReportUuid, ReplayabilityStatus>,
    latest_run_id: Option<ReportUuid>,
}

impl<'a> SnapshotWithReplayability<'a> {
    /// Creates a new snapshot with replayability by checking all runs.
    ///
    /// This computes [`ReplayabilityStatus`] for each run by checking file
    /// existence and format versions.
    pub fn new(snapshot: &'a RunStoreSnapshot) -> Self {
        let runs_dir = snapshot.runs_dir();
        let replayability: HashMap<_, _> = snapshot
            .runs()
            .iter()
            .map(|run| (run.run_id, run.check_replayability(runs_dir)))
            .collect();

        // Find the latest run by time.
        let latest_run_id = snapshot.most_recent_run().ok().map(|r| r.run_id);

        Self {
            snapshot,
            replayability,
            latest_run_id,
        }
    }

    /// Returns a reference to the underlying snapshot.
    pub fn snapshot(&self) -> &'a RunStoreSnapshot {
        self.snapshot
    }

    /// Returns the replayability map.
    pub fn replayability(&self) -> &HashMap<ReportUuid, ReplayabilityStatus> {
        &self.replayability
    }

    /// Returns the replayability status for a specific run.
    ///
    /// # Panics
    ///
    /// Panics if the run ID is not in the snapshot. This maintains the
    /// invariant that all runs in the snapshot have replayability computed.
    pub fn get_replayability(&self, run_id: ReportUuid) -> &ReplayabilityStatus {
        self.replayability
            .get(&run_id)
            .expect("run ID should be in replayability map")
    }

    /// Returns the ID of the most recent run by start time, if any.
    pub fn latest_run_id(&self) -> Option<ReportUuid> {
        self.latest_run_id
    }
}

#[cfg(test)]
impl SnapshotWithReplayability<'_> {
    /// Creates a snapshot with replayability for testing.
    ///
    /// All runs are marked as [`ReplayabilityStatus::Replayable`] by default.
    pub fn new_for_test(snapshot: &RunStoreSnapshot) -> SnapshotWithReplayability<'_> {
        let replayability: HashMap<_, _> = snapshot
            .runs()
            .iter()
            .map(|run| (run.run_id, ReplayabilityStatus::Replayable))
            .collect();

        // For tests, latest is just the most recent by time.
        let latest_run_id = snapshot
            .runs()
            .iter()
            .max_by_key(|r| r.started_at)
            .map(|r| r.run_id);

        SnapshotWithReplayability {
            snapshot,
            replayability,
            latest_run_id,
        }
    }
}

#[cfg(test)]
impl RunStoreSnapshot {
    /// Creates a new snapshot for testing.
    pub(crate) fn new_for_test(runs: Vec<RecordedRunInfo>) -> Self {
        use super::run_id_index::RunIdIndex;

        let run_id_index = RunIdIndex::new(&runs);
        Self {
            runs_dir: Utf8PathBuf::from("/test/runs"),
            runs,
            write_permission: RunsJsonWritePermission::Allowed,
            run_id_index,
        }
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
