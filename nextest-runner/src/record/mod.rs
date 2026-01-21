// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recording infrastructure for nextest runs.
//!
//! This module provides functionality to record test runs to disk for later inspection
//! and replay. The recording captures test events, outputs, and metadata in a structured
//! archive format.
//!
//! # Architecture
//!
//! The recording system consists of:
//!
//! - [`RunStore`]: Manages the directory where recordings are stored, handling locking
//!   and the list of recorded runs.
//! - [`RunRecorder`]: Writes a single run's data to disk, including metadata and events.
//! - [`RecordReader`]: Reads a recorded run from disk for replay or inspection.
//! - [`records_cache_dir`]: Returns the platform-specific cache directory for recordings.
//!
//! # Archive format
//!
//! Each run is stored in a directory named by its UUID, containing:
//!
//! - `store.zip`: A zstd-compressed archive containing metadata and test outputs.
//! - `run.log.zst`: A zstd-compressed JSON Lines file of test events.

mod cache_dir;
pub mod dicts;
mod display;
pub mod format;
mod reader;
mod recorder;
pub mod replay;
mod rerun;
mod retention;
mod run_id_index;
mod session;
mod store;
mod summary;
#[cfg(test)]
pub(crate) mod test_helpers;
mod tree;

pub use cache_dir::{NEXTEST_CACHE_DIR_ENV, records_cache_dir};
pub use display::{
    DisplayPrunePlan, DisplayPruneResult, DisplayRecordedRunInfo, DisplayRecordedRunInfoDetailed,
    DisplayRunList, RunListAlignment, Styles,
};
pub use format::RunsJsonWritePermission;
pub use reader::{RecordEventIter, RecordReader};
pub use recorder::{RunRecorder, StoreSizes};
pub use replay::{
    ReplayContext, ReplayConversionError, ReplayHeader, ReplayReporter, ReplayReporterBuilder,
};
pub use rerun::ComputedRerunInfo;
pub use retention::{PruneKind, PrunePlan, PruneResult, RecordRetentionPolicy};
pub use run_id_index::{RunIdIndex, RunIdSelector, ShortestRunIdPrefix};
pub use session::{
    RecordFinalizeResult, RecordFinalizeWarning, RecordSession, RecordSessionConfig,
    RecordSessionSetup,
};
pub use store::{
    CompletedRunStats, ComponentSizes, ExclusiveLockedRunStore, NonReplayableReason,
    RecordedRunInfo, RecordedRunStatus, RecordedSizes, ReplayabilityStatus, ResolveRunIdResult,
    RunStore, RunStoreSnapshot, SharedLockedRunStore, SnapshotWithReplayability, StoreRunsDir,
    StressCompletedRunStats,
};
pub use summary::{
    CoreEventKind, OutputEventKind, OutputFileName, RecordOpts, StressConditionSummary,
    StressIndexSummary, TestEventKindSummary, TestEventSummary, ZipStoreOutput,
};
