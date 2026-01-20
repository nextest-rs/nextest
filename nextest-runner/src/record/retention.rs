// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Retention policies and pruning for recorded test runs.
//!
//! This module provides the [`RecordRetentionPolicy`] type for configuring
//! retention limits, and functions for determining which runs should be deleted
//! to enforce those limits.

use super::{
    display::{DisplayPrunePlan, DisplayPruneResult, Styles},
    run_id_index::RunIdIndex,
    store::StoreRunsDir,
};
use crate::{
    errors::RecordPruneError, record::RecordedRunInfo, redact::Redactor,
    user_config::elements::RecordConfig,
};
use bytesize::ByteSize;
use chrono::{DateTime, TimeDelta, Utc};
use quick_junit::ReportUuid;
use std::{collections::HashSet, time::Duration};

/// A retention policy for recorded test runs.
///
/// The policy enforces limits on the number of runs, total size, and age.
/// All limits are optional; if unset, that dimension is not limited.
///
/// When pruning, runs are evaluated in order from most recently used to least
/// recently used (by `last_written_at`). A run is kept if it satisfies all
/// three conditions:
///
/// 1. The total count of kept runs is below `max_count`.
/// 2. The cumulative size of kept runs is below `max_total_size`.
/// 3. The run was used more recently than `max_age`.
///
/// Incomplete runs are treated the same as complete runs for retention purposes.
#[derive(Clone, Debug, Default)]
pub struct RecordRetentionPolicy {
    /// Maximum number of runs to keep.
    pub max_count: Option<usize>,

    /// Maximum total size of all runs in bytes.
    pub max_total_size: Option<ByteSize>,

    /// Maximum age of runs to keep.
    pub max_age: Option<Duration>,
}

impl RecordRetentionPolicy {
    /// Computes which runs should be deleted according to this policy.
    ///
    /// Returns a list of run IDs that should be deleted. The order of the
    /// returned IDs is not specified.
    ///
    /// The `now` parameter is used to calculate run ages. Typically this should
    /// be `Utc::now()`.
    pub(crate) fn compute_runs_to_delete(
        &self,
        runs: &[RecordedRunInfo],
        now: DateTime<Utc>,
    ) -> Vec<ReportUuid> {
        // Sort by last_written_at (most recently used first) for LRU eviction.
        let mut sorted_runs: Vec<_> = runs.iter().collect();
        sorted_runs.sort_by(|a, b| b.last_written_at.cmp(&a.last_written_at));

        let mut to_delete = Vec::new();
        let mut kept_count = 0usize;
        let mut kept_size = 0u64;

        for run in sorted_runs {
            let mut should_delete = false;

            if let Some(max_count) = self.max_count
                && kept_count >= max_count
            {
                should_delete = true;
            }

            if let Some(max_total_size) = self.max_total_size
                && kept_size + run.sizes.total_compressed() > max_total_size.as_u64()
            {
                should_delete = true;
            }

            if let Some(max_age) = self.max_age {
                // Use signed_duration_since and saturate negative values to zero. A
                // negative age can occur if the system clock moved backward between
                // recording and pruning; treat such runs as "just used".
                let time_since_last_use = now
                    .signed_duration_since(run.last_written_at)
                    .max(TimeDelta::zero());
                if time_since_last_use > TimeDelta::from_std(max_age).unwrap_or(TimeDelta::MAX) {
                    should_delete = true;
                }
            }

            if should_delete {
                to_delete.push(run.run_id);
            } else {
                kept_count += 1;
                kept_size += run.sizes.total_compressed();
            }
        }

        to_delete
    }

    /// Checks if any retention limit is exceeded by the given factor.
    ///
    /// Returns `true` if:
    /// - The run count exceeds `factor * max_count`, OR
    /// - The total size exceeds `factor * max_total_size`.
    ///
    /// Age limits are not checked here; they are handled by daily pruning.
    ///
    /// This is used to trigger pruning when limits are significantly exceeded,
    /// even if the daily prune interval hasn't elapsed.
    pub(crate) fn limits_exceeded_by_factor(&self, runs: &[RecordedRunInfo], factor: f64) -> bool {
        // Check count limit.
        if let Some(max_count) = self.max_count {
            let threshold = (max_count as f64 * factor) as usize;
            if runs.len() > threshold {
                return true;
            }
        }

        // Check size limit.
        if let Some(max_total_size) = self.max_total_size {
            let total_size: u64 = runs.iter().map(|r| r.sizes.total_compressed()).sum();
            let threshold = (max_total_size.as_u64() as f64 * factor) as u64;
            if total_size > threshold {
                return true;
            }
        }

        false
    }
}

impl From<&RecordConfig> for RecordRetentionPolicy {
    fn from(config: &RecordConfig) -> Self {
        Self {
            max_count: Some(config.max_records),
            max_total_size: Some(config.max_total_size),
            max_age: Some(config.max_age),
        }
    }
}

/// Whether pruning was explicit (user-requested) or implicit (automatic).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PruneKind {
    /// Explicit pruning via `cargo nextest store prune`.
    #[default]
    Explicit,

    /// Implicit pruning during recording (via `prune_if_needed`).
    Implicit,
}

/// The result of a pruning operation.
#[derive(Debug, Default)]
pub struct PruneResult {
    /// Whether this was explicit or implicit pruning.
    pub kind: PruneKind,

    /// Number of runs that were deleted.
    pub deleted_count: usize,

    /// Number of orphaned directories that were deleted.
    ///
    /// Orphaned directories are run directories that exist on disk but are not
    /// tracked in `runs.json.zst`. This can happen if a process crashes or is killed
    /// after creating a run directory but before recording completes.
    pub orphans_deleted: usize,

    /// Total bytes freed by the deletion.
    pub freed_bytes: u64,

    /// Errors that occurred during pruning.
    ///
    /// Pruning continues despite individual errors, so this list may contain
    /// multiple entries.
    pub errors: Vec<RecordPruneError>,
}

impl PruneResult {
    /// Returns a display wrapper for the prune result.
    pub fn display<'a>(&'a self, styles: &'a Styles) -> DisplayPruneResult<'a> {
        DisplayPruneResult {
            result: self,
            styles,
        }
    }
}

/// The result of computing a prune plan (dry-run mode).
///
/// Contains information about which runs would be deleted if the prune
/// operation were executed.
#[derive(Clone, Debug)]
pub struct PrunePlan {
    /// Runs that would be deleted.
    runs: Vec<RecordedRunInfo>,
}

impl PrunePlan {
    /// Creates a new prune plan from a list of runs to delete.
    ///
    /// The runs are sorted by start time (oldest first).
    pub(crate) fn new(mut runs: Vec<RecordedRunInfo>) -> Self {
        runs.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        Self { runs }
    }

    pub(super) fn compute(runs: &[RecordedRunInfo], policy: &RecordRetentionPolicy) -> Self {
        let now = Utc::now();
        let to_delete: HashSet<_> = policy
            .compute_runs_to_delete(runs, now)
            .into_iter()
            .collect();

        let runs_to_delete: Vec<_> = runs
            .iter()
            .filter(|r| to_delete.contains(&r.run_id))
            .cloned()
            .collect();

        Self::new(runs_to_delete)
    }

    /// Returns the runs that would be deleted.
    pub fn runs(&self) -> &[RecordedRunInfo] {
        &self.runs
    }

    /// Returns the number of runs that would be deleted.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Returns the total size in bytes of runs that would be deleted.
    pub fn total_bytes(&self) -> u64 {
        self.runs.iter().map(|r| r.sizes.total_compressed()).sum()
    }

    /// Returns a display wrapper for the prune plan.
    ///
    /// The `run_id_index` is used for computing shortest unique prefixes,
    /// which are highlighted differently in the output (similar to jj).
    ///
    /// The `redactor` parameter, if provided, redacts timestamps, durations,
    /// and sizes for snapshot testing while preserving column alignment.
    pub fn display<'a>(
        &'a self,
        run_id_index: &'a RunIdIndex,
        styles: &'a Styles,
        redactor: &'a Redactor,
    ) -> DisplayPrunePlan<'a> {
        DisplayPrunePlan {
            plan: self,
            run_id_index,
            styles,
            redactor,
        }
    }
}

/// Deletes run directories and updates the run list.
///
/// This function:
/// 1. Deletes each run directory from disk
/// 2. Removes deleted runs from the provided list
/// 3. Returns statistics about the operation
///
/// Deletion continues even if individual runs fail to delete. Errors are
/// collected in the returned `PruneResult`.
pub(crate) fn delete_runs(
    runs_dir: StoreRunsDir<'_>,
    runs: &mut Vec<RecordedRunInfo>,
    to_delete: &HashSet<ReportUuid>,
) -> PruneResult {
    let mut result = PruneResult::default();

    for run_id in to_delete {
        let run_dir = runs_dir.run_dir(*run_id);

        let size_bytes = runs
            .iter()
            .find(|r| &r.run_id == run_id)
            .map(|r| r.sizes.total_compressed())
            .unwrap_or(0);

        match std::fs::remove_dir_all(&run_dir) {
            Ok(()) => {
                result.deleted_count += 1;
                result.freed_bytes += size_bytes;
            }
            Err(error) => {
                // Don't treat "not found" as an error - the directory may have
                // already been deleted or never created.
                if error.kind() != std::io::ErrorKind::NotFound {
                    result.errors.push(RecordPruneError::DeleteRun {
                        run_id: *run_id,
                        path: run_dir,
                        error,
                    });
                } else {
                    // Still count it as deleted since it's no longer present.
                    result.deleted_count += 1;
                    result.freed_bytes += size_bytes;
                }
            }
        }
    }

    runs.retain(|run| !to_delete.contains(&run.run_id));

    result
}

/// Deletes orphaned run directories that are not tracked in runs.json.zst.
///
/// An orphaned directory is one that exists on disk but whose UUID is not
/// present in the `known_runs` set. This can happen if a process crashes
/// after creating a run directory but before the run completes.
pub(crate) fn delete_orphaned_dirs(
    runs_dir: StoreRunsDir<'_>,
    known_runs: &HashSet<ReportUuid>,
    result: &mut PruneResult,
) {
    let runs_path = runs_dir.as_path();
    let entries = match runs_path.read_dir_utf8() {
        Ok(entries) => entries,
        Err(error) => {
            // If we can't read the directory, record the error but don't fail.
            if error.kind() != std::io::ErrorKind::NotFound {
                result.errors.push(RecordPruneError::ReadRunsDir {
                    path: runs_path.to_owned(),
                    error,
                });
            }
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result.errors.push(RecordPruneError::ReadDirEntry {
                    dir: runs_path.to_owned(),
                    error,
                });
                continue;
            }
        };

        let entry_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                result.errors.push(RecordPruneError::ReadFileType {
                    path: entry_path.to_owned(),
                    error,
                });
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        let dir_name = entry.file_name();
        let run_id = match dir_name.parse::<ReportUuid>() {
            Ok(id) => id,
            // Not a UUID directory, skip without error.
            Err(_) => continue,
        };

        if known_runs.contains(&run_id) {
            continue;
        }

        let path = runs_dir.run_dir(run_id);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                result.orphans_deleted += 1;
            }
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    result
                        .errors
                        .push(RecordPruneError::DeleteOrphan { path, error });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        CompletedRunStats, ComponentSizes, RecordedRunStatus, RecordedSizes,
        format::RECORD_FORMAT_VERSION,
    };
    use chrono::{FixedOffset, TimeZone};
    use semver::Version;
    use std::collections::BTreeMap;

    fn make_run(
        run_id: ReportUuid,
        started_at: DateTime<FixedOffset>,
        total_compressed_size: u64,
        status: RecordedRunStatus,
    ) -> RecordedRunInfo {
        // For simplicity in tests, put all size in the store component.
        RecordedRunInfo {
            run_id,
            store_format_version: RECORD_FORMAT_VERSION,
            nextest_version: Version::new(0, 1, 0),
            started_at,
            last_written_at: started_at,
            duration_secs: Some(1.0),
            cli_args: Vec::new(),
            build_scope_args: Vec::new(),
            env_vars: BTreeMap::new(),
            parent_run_id: None,
            sizes: RecordedSizes {
                log: ComponentSizes::default(),
                store: ComponentSizes {
                    compressed: total_compressed_size,
                    // Use a fixed ratio for uncompressed size in tests.
                    uncompressed: total_compressed_size * 3,
                    entries: 0,
                },
            },
            status,
        }
    }

    /// Creates a simple completed status for tests.
    fn completed_status() -> RecordedRunStatus {
        RecordedRunStatus::Completed(CompletedRunStats {
            initial_run_count: 10,
            passed: 10,
            failed: 0,
            exit_code: 0,
        })
    }

    /// Creates an incomplete status for tests.
    fn incomplete_status() -> RecordedRunStatus {
        RecordedRunStatus::Incomplete
    }

    // Test time helpers. The base time is arbitrary; what matters is the
    // relative offsets between run start times and "now".
    const BASE_YEAR: i32 = 2024;

    /// Creates a run start time at base + offset seconds.
    fn run_start_time(secs_offset: i64) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(BASE_YEAR, 1, 1, 0, 0, 0)
            .unwrap()
            + chrono::Duration::seconds(secs_offset)
    }

    /// Returns the simulated "current time" for tests: 60 days after the base.
    fn now_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(BASE_YEAR, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::days(60)
    }

    #[test]
    fn test_no_limits_keeps_all() {
        let policy = RecordRetentionPolicy::default();
        let runs = vec![
            make_run(
                ReportUuid::new_v4(),
                run_start_time(0),
                1000,
                completed_status(),
            ),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(100),
                2000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert!(to_delete.is_empty());
    }

    #[test]
    fn test_incomplete_runs_not_automatically_deleted() {
        let policy = RecordRetentionPolicy {
            max_count: Some(2),
            ..Default::default()
        };

        let oldest_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(oldest_id, run_start_time(0), 1000, completed_status()),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(100),
                2000,
                incomplete_status(),
            ), // incomplete
            make_run(
                ReportUuid::new_v4(),
                run_start_time(200),
                1000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0], oldest_id);
    }

    #[test]
    fn test_count_limit() {
        let policy = RecordRetentionPolicy {
            max_count: Some(2),
            ..Default::default()
        };

        let oldest_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(oldest_id, run_start_time(0), 1000, completed_status()),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(100),
                1000,
                completed_status(),
            ),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(200),
                1000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0], oldest_id);
    }

    #[test]
    fn test_size_limit() {
        let policy = RecordRetentionPolicy {
            max_total_size: Some(ByteSize::b(2500)),
            ..Default::default()
        };

        let oldest_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(oldest_id, run_start_time(0), 1000, completed_status()),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(100),
                1000,
                completed_status(),
            ),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(200),
                1000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0], oldest_id);
    }

    #[test]
    fn test_age_limit() {
        let policy = RecordRetentionPolicy {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)), // 30 days
            ..Default::default()
        };

        let old_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(old_id, run_start_time(0), 1000, completed_status()), // 60 days old.
            make_run(
                ReportUuid::new_v4(),
                run_start_time(45 * 24 * 60 * 60), // 15 days old.
                1000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0], old_id);
    }

    #[test]
    fn test_combined_limits() {
        let policy = RecordRetentionPolicy {
            max_count: Some(2),
            max_total_size: Some(ByteSize::b(2500)),
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
        };

        let old_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(old_id, run_start_time(0), 1000, completed_status()), // 60 days old.
            make_run(
                ReportUuid::new_v4(),
                run_start_time(45 * 24 * 60 * 60), // 15 days old.
                1000,
                completed_status(),
            ),
            make_run(
                ReportUuid::new_v4(),
                run_start_time(50 * 24 * 60 * 60), // 10 days old.
                1000,
                completed_status(),
            ),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0], old_id);
    }

    #[test]
    fn test_from_record_config() {
        let config = RecordConfig {
            enabled: true,
            max_records: 50,
            max_total_size: ByteSize::gb(2),
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            max_output_size: ByteSize::mb(10),
        };

        let policy = RecordRetentionPolicy::from(&config);

        assert_eq!(policy.max_count, Some(50));
        assert_eq!(policy.max_total_size, Some(ByteSize::gb(2)));
        assert_eq!(policy.max_age, Some(Duration::from_secs(7 * 24 * 60 * 60)));
    }

    #[test]
    fn test_all_runs_deleted_by_age() {
        let policy = RecordRetentionPolicy {
            max_age: Some(Duration::from_secs(7 * 24 * 60 * 60)), // 7 days
            ..Default::default()
        };

        let id1 = ReportUuid::new_v4();
        let id2 = ReportUuid::new_v4();
        let id3 = ReportUuid::new_v4();
        let runs = vec![
            make_run(id1, run_start_time(0), 1000, completed_status()), // 60 days old
            make_run(id2, run_start_time(100), 1000, completed_status()), // 60 days old
            make_run(id3, run_start_time(200), 1000, completed_status()), // 60 days old
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(to_delete.len(), 3, "all runs should be deleted");
        assert!(to_delete.contains(&id1));
        assert!(to_delete.contains(&id2));
        assert!(to_delete.contains(&id3));
    }

    #[test]
    fn test_all_runs_deleted_by_count_zero() {
        let policy = RecordRetentionPolicy {
            max_count: Some(0),
            ..Default::default()
        };

        let id1 = ReportUuid::new_v4();
        let id2 = ReportUuid::new_v4();
        let runs = vec![
            make_run(id1, run_start_time(0), 1000, completed_status()),
            make_run(id2, run_start_time(100), 1000, completed_status()),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(
            to_delete.len(),
            2,
            "all runs should be deleted with max_count=0"
        );
        assert!(to_delete.contains(&id1));
        assert!(to_delete.contains(&id2));
    }

    #[test]
    fn test_all_runs_deleted_by_size() {
        let policy = RecordRetentionPolicy {
            max_total_size: Some(ByteSize::b(0)),
            ..Default::default()
        };

        let id1 = ReportUuid::new_v4();
        let id2 = ReportUuid::new_v4();
        let runs = vec![
            make_run(id1, run_start_time(0), 1000, completed_status()),
            make_run(id2, run_start_time(100), 1000, completed_status()),
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert_eq!(
            to_delete.len(),
            2,
            "all runs should be deleted with max_total_size=0"
        );
        assert!(to_delete.contains(&id1));
        assert!(to_delete.contains(&id2));
    }

    #[test]
    fn test_empty_runs_list() {
        let policy = RecordRetentionPolicy {
            max_count: Some(5),
            max_total_size: Some(ByteSize::mb(100)),
            max_age: Some(Duration::from_secs(7 * 24 * 60 * 60)),
        };

        let runs: Vec<RecordedRunInfo> = vec![];
        let to_delete = policy.compute_runs_to_delete(&runs, now_time());
        assert!(
            to_delete.is_empty(),
            "empty input should return empty output"
        );
    }

    #[test]
    fn test_clock_skew_negative_age_saturates_to_zero() {
        // If the system clock moved backward between recording and pruning, the
        // run's start time could be in the "future" relative to now. This should
        // be treated as age 0 (just created), not as a negative age.
        let policy = RecordRetentionPolicy {
            max_age: Some(Duration::from_secs(7 * 24 * 60 * 60)), // 7 days
            ..Default::default()
        };

        // Create a run that started "in the future" (1 day after now_time).
        let future_start = run_start_time(61 * 24 * 60 * 60); // 61 days after base = 1 day after now_time
        let future_id = ReportUuid::new_v4();

        // Also include a legitimately old run for comparison.
        let old_id = ReportUuid::new_v4();
        let runs = vec![
            make_run(old_id, run_start_time(0), 1000, completed_status()), // 60 days old, should be deleted
            make_run(future_id, future_start, 1000, completed_status()), // "future" run, should be kept
        ];

        let to_delete = policy.compute_runs_to_delete(&runs, now_time());

        // The old run should be deleted (exceeds 7 day limit).
        // The "future" run should be kept (age saturates to 0).
        assert_eq!(to_delete.len(), 1, "only old run should be deleted");
        assert_eq!(to_delete[0], old_id);
        assert!(
            !to_delete.contains(&future_id),
            "future run should be kept due to clock skew handling"
        );
    }

    #[test]
    fn test_limits_exceeded_by_factor() {
        // Test count limit.
        let count_policy = RecordRetentionPolicy {
            max_count: Some(10),
            ..Default::default()
        };

        // 15 runs is exactly 1.5x the limit of 10.
        let runs_15: Vec<_> = (0..15)
            .map(|i| {
                make_run(
                    ReportUuid::new_v4(),
                    run_start_time(i),
                    100,
                    completed_status(),
                )
            })
            .collect();

        // At exactly 1.5x, should not be exceeded (we use > not >=).
        assert!(
            !count_policy.limits_exceeded_by_factor(&runs_15, 1.5),
            "15 runs should not exceed 1.5x limit of 10"
        );

        // 16 runs exceeds 1.5x the limit of 10.
        let mut runs_16 = runs_15.clone();
        runs_16.push(make_run(
            ReportUuid::new_v4(),
            run_start_time(16),
            100,
            completed_status(),
        ));
        assert!(
            count_policy.limits_exceeded_by_factor(&runs_16, 1.5),
            "16 runs should exceed 1.5x limit of 10"
        );

        // Test size limit.
        let size_policy = RecordRetentionPolicy {
            max_total_size: Some(ByteSize::b(1000)),
            ..Default::default()
        };

        // 1500 bytes is exactly 1.5x the limit of 1000.
        let runs_1500 = vec![make_run(
            ReportUuid::new_v4(),
            run_start_time(0),
            1500,
            completed_status(),
        )];
        assert!(
            !size_policy.limits_exceeded_by_factor(&runs_1500, 1.5),
            "1500 bytes should not exceed 1.5x limit of 1000"
        );

        // 1501 bytes exceeds 1.5x the limit of 1000.
        let runs_1501 = vec![make_run(
            ReportUuid::new_v4(),
            run_start_time(0),
            1501,
            completed_status(),
        )];
        assert!(
            size_policy.limits_exceeded_by_factor(&runs_1501, 1.5),
            "1501 bytes should exceed 1.5x limit of 1000"
        );

        // Test no limits set.
        let no_limits_policy = RecordRetentionPolicy::default();
        let many_runs: Vec<_> = (0..100)
            .map(|i| {
                make_run(
                    ReportUuid::new_v4(),
                    run_start_time(i),
                    1_000_000,
                    completed_status(),
                )
            })
            .collect();
        assert!(
            !no_limits_policy.limits_exceeded_by_factor(&many_runs, 1.5),
            "no limits set should never be exceeded"
        );

        // Test empty runs.
        let runs_empty: Vec<RecordedRunInfo> = vec![];
        assert!(
            !count_policy.limits_exceeded_by_factor(&runs_empty, 1.5),
            "empty runs should not exceed limits"
        );
    }
}
