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
//! - [`records_state_dir`]: Returns the platform-specific state directory for recordings.
//!
//! # Archive format
//!
//! Each run is stored in a directory named by its UUID, containing:
//!
//! - `store.zip`: A zstd-compressed archive containing metadata and test outputs.
//! - `run.log.zst`: A zstd-compressed JSON Lines file of test events.

pub mod dicts;
mod display;
mod format;
mod portable;
mod reader;
mod recorder;
pub mod replay;
mod rerun;
mod retention;
mod run_id_index;
mod session;
mod state_dir;
mod store;
mod summary;
#[cfg(test)]
mod test_helpers;
mod tree;

pub use display::{
    DisplayPrunePlan, DisplayPruneResult, DisplayRecordedRunInfo, DisplayRecordedRunInfoDetailed,
    DisplayRunList, RunListAlignment, Styles,
};
pub use format::{
    CARGO_METADATA_JSON_PATH, OutputDict, PORTABLE_MANIFEST_FILE_NAME,
    PortableRecordingFormatVersion, PortableRecordingVersionIncompatibility, RECORD_OPTS_JSON_PATH,
    RERUN_INFO_JSON_PATH, RUN_LOG_FILE_NAME, RerunInfo, RerunRootInfo, RerunTestSuiteInfo,
    RunsJsonFormatVersion, RunsJsonWritePermission, STDERR_DICT_PATH, STDOUT_DICT_PATH,
    STORE_FORMAT_VERSION, STORE_ZIP_FILE_NAME, StoreFormatMajorVersion, StoreFormatMinorVersion,
    StoreFormatVersion, StoreVersionIncompatibility, TEST_LIST_JSON_PATH, has_zip_extension,
};
pub use portable::{
    ExtractOuterFileResult, PortableRecording, PortableRecordingEventIter, PortableRecordingResult,
    PortableRecordingRunLog, PortableRecordingWriter, PortableStoreReader,
};
pub use reader::{RecordEventIter, RecordReader, StoreReader};
pub use recorder::{RunRecorder, StoreSizes};
pub use replay::{
    ReplayContext, ReplayConversionError, ReplayHeader, ReplayReporter, ReplayReporterBuilder,
};
pub use rerun::ComputedRerunInfo;
pub use retention::{PruneKind, PrunePlan, PruneResult, RecordRetentionPolicy};
pub use run_id_index::{RunIdIndex, RunIdOrRecordingSelector, RunIdSelector, ShortestRunIdPrefix};
pub use session::{
    RecordFinalizeResult, RecordFinalizeWarning, RecordSession, RecordSessionConfig,
    RecordSessionSetup,
};
pub use state_dir::{NEXTEST_STATE_DIR_ENV, encode_workspace_path, records_state_dir};
pub use store::{
    CompletedRunStats, ComponentSizes, ExclusiveLockedRunStore, NonReplayableReason,
    RecordedRunInfo, RecordedRunStatus, RecordedSizes, ReplayabilityStatus, ResolveRunIdResult,
    RunFilesExist, RunStore, RunStoreSnapshot, SharedLockedRunStore, SnapshotWithReplayability,
    StoreRunFiles, StoreRunsDir, StressCompletedRunStats,
};
pub use summary::{
    CoreEventKind, OutputEventKind, OutputFileName, RecordOpts, StressConditionSummary,
    StressIndexSummary, TestEventKindSummary, TestEventSummary, ZipStoreOutput,
};
