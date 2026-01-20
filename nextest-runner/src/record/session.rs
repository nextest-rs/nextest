// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recording session management.
//!
//! This module provides [`RecordSession`], which encapsulates the full lifecycle of
//! a recording session: setup, integration with the reporter, and finalization.
//! This allows both `run` and `bench` commands to share recording logic.

use super::{
    CompletedRunStats, RecordedRunStatus, RunRecorder, RunStore, ShortestRunIdPrefix, StoreSizes,
    StressCompletedRunStats, records_cache_dir,
    retention::{PruneResult, RecordRetentionPolicy},
};
use crate::{
    errors::{RecordPruneError, RecordSetupError, RunStoreError},
    record::{Styles, format::RerunInfo},
    reporter::{
        RunFinishedInfo,
        events::{FinalRunStats, RunFinishedStats, StressFinalRunStats},
    },
};
use bytesize::ByteSize;
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, FixedOffset};
use owo_colors::OwoColorize;
use quick_junit::ReportUuid;
use semver::Version;
use std::{collections::BTreeMap, fmt};

/// Configuration for creating a recording session.
#[derive(Clone, Debug)]
pub struct RecordSessionConfig<'a> {
    /// The workspace root path, used to determine the cache directory.
    pub workspace_root: &'a Utf8Path,
    /// The unique identifier for this run.
    pub run_id: ReportUuid,
    /// The version of nextest creating this recording.
    pub nextest_version: Version,
    /// When the run started.
    pub started_at: DateTime<FixedOffset>,
    /// The command-line arguments used to invoke nextest.
    pub cli_args: Vec<String>,
    /// Build scope arguments (package and target selection).
    ///
    /// These determine which packages and targets are built. In a rerun chain,
    /// these are inherited from the original run unless explicitly overridden.
    pub build_scope_args: Vec<String>,
    /// Environment variables that affect nextest behavior (NEXTEST_* and CARGO_*).
    pub env_vars: BTreeMap<String, String>,
    /// Maximum size per output file before truncation.
    pub max_output_size: ByteSize,
    /// Rerun-specific metadata, if this is a rerun.
    ///
    /// If present, this will be written to `meta/rerun-info.json` in the archive.
    pub rerun_info: Option<RerunInfo>,
}

/// Result of setting up a recording session.
#[derive(Debug)]
pub struct RecordSessionSetup {
    /// The session handle for later finalization.
    pub session: RecordSession,
    /// The recorder to pass to the structured reporter.
    pub recorder: RunRecorder,
    /// The shortest unique prefix for the run ID.
    ///
    /// This can be used for display purposes to highlight the unique prefix
    /// portion of the run ID.
    pub run_id_unique_prefix: ShortestRunIdPrefix,
}

/// Manages the full lifecycle of a recording session.
///
/// This type encapsulates setup, execution integration, and finalization.
#[derive(Debug)]
pub struct RecordSession {
    cache_dir: Utf8PathBuf,
    run_id: ReportUuid,
}

impl RecordSession {
    /// Sets up a new recording session.
    ///
    /// Creates the run store, acquires an exclusive lock, and creates the
    /// recorder. The lock is released after setup completes (the recorder
    /// writes independently).
    ///
    /// Returns a setup result containing the session handle and recorder, or an
    /// error if setup fails.
    pub fn setup(config: RecordSessionConfig<'_>) -> Result<RecordSessionSetup, RecordSetupError> {
        let cache_dir =
            records_cache_dir(config.workspace_root).map_err(RecordSetupError::CacheDirNotFound)?;

        let store = RunStore::new(&cache_dir).map_err(RecordSetupError::StoreCreate)?;

        let locked_store = store
            .lock_exclusive()
            .map_err(RecordSetupError::StoreLock)?;

        let (mut recorder, run_id_unique_prefix) = locked_store
            .create_run_recorder(
                config.run_id,
                config.nextest_version,
                config.started_at,
                config.cli_args,
                config.build_scope_args,
                config.env_vars,
                config.max_output_size,
                config.rerun_info.as_ref().map(|info| info.parent_run_id),
            )
            .map_err(RecordSetupError::RecorderCreate)?;

        // If this is a rerun, write the rerun info to the archive.
        if let Some(rerun_info) = config.rerun_info {
            recorder
                .write_rerun_info(&rerun_info)
                .map_err(RecordSetupError::RecorderCreate)?;
        }

        let session = RecordSession {
            cache_dir,
            run_id: config.run_id,
        };

        Ok(RecordSessionSetup {
            session,
            recorder,
            run_id_unique_prefix,
        })
    }

    /// Returns the run ID for this session.
    pub fn run_id(&self) -> ReportUuid {
        self.run_id
    }

    /// Returns the cache directory for this session.
    pub fn cache_dir(&self) -> &Utf8Path {
        &self.cache_dir
    }

    /// Finalizes the recording session after the run completes.
    ///
    /// This method marks the run as completed with its final sizes and stats.
    ///
    /// All errors during finalization are non-fatal and returned as warnings,
    /// since the recording itself has already completed successfully.
    ///
    /// This should be called after `reporter.finish()` returns the recording sizes.
    ///
    /// The `exit_code` parameter should be the exit code that the process will
    /// return. This is stored in the run metadata for later inspection.
    pub fn finalize(
        self,
        recording_sizes: Option<StoreSizes>,
        run_finished: Option<RunFinishedInfo>,
        exit_code: i32,
        policy: &RecordRetentionPolicy,
    ) -> RecordFinalizeResult {
        let mut result = RecordFinalizeResult::default();

        // If recording didn't produce sizes, there's nothing to finalize.
        let Some(sizes) = recording_sizes else {
            return result;
        };

        // Convert run finished info to status and duration.
        let (status, duration_secs) = match run_finished {
            Some(info) => (
                convert_run_stats_to_status(info.stats, exit_code),
                Some(info.elapsed.as_secs_f64()),
            ),
            // This shouldn't happen when recording_sizes is Some, but handle gracefully.
            None => (RecordedRunStatus::Incomplete, None),
        };

        // Re-open the store and acquire the lock.
        let store = match RunStore::new(&self.cache_dir) {
            Ok(store) => store,
            Err(err) => {
                result
                    .warnings
                    .push(RecordFinalizeWarning::StoreOpenFailed(err));
                return result;
            }
        };

        let mut locked_store = match store.lock_exclusive() {
            Ok(locked) => locked,
            Err(err) => {
                result
                    .warnings
                    .push(RecordFinalizeWarning::StoreLockFailed(err));
                return result;
            }
        };

        // Mark the run as completed and persist.
        match locked_store.complete_run(self.run_id, sizes, status, duration_secs) {
            Ok(true) => {}
            Ok(false) => {
                // Run was not found in the store, likely pruned during execution.
                result
                    .warnings
                    .push(RecordFinalizeWarning::RunNotFoundDuringComplete(
                        self.run_id,
                    ));
            }
            Err(err) => {
                result
                    .warnings
                    .push(RecordFinalizeWarning::MetadataPersistFailed(err));
            }
        }
        // Continue with pruning even if metadata persistence failed.

        // Prune old runs if needed (once daily or if limits exceeded by 1.5x).
        match locked_store.prune_if_needed(policy) {
            Ok(Some(mut prune_result)) => {
                // Move any errors that occurred during pruning into warnings.
                for error in prune_result.errors.drain(..) {
                    result
                        .warnings
                        .push(RecordFinalizeWarning::PruneError(error));
                }
                result.prune_result = Some(prune_result);
            }
            Ok(None) => {
                // Pruning was skipped; nothing to do.
            }
            Err(err) => {
                result
                    .warnings
                    .push(RecordFinalizeWarning::PruneFailed(err));
            }
        }

        result
    }
}

/// Converts `RunFinishedStats` to `RecordedRunStatus`.
fn convert_run_stats_to_status(stats: RunFinishedStats, exit_code: i32) -> RecordedRunStatus {
    match stats {
        RunFinishedStats::Single(run_stats) => {
            let completed_stats = CompletedRunStats {
                initial_run_count: run_stats.initial_run_count,
                passed: run_stats.passed,
                failed: run_stats.failed_count(),
                exit_code,
            };

            // Check if the run was cancelled based on final stats.
            match run_stats.summarize_final() {
                FinalRunStats::Success
                | FinalRunStats::NoTestsRun
                | FinalRunStats::Failed { .. } => RecordedRunStatus::Completed(completed_stats),
                FinalRunStats::Cancelled { .. } => RecordedRunStatus::Cancelled(completed_stats),
            }
        }
        RunFinishedStats::Stress(stress_stats) => {
            let stress_completed_stats = StressCompletedRunStats {
                initial_iteration_count: stress_stats.completed.total,
                success_count: stress_stats.success_count,
                failed_count: stress_stats.failed_count,
                exit_code,
            };

            // Check if the stress run was cancelled.
            match stress_stats.summarize_final() {
                StressFinalRunStats::Success
                | StressFinalRunStats::NoTestsRun
                | StressFinalRunStats::Failed => {
                    RecordedRunStatus::StressCompleted(stress_completed_stats)
                }
                StressFinalRunStats::Cancelled => {
                    RecordedRunStatus::StressCancelled(stress_completed_stats)
                }
            }
        }
    }
}

/// Result of finalizing a recording session.
#[derive(Debug, Default)]
pub struct RecordFinalizeResult {
    /// Warnings encountered during finalization.
    pub warnings: Vec<RecordFinalizeWarning>,
    /// The prune result, if pruning was performed.
    pub prune_result: Option<PruneResult>,
}

impl RecordFinalizeResult {
    /// Logs warnings and pruning statistics from the finalization result.
    pub fn log(&self, styles: &Styles) {
        for warning in &self.warnings {
            tracing::warn!("{warning}");
        }

        if let Some(prune_result) = &self.prune_result
            && (prune_result.deleted_count > 0 || prune_result.orphans_deleted > 0)
        {
            tracing::info!(
                "{}(hint: {} to replay runs)",
                prune_result.display(styles),
                "cargo nextest replay".style(styles.count),
            );
        }
    }
}

/// Non-fatal warning during recording finalization.
#[derive(Debug)]
pub enum RecordFinalizeWarning {
    /// Recording completed but the run store couldn't be opened.
    StoreOpenFailed(RunStoreError),
    /// Recording completed but the run store couldn't be locked.
    StoreLockFailed(RunStoreError),
    /// Recording completed but run metadata couldn't be persisted.
    MetadataPersistFailed(RunStoreError),
    /// Recording completed but the run was not found in the store.
    ///
    /// This can happen if an aggressive prune deleted the run while the test
    /// was still executing.
    RunNotFoundDuringComplete(ReportUuid),
    /// Error during pruning (overall prune operation failed).
    PruneFailed(RunStoreError),
    /// Error during pruning (individual run or orphan deletion failed).
    PruneError(RecordPruneError),
}

impl fmt::Display for RecordFinalizeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreOpenFailed(err) => {
                write!(f, "recording completed but failed to open run store: {err}")
            }
            Self::StoreLockFailed(err) => {
                write!(f, "recording completed but failed to lock run store: {err}")
            }
            Self::MetadataPersistFailed(err) => {
                write!(
                    f,
                    "recording completed but failed to persist run metadata: {err}"
                )
            }
            Self::RunNotFoundDuringComplete(run_id) => {
                write!(
                    f,
                    "recording completed but run {run_id} was not found in store \
                     (may have been pruned during execution)"
                )
            }
            Self::PruneFailed(err) => write!(f, "error during prune: {err}"),
            Self::PruneError(msg) => write!(f, "error during prune: {msg}"),
        }
    }
}
