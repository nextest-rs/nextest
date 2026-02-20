// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recording format metadata shared between recorder and reader.

use super::{
    CompletedRunStats, ComponentSizes, RecordedRunInfo, RecordedRunStatus, RecordedSizes,
    StressCompletedRunStats,
};
use camino::Utf8Path;
use chrono::{DateTime, FixedOffset, Utc};
use eazip::{CompressionMethod, write::FileOptions};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_metadata::{RustBinaryId, TestCaseName};
use quick_junit::ReportUuid;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    num::NonZero,
};

// ---
// Format version newtypes
// ---

/// Defines a newtype wrapper around `u32` for format versions.
///
/// Use `@default` variant to also derive `Default` (defaults to 0).
macro_rules! define_format_version {
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
        #[serde(transparent)]
        $vis struct $name(u32);

        impl $name {
            #[doc = concat!("Creates a new `", stringify!($name), "`.")]
            pub const fn new(version: u32) -> Self {
                Self(version)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };

    (
        @default
        $(#[$attr:meta])*
        $vis:vis struct $name:ident;
    ) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
        #[serde(transparent)]
        $vis struct $name(u32);

        impl $name {
            #[doc = concat!("Creates a new `", stringify!($name), "`.")]
            pub const fn new(version: u32) -> Self {
                Self(version)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

define_format_version! {
    /// Version of the `runs.json.zst` outer format.
    ///
    /// Increment this when adding new semantically important fields to `runs.json.zst`.
    /// Readers can read newer versions (assuming append-only evolution with serde
    /// defaults), but writers must refuse to write if the file version is higher
    /// than this.
    pub struct RunsJsonFormatVersion;
}

define_format_version! {
    /// Major version of the `store.zip` archive format for breaking changes to the
    /// archive structure.
    pub struct StoreFormatMajorVersion;
}

define_format_version! {
    @default
    /// Minor version of the `store.zip` archive format for additive changes.
    pub struct StoreFormatMinorVersion;
}

/// Combined major and minor version of the `store.zip` archive format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreFormatVersion {
    /// The major version (breaking changes).
    pub major: StoreFormatMajorVersion,
    /// The minor version (additive changes).
    pub minor: StoreFormatMinorVersion,
}

impl StoreFormatVersion {
    /// Creates a new `StoreFormatVersion`.
    pub const fn new(major: StoreFormatMajorVersion, minor: StoreFormatMinorVersion) -> Self {
        Self { major, minor }
    }

    /// Checks if an archive with version `self` can be read by a reader that
    /// supports `supported`.
    pub fn check_readable_by(self, supported: Self) -> Result<(), StoreVersionIncompatibility> {
        if self.major != supported.major {
            return Err(StoreVersionIncompatibility::MajorMismatch {
                archive_major: self.major,
                supported_major: supported.major,
            });
        }
        if self.minor > supported.minor {
            return Err(StoreVersionIncompatibility::MinorTooNew {
                archive_minor: self.minor,
                supported_minor: supported.minor,
            });
        }
        Ok(())
    }
}

impl fmt::Display for StoreFormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// An incompatibility between an archive's store format version and what the
/// reader supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreVersionIncompatibility {
    /// The archive's major version differs from the supported major version.
    MajorMismatch {
        /// The major version in the archive.
        archive_major: StoreFormatMajorVersion,
        /// The major version this nextest supports.
        supported_major: StoreFormatMajorVersion,
    },
    /// The archive's minor version is newer than the supported minor version.
    MinorTooNew {
        /// The minor version in the archive.
        archive_minor: StoreFormatMinorVersion,
        /// The maximum minor version this nextest supports.
        supported_minor: StoreFormatMinorVersion,
    },
}

impl fmt::Display for StoreVersionIncompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MajorMismatch {
                archive_major,
                supported_major,
            } => {
                write!(
                    f,
                    "major version {} differs from supported version {}",
                    archive_major, supported_major
                )
            }
            Self::MinorTooNew {
                archive_minor,
                supported_minor,
            } => {
                write!(
                    f,
                    "minor version {} is newer than supported version {}",
                    archive_minor, supported_minor
                )
            }
        }
    }
}

// ---
// runs.json.zst format types
// ---

/// The current format version for runs.json.zst.
pub(super) const RUNS_JSON_FORMAT_VERSION: RunsJsonFormatVersion = RunsJsonFormatVersion::new(2);

/// The current format version for recorded test runs (store.zip and run.log).
///
/// This combines a major version (for breaking changes) and a minor version
/// (for additive changes). Readers check compatibility via
/// [`StoreFormatVersion::check_readable_by`].
pub const STORE_FORMAT_VERSION: StoreFormatVersion = StoreFormatVersion::new(
    StoreFormatMajorVersion::new(1),
    StoreFormatMinorVersion::new(0),
);

/// Whether a runs.json.zst file can be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunsJsonWritePermission {
    /// Writing is allowed.
    Allowed,
    /// Writing is not allowed because the file has a newer format version.
    Denied {
        /// The format version in the file.
        file_version: RunsJsonFormatVersion,
        /// The maximum version this nextest can write.
        max_supported_version: RunsJsonFormatVersion,
    },
}

/// The list of recorded runs (serialization format for runs.json.zst).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RecordedRunList {
    /// The format version of this file.
    pub(super) format_version: RunsJsonFormatVersion,

    /// When the store was last pruned.
    ///
    /// Used to implement once-daily implicit pruning. Explicit pruning via CLI
    /// always runs regardless of this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_pruned_at: Option<DateTime<Utc>>,

    /// The list of runs.
    #[serde(default)]
    pub(super) runs: Vec<RecordedRun>,
}

/// Data extracted from a `RecordedRunList`.
pub(super) struct RunListData {
    pub(super) runs: Vec<RecordedRunInfo>,
    pub(super) last_pruned_at: Option<DateTime<Utc>>,
}

impl RecordedRunList {
    /// Creates a new, empty run list with the current format version.
    #[cfg(test)]
    fn new() -> Self {
        Self {
            format_version: RUNS_JSON_FORMAT_VERSION,
            last_pruned_at: None,
            runs: Vec::new(),
        }
    }

    /// Converts the serialization format to internal representation.
    pub(super) fn into_data(self) -> RunListData {
        RunListData {
            runs: self.runs.into_iter().map(RecordedRunInfo::from).collect(),
            last_pruned_at: self.last_pruned_at,
        }
    }

    /// Creates a serialization format from internal representation.
    ///
    /// Always uses the current format version. If the file had an older version,
    /// this effectively upgrades it when written back.
    pub(super) fn from_data(
        runs: &[RecordedRunInfo],
        last_pruned_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            format_version: RUNS_JSON_FORMAT_VERSION,
            last_pruned_at,
            runs: runs.iter().map(RecordedRun::from).collect(),
        }
    }

    /// Returns whether this runs.json.zst can be written to by this nextest version.
    ///
    /// If the file has a newer format version than we support, writing is denied
    /// to avoid data loss.
    pub(super) fn write_permission(&self) -> RunsJsonWritePermission {
        if self.format_version > RUNS_JSON_FORMAT_VERSION {
            RunsJsonWritePermission::Denied {
                file_version: self.format_version,
                max_supported_version: RUNS_JSON_FORMAT_VERSION,
            }
        } else {
            RunsJsonWritePermission::Allowed
        }
    }
}

/// Metadata about a recorded run (serialization format for runs.json.zst and portable recordings).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RecordedRun {
    /// The unique identifier for this run.
    pub(super) run_id: ReportUuid,
    /// The major format version of this run's store.zip and run.log.
    ///
    /// Runs with a different major version cannot be replayed by this nextest
    /// version.
    pub(super) store_format_version: StoreFormatMajorVersion,
    /// The minor format version of this run's store.zip and run.log.
    ///
    /// Runs with a newer minor version (same major) cannot be replayed by this
    /// nextest version. Older minor versions are compatible.
    #[serde(default)]
    pub(super) store_format_minor_version: StoreFormatMinorVersion,
    /// The version of nextest that created this run.
    pub(super) nextest_version: Version,
    /// When the run started.
    pub(super) started_at: DateTime<FixedOffset>,
    /// When this run was last written to.
    ///
    /// Used for LRU eviction. Updated when the run is created, when the run
    /// completes, and in the future when operations like `rerun` reference
    /// this run.
    pub(super) last_written_at: DateTime<FixedOffset>,
    /// Duration of the run in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) duration_secs: Option<f64>,
    /// The command-line arguments used to invoke nextest.
    #[serde(default)]
    pub(super) cli_args: Vec<String>,
    /// Build scope arguments (package and target selection).
    ///
    /// These determine which packages and targets are built. In a rerun chain,
    /// these are inherited from the original run unless explicitly overridden.
    #[serde(default)]
    pub(super) build_scope_args: Vec<String>,
    /// Environment variables that affect nextest behavior (NEXTEST_* and CARGO_*).
    ///
    /// This has a default for deserializing old runs.json.zst files that don't have this field.
    #[serde(default)]
    pub(super) env_vars: BTreeMap<String, String>,
    /// The parent run ID.
    #[serde(default)]
    pub(super) parent_run_id: Option<ReportUuid>,
    /// Sizes broken down by component (log and store).
    ///
    /// This is all zeros until the run completes successfully.
    pub(super) sizes: RecordedSizesFormat,
    /// Status and statistics for the run.
    pub(super) status: RecordedRunStatusFormat,
}

/// Sizes broken down by component (serialization format for runs.json.zst).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RecordedSizesFormat {
    /// Sizes for the run log (run.log.zst).
    pub(super) log: ComponentSizesFormat,
    /// Sizes for the store archive (store.zip).
    pub(super) store: ComponentSizesFormat,
}

/// Compressed and uncompressed sizes for a single component (serialization format).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ComponentSizesFormat {
    /// Compressed size in bytes.
    pub(super) compressed: u64,
    /// Uncompressed size in bytes.
    pub(super) uncompressed: u64,
    /// Number of entries (records for log, files for store).
    #[serde(default)]
    pub(super) entries: u64,
}

impl From<RecordedSizes> for RecordedSizesFormat {
    fn from(sizes: RecordedSizes) -> Self {
        Self {
            log: ComponentSizesFormat {
                compressed: sizes.log.compressed,
                uncompressed: sizes.log.uncompressed,
                entries: sizes.log.entries,
            },
            store: ComponentSizesFormat {
                compressed: sizes.store.compressed,
                uncompressed: sizes.store.uncompressed,
                entries: sizes.store.entries,
            },
        }
    }
}

impl From<RecordedSizesFormat> for RecordedSizes {
    fn from(sizes: RecordedSizesFormat) -> Self {
        Self {
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
        }
    }
}

/// Status of a recorded run (serialization format).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub(super) enum RecordedRunStatusFormat {
    /// The run was interrupted before completion.
    Incomplete,
    /// A normal test run completed.
    #[serde(rename_all = "kebab-case")]
    Completed {
        /// The number of tests that were expected to run.
        initial_run_count: usize,
        /// The number of tests that passed.
        passed: usize,
        /// The number of tests that failed.
        failed: usize,
        /// The exit code from the run.
        exit_code: i32,
    },
    /// A normal test run was cancelled.
    #[serde(rename_all = "kebab-case")]
    Cancelled {
        /// The number of tests that were expected to run.
        initial_run_count: usize,
        /// The number of tests that passed.
        passed: usize,
        /// The number of tests that failed.
        failed: usize,
        /// The exit code from the run.
        exit_code: i32,
    },
    /// A stress test run completed.
    #[serde(rename_all = "kebab-case")]
    StressCompleted {
        /// The number of stress iterations that were expected to run, if known.
        initial_iteration_count: Option<NonZero<u32>>,
        /// The number of stress iterations that succeeded.
        success_count: u32,
        /// The number of stress iterations that failed.
        failed_count: u32,
        /// The exit code from the run.
        exit_code: i32,
    },
    /// A stress test run was cancelled.
    #[serde(rename_all = "kebab-case")]
    StressCancelled {
        /// The number of stress iterations that were expected to run, if known.
        initial_iteration_count: Option<NonZero<u32>>,
        /// The number of stress iterations that succeeded.
        success_count: u32,
        /// The number of stress iterations that failed.
        failed_count: u32,
        /// The exit code from the run.
        exit_code: i32,
    },
    /// An unknown status from a newer version of nextest.
    ///
    /// This variant is used for forward compatibility when reading runs.json.zst
    /// files created by newer nextest versions that may have new status types.
    #[serde(other)]
    Unknown,
}

impl From<RecordedRun> for RecordedRunInfo {
    fn from(run: RecordedRun) -> Self {
        Self {
            run_id: run.run_id,
            store_format_version: StoreFormatVersion::new(
                run.store_format_version,
                run.store_format_minor_version,
            ),
            nextest_version: run.nextest_version,
            started_at: run.started_at,
            last_written_at: run.last_written_at,
            duration_secs: run.duration_secs,
            cli_args: run.cli_args,
            build_scope_args: run.build_scope_args,
            env_vars: run.env_vars,
            parent_run_id: run.parent_run_id,
            sizes: run.sizes.into(),
            status: run.status.into(),
        }
    }
}

impl From<&RecordedRunInfo> for RecordedRun {
    fn from(run: &RecordedRunInfo) -> Self {
        Self {
            run_id: run.run_id,
            store_format_version: run.store_format_version.major,
            store_format_minor_version: run.store_format_version.minor,
            nextest_version: run.nextest_version.clone(),
            started_at: run.started_at,
            last_written_at: run.last_written_at,
            duration_secs: run.duration_secs,
            cli_args: run.cli_args.clone(),
            build_scope_args: run.build_scope_args.clone(),
            env_vars: run.env_vars.clone(),
            parent_run_id: run.parent_run_id,
            sizes: run.sizes.into(),
            status: (&run.status).into(),
        }
    }
}

impl From<RecordedRunStatusFormat> for RecordedRunStatus {
    fn from(status: RecordedRunStatusFormat) -> Self {
        match status {
            RecordedRunStatusFormat::Incomplete => Self::Incomplete,
            RecordedRunStatusFormat::Unknown => Self::Unknown,
            RecordedRunStatusFormat::Completed {
                initial_run_count,
                passed,
                failed,
                exit_code,
            } => Self::Completed(CompletedRunStats {
                initial_run_count,
                passed,
                failed,
                exit_code,
            }),
            RecordedRunStatusFormat::Cancelled {
                initial_run_count,
                passed,
                failed,
                exit_code,
            } => Self::Cancelled(CompletedRunStats {
                initial_run_count,
                passed,
                failed,
                exit_code,
            }),
            RecordedRunStatusFormat::StressCompleted {
                initial_iteration_count,
                success_count,
                failed_count,
                exit_code,
            } => Self::StressCompleted(StressCompletedRunStats {
                initial_iteration_count,
                success_count,
                failed_count,
                exit_code,
            }),
            RecordedRunStatusFormat::StressCancelled {
                initial_iteration_count,
                success_count,
                failed_count,
                exit_code,
            } => Self::StressCancelled(StressCompletedRunStats {
                initial_iteration_count,
                success_count,
                failed_count,
                exit_code,
            }),
        }
    }
}

impl From<&RecordedRunStatus> for RecordedRunStatusFormat {
    fn from(status: &RecordedRunStatus) -> Self {
        match status {
            RecordedRunStatus::Incomplete => Self::Incomplete,
            RecordedRunStatus::Unknown => Self::Unknown,
            RecordedRunStatus::Completed(stats) => Self::Completed {
                initial_run_count: stats.initial_run_count,
                passed: stats.passed,
                failed: stats.failed,
                exit_code: stats.exit_code,
            },
            RecordedRunStatus::Cancelled(stats) => Self::Cancelled {
                initial_run_count: stats.initial_run_count,
                passed: stats.passed,
                failed: stats.failed,
                exit_code: stats.exit_code,
            },
            RecordedRunStatus::StressCompleted(stats) => Self::StressCompleted {
                initial_iteration_count: stats.initial_iteration_count,
                success_count: stats.success_count,
                failed_count: stats.failed_count,
                exit_code: stats.exit_code,
            },
            RecordedRunStatus::StressCancelled(stats) => Self::StressCancelled {
                initial_iteration_count: stats.initial_iteration_count,
                success_count: stats.success_count,
                failed_count: stats.failed_count,
                exit_code: stats.exit_code,
            },
        }
    }
}

// ---
// Rerun types
// ---

/// Rerun-specific metadata stored in `meta/rerun-info.json`.
///
/// This is only present for reruns (runs with a parent run).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RerunInfo {
    /// The immediate parent run ID.
    pub parent_run_id: ReportUuid,

    /// Root information from the original run.
    pub root_info: RerunRootInfo,

    /// The set of outstanding and passing test cases.
    pub test_suites: IdOrdMap<RerunTestSuiteInfo>,
}

/// For a rerun, information obtained from the root of the rerun chain.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RerunRootInfo {
    /// The run ID.
    pub run_id: ReportUuid,

    /// Build scope args from the original run.
    pub build_scope_args: Vec<String>,
}

impl RerunRootInfo {
    /// Creates a new `RerunRootInfo` for a root of a rerun chain.
    ///
    /// `build_scope_args` should be the build scope arguments extracted from
    /// the original run's CLI args. Use `extract_build_scope_args` from
    /// `cargo-nextest` to extract these.
    pub fn new(run_id: ReportUuid, build_scope_args: Vec<String>) -> Self {
        Self {
            run_id,
            build_scope_args,
        }
    }
}

/// A test suite's outstanding and passing test cases.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RerunTestSuiteInfo {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The set of passing test cases.
    pub passing: BTreeSet<TestCaseName>,

    /// The set of outstanding test cases.
    pub outstanding: BTreeSet<TestCaseName>,
}

impl RerunTestSuiteInfo {
    pub(super) fn new(binary_id: RustBinaryId) -> Self {
        Self {
            binary_id,
            passing: BTreeSet::new(),
            outstanding: BTreeSet::new(),
        }
    }
}

impl IdOrdItem for RerunTestSuiteInfo {
    type Key<'a> = &'a RustBinaryId;
    fn key(&self) -> Self::Key<'_> {
        &self.binary_id
    }
    id_upcast!();
}

// ---
// Recording format types
// ---

/// File name for the store archive.
pub static STORE_ZIP_FILE_NAME: &str = "store.zip";

/// File name for the run log.
pub static RUN_LOG_FILE_NAME: &str = "run.log.zst";

/// Returns true if the path has a `.zip` extension (case-insensitive).
pub fn has_zip_extension(path: &Utf8Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

// Paths within the zip archive.
/// Path to cargo metadata within the store archive.
pub static CARGO_METADATA_JSON_PATH: &str = "meta/cargo-metadata.json";
/// Path to the test list within the store archive.
pub static TEST_LIST_JSON_PATH: &str = "meta/test-list.json";
/// Path to record options within the store archive.
pub static RECORD_OPTS_JSON_PATH: &str = "meta/record-opts.json";
/// Path to rerun info within the store archive (only present for reruns).
pub static RERUN_INFO_JSON_PATH: &str = "meta/rerun-info.json";
/// Path to the stdout dictionary within the store archive.
pub static STDOUT_DICT_PATH: &str = "meta/stdout.dict";
/// Path to the stderr dictionary within the store archive.
pub static STDERR_DICT_PATH: &str = "meta/stderr.dict";

// ---
// Portable recording format types
// ---

define_format_version! {
    /// Major version of the portable recording format for breaking changes.
    pub struct PortableRecordingFormatMajorVersion;
}

define_format_version! {
    @default
    /// Minor version of the portable recording format for additive changes.
    pub struct PortableRecordingFormatMinorVersion;
}

/// Combined major and minor version of the portable recording format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PortableRecordingFormatVersion {
    /// The major version (breaking changes).
    pub major: PortableRecordingFormatMajorVersion,
    /// The minor version (additive changes).
    pub minor: PortableRecordingFormatMinorVersion,
}

impl PortableRecordingFormatVersion {
    /// Creates a new `PortableRecordingFormatVersion`.
    pub const fn new(
        major: PortableRecordingFormatMajorVersion,
        minor: PortableRecordingFormatMinorVersion,
    ) -> Self {
        Self { major, minor }
    }

    /// Checks if an archive with version `self` can be read by a reader that
    /// supports `supported`.
    pub fn check_readable_by(
        self,
        supported: Self,
    ) -> Result<(), PortableRecordingVersionIncompatibility> {
        if self.major != supported.major {
            return Err(PortableRecordingVersionIncompatibility::MajorMismatch {
                archive_major: self.major,
                supported_major: supported.major,
            });
        }
        if self.minor > supported.minor {
            return Err(PortableRecordingVersionIncompatibility::MinorTooNew {
                archive_minor: self.minor,
                supported_minor: supported.minor,
            });
        }
        Ok(())
    }
}

impl fmt::Display for PortableRecordingFormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// An incompatibility between an archive's portable format version and what the
/// reader supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortableRecordingVersionIncompatibility {
    /// The archive's major version differs from the supported major version.
    MajorMismatch {
        /// The major version in the archive.
        archive_major: PortableRecordingFormatMajorVersion,
        /// The major version this nextest supports.
        supported_major: PortableRecordingFormatMajorVersion,
    },
    /// The archive's minor version is newer than the supported minor version.
    MinorTooNew {
        /// The minor version in the archive.
        archive_minor: PortableRecordingFormatMinorVersion,
        /// The maximum minor version this nextest supports.
        supported_minor: PortableRecordingFormatMinorVersion,
    },
}

impl fmt::Display for PortableRecordingVersionIncompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MajorMismatch {
                archive_major,
                supported_major,
            } => {
                write!(
                    f,
                    "major version {} differs from supported version {}",
                    archive_major, supported_major
                )
            }
            Self::MinorTooNew {
                archive_minor,
                supported_minor,
            } => {
                write!(
                    f,
                    "minor version {} is newer than supported version {}",
                    archive_minor, supported_minor
                )
            }
        }
    }
}

/// The current format version for portable recordings.
pub const PORTABLE_RECORDING_FORMAT_VERSION: PortableRecordingFormatVersion =
    PortableRecordingFormatVersion::new(
        PortableRecordingFormatMajorVersion::new(1),
        PortableRecordingFormatMinorVersion::new(0),
    );

/// File name for the manifest within a portable recording.
pub static PORTABLE_MANIFEST_FILE_NAME: &str = "manifest.json";

/// The manifest for a portable recording.
///
/// A portable recording packages a single recorded run into a self-contained
/// zip file for sharing and import.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PortableManifest {
    /// The format version of this portable recording.
    pub(crate) format_version: PortableRecordingFormatVersion,
    /// The run metadata.
    pub(super) run: RecordedRun,
}

impl PortableManifest {
    /// Creates a new manifest for the given run.
    pub(crate) fn new(run: &RecordedRunInfo) -> Self {
        Self {
            format_version: PORTABLE_RECORDING_FORMAT_VERSION,
            run: RecordedRun::from(run),
        }
    }

    /// Returns the run info extracted from this manifest.
    pub(crate) fn run_info(&self) -> RecordedRunInfo {
        RecordedRunInfo::from(self.run.clone())
    }

    /// Returns the store format version from the run metadata.
    pub(crate) fn store_format_version(&self) -> StoreFormatVersion {
        StoreFormatVersion::new(
            self.run.store_format_version,
            self.run.store_format_minor_version,
        )
    }
}

/// Which dictionary to use for compressing/decompressing a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputDict {
    /// Use the stdout dictionary (for stdout and combined output).
    Stdout,
    /// Use the stderr dictionary.
    Stderr,
    /// Use standard zstd compression (for metadata files).
    None,
}

impl OutputDict {
    /// Determines which dictionary to use based on the file path.
    ///
    /// Output files in `out/` use dictionaries based on their suffix:
    /// - `-stdout` and `-combined` use the stdout dictionary.
    /// - `-stderr` uses the stderr dictionary.
    ///
    /// All other files (metadata in `meta/`) use standard zstd.
    pub fn for_path(path: &Utf8Path) -> Self {
        let mut iter = path.iter();
        let Some(first_component) = iter.next() else {
            return Self::None;
        };
        // Output files are always in the out/ directory.
        if first_component != "out" {
            return Self::None;
        }

        Self::for_output_file_name(iter.as_path().as_str())
    }

    /// Determines which dictionary to use based on the output file name.
    ///
    /// The file name should be the basename without the `out/` prefix,
    /// e.g., `test-abc123-1-stdout`.
    pub fn for_output_file_name(file_name: &str) -> Self {
        if file_name.ends_with("-stdout") || file_name.ends_with("-combined") {
            Self::Stdout
        } else if file_name.ends_with("-stderr") {
            Self::Stderr
        } else {
            // Unknown output type, use standard compression.
            Self::None
        }
    }

    /// Returns the dictionary bytes for this output type (for writing new archives).
    ///
    /// Returns `None` for `OutputDict::None`.
    pub fn dict_bytes(self) -> Option<&'static [u8]> {
        match self {
            Self::Stdout => Some(super::dicts::STDOUT),
            Self::Stderr => Some(super::dicts::STDERR),
            Self::None => None,
        }
    }
}

// ---
// Zip file options helpers
// ---

/// Returns file options for storing pre-compressed data (no additional
/// compression).
pub(super) fn stored_file_options() -> FileOptions {
    let mut options = FileOptions::default();
    options.compression_method = CompressionMethod::STORE;
    options
}

/// Returns file options for zstd-compressed data.
pub(super) fn zstd_file_options() -> FileOptions {
    let mut options = FileOptions::default();
    options.compression_method = CompressionMethod::ZSTD;
    options.level = Some(3);
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_dict_for_path() {
        // Metadata files should not use dictionaries.
        assert_eq!(
            OutputDict::for_path("meta/cargo-metadata.json".as_ref()),
            OutputDict::None
        );
        assert_eq!(
            OutputDict::for_path("meta/test-list.json".as_ref()),
            OutputDict::None
        );

        // Content-addressed output files should use appropriate dictionaries.
        assert_eq!(
            OutputDict::for_path("out/0123456789abcdef-stdout".as_ref()),
            OutputDict::Stdout
        );
        assert_eq!(
            OutputDict::for_path("out/0123456789abcdef-stderr".as_ref()),
            OutputDict::Stderr
        );
        assert_eq!(
            OutputDict::for_path("out/0123456789abcdef-combined".as_ref()),
            OutputDict::Stdout
        );
    }

    #[test]
    fn test_output_dict_for_output_file_name() {
        // Content-addressed file names.
        assert_eq!(
            OutputDict::for_output_file_name("0123456789abcdef-stdout"),
            OutputDict::Stdout
        );
        assert_eq!(
            OutputDict::for_output_file_name("0123456789abcdef-stderr"),
            OutputDict::Stderr
        );
        assert_eq!(
            OutputDict::for_output_file_name("0123456789abcdef-combined"),
            OutputDict::Stdout
        );
        assert_eq!(
            OutputDict::for_output_file_name("0123456789abcdef-unknown"),
            OutputDict::None
        );
    }

    #[test]
    fn test_dict_bytes() {
        assert!(OutputDict::Stdout.dict_bytes().is_some());
        assert!(OutputDict::Stderr.dict_bytes().is_some());
        assert!(OutputDict::None.dict_bytes().is_none());
    }

    #[test]
    fn test_runs_json_missing_version() {
        // runs.json.zst without format-version should fail to deserialize.
        let json = r#"{"runs": []}"#;
        let result: Result<RecordedRunList, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error for missing format-version");
    }

    #[test]
    fn test_runs_json_current_version() {
        // runs.json.zst with current version should deserialize and allow writes.
        let json = format!(
            r#"{{"format-version": {}, "runs": []}}"#,
            RUNS_JSON_FORMAT_VERSION
        );
        let list: RecordedRunList = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(list.write_permission(), RunsJsonWritePermission::Allowed);
    }

    #[test]
    fn test_runs_json_older_version() {
        // runs.json.zst with older version (if any existed) should allow writes.
        // Since we only have version 1, test version 0 if we supported it.
        // For now, this test just ensures version 1 allows writes.
        let json = r#"{"format-version": 1, "runs": []}"#;
        let list: RecordedRunList = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(list.write_permission(), RunsJsonWritePermission::Allowed);
    }

    #[test]
    fn test_runs_json_newer_version() {
        // runs.json.zst with newer version should deserialize but deny writes.
        let json = r#"{"format-version": 99, "runs": []}"#;
        let list: RecordedRunList = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(
            list.write_permission(),
            RunsJsonWritePermission::Denied {
                file_version: RunsJsonFormatVersion::new(99),
                max_supported_version: RUNS_JSON_FORMAT_VERSION,
            }
        );
    }

    #[test]
    fn test_runs_json_serialization_includes_version() {
        // Serialized runs.json.zst should always include format-version.
        let list = RecordedRunList::from_data(&[], None);
        let json = serde_json::to_string(&list).expect("should serialize");
        assert!(
            json.contains("format-version"),
            "serialized runs.json.zst should include format-version"
        );

        // Verify it's the current version.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");
        let version: RunsJsonFormatVersion =
            serde_json::from_value(parsed["format-version"].clone()).expect("valid version");
        assert_eq!(
            version, RUNS_JSON_FORMAT_VERSION,
            "format-version should be current version"
        );
    }

    #[test]
    fn test_runs_json_new() {
        // RecordedRunList::new() should create with current version.
        let list = RecordedRunList::new();
        assert_eq!(list.format_version, RUNS_JSON_FORMAT_VERSION);
        assert!(list.runs.is_empty());
        assert_eq!(list.write_permission(), RunsJsonWritePermission::Allowed);
    }

    // --- RecordedRun serialization snapshot tests ---

    fn make_test_run(status: RecordedRunStatusFormat) -> RecordedRun {
        RecordedRun {
            run_id: ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000),
            store_format_version: STORE_FORMAT_VERSION.major,
            store_format_minor_version: STORE_FORMAT_VERSION.minor,
            nextest_version: Version::new(0, 9, 111),
            started_at: DateTime::parse_from_rfc3339("2024-12-19T14:22:33-08:00")
                .expect("valid timestamp"),
            last_written_at: DateTime::parse_from_rfc3339("2024-12-19T22:22:33Z")
                .expect("valid timestamp"),
            duration_secs: Some(12.345),
            cli_args: vec![
                "cargo".to_owned(),
                "nextest".to_owned(),
                "run".to_owned(),
                "--workspace".to_owned(),
            ],
            build_scope_args: vec!["--workspace".to_owned()],
            env_vars: BTreeMap::from([
                ("CARGO_TERM_COLOR".to_owned(), "always".to_owned()),
                ("NEXTEST_PROFILE".to_owned(), "ci".to_owned()),
            ]),
            parent_run_id: Some(ReportUuid::from_u128(
                0x550e7400_e29b_41d4_a716_446655440000,
            )),
            sizes: RecordedSizesFormat {
                log: ComponentSizesFormat {
                    compressed: 2345,
                    uncompressed: 5678,
                    entries: 42,
                },
                store: ComponentSizesFormat {
                    compressed: 10000,
                    uncompressed: 40000,
                    entries: 15,
                },
            },
            status,
        }
    }

    #[test]
    fn test_recorded_run_serialize_incomplete() {
        let run = make_test_run(RecordedRunStatusFormat::Incomplete);
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_serialize_completed() {
        let run = make_test_run(RecordedRunStatusFormat::Completed {
            initial_run_count: 100,
            passed: 95,
            failed: 5,
            exit_code: 0,
        });
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_serialize_cancelled() {
        let run = make_test_run(RecordedRunStatusFormat::Cancelled {
            initial_run_count: 100,
            passed: 45,
            failed: 5,
            exit_code: 100,
        });
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_serialize_stress_completed() {
        let run = make_test_run(RecordedRunStatusFormat::StressCompleted {
            initial_iteration_count: NonZero::new(100),
            success_count: 98,
            failed_count: 2,
            exit_code: 0,
        });
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_serialize_stress_cancelled() {
        let run = make_test_run(RecordedRunStatusFormat::StressCancelled {
            initial_iteration_count: NonZero::new(100),
            success_count: 45,
            failed_count: 5,
            exit_code: 100,
        });
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_deserialize_unknown_status() {
        // Simulate a run from a future nextest version with an unknown status.
        // The store-format-version is set to 999 to indicate a future version.
        let json = r#"{
            "run-id": "550e8400-e29b-41d4-a716-446655440000",
            "store-format-version": 999,
            "nextest-version": "0.9.999",
            "started-at": "2024-12-19T14:22:33-08:00",
            "last-written-at": "2024-12-19T22:22:33Z",
            "cli-args": ["cargo", "nextest", "run"],
            "env-vars": {},
            "sizes": {
                "log": { "compressed": 2345, "uncompressed": 5678 },
                "store": { "compressed": 10000, "uncompressed": 40000 }
            },
            "status": {
                "status": "super-new-status",
                "some-future-field": 42
            }
        }"#;
        let run: RecordedRun = serde_json::from_str(json).expect("should deserialize");
        assert!(
            matches!(run.status, RecordedRunStatusFormat::Unknown),
            "unknown status should deserialize to Unknown variant"
        );

        // Verify domain conversion preserves Unknown.
        let info: RecordedRunInfo = run.into();
        assert!(
            matches!(info.status, RecordedRunStatus::Unknown),
            "Unknown format should convert to Unknown domain type"
        );
    }

    #[test]
    fn test_recorded_run_roundtrip() {
        let original = make_test_run(RecordedRunStatusFormat::Completed {
            initial_run_count: 100,
            passed: 95,
            failed: 5,
            exit_code: 0,
        });
        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let roundtripped: RecordedRun =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(roundtripped.run_id, original.run_id);
        assert_eq!(roundtripped.nextest_version, original.nextest_version);
        assert_eq!(roundtripped.started_at, original.started_at);
        assert_eq!(roundtripped.sizes, original.sizes);

        // Verify status fields via domain conversion.
        let info: RecordedRunInfo = roundtripped.into();
        match info.status {
            RecordedRunStatus::Completed(stats) => {
                assert_eq!(stats.initial_run_count, 100);
                assert_eq!(stats.passed, 95);
                assert_eq!(stats.failed, 5);
            }
            _ => panic!("expected Completed variant"),
        }
    }

    // --- Store format version tests ---

    /// Helper to create a StoreFormatVersion.
    fn version(major: u32, minor: u32) -> StoreFormatVersion {
        StoreFormatVersion::new(
            StoreFormatMajorVersion::new(major),
            StoreFormatMinorVersion::new(minor),
        )
    }

    #[test]
    fn test_store_version_compatibility() {
        assert!(
            version(1, 0).check_readable_by(version(1, 0)).is_ok(),
            "same version should be compatible"
        );

        assert!(
            version(1, 0).check_readable_by(version(1, 2)).is_ok(),
            "older minor version should be compatible"
        );

        let error = version(1, 3).check_readable_by(version(1, 2)).unwrap_err();
        assert_eq!(
            error,
            StoreVersionIncompatibility::MinorTooNew {
                archive_minor: StoreFormatMinorVersion::new(3),
                supported_minor: StoreFormatMinorVersion::new(2),
            },
            "newer minor version should be incompatible"
        );
        insta::assert_snapshot!(error.to_string(), @"minor version 3 is newer than supported version 2");

        let error = version(2, 0).check_readable_by(version(1, 5)).unwrap_err();
        assert_eq!(
            error,
            StoreVersionIncompatibility::MajorMismatch {
                archive_major: StoreFormatMajorVersion::new(2),
                supported_major: StoreFormatMajorVersion::new(1),
            },
            "different major version should be incompatible"
        );
        insta::assert_snapshot!(error.to_string(), @"major version 2 differs from supported version 1");

        insta::assert_snapshot!(version(1, 2).to_string(), @"1.2");
    }

    #[test]
    fn test_recorded_run_deserialize_without_minor_version() {
        // Old archives without store-format-minor-version should default to 0.
        let json = r#"{
            "run-id": "550e8400-e29b-41d4-a716-446655440000",
            "store-format-version": 1,
            "nextest-version": "0.9.111",
            "started-at": "2024-12-19T14:22:33-08:00",
            "last-written-at": "2024-12-19T22:22:33Z",
            "cli-args": [],
            "env-vars": {},
            "sizes": {
                "log": { "compressed": 0, "uncompressed": 0 },
                "store": { "compressed": 0, "uncompressed": 0 }
            },
            "status": { "status": "incomplete" }
        }"#;
        let run: RecordedRun = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(run.store_format_version, StoreFormatMajorVersion::new(1));
        assert_eq!(
            run.store_format_minor_version,
            StoreFormatMinorVersion::new(0)
        );

        // Domain conversion should produce a StoreFormatVersion with minor 0.
        let info: RecordedRunInfo = run.into();
        assert_eq!(info.store_format_version, version(1, 0));
    }

    #[test]
    fn test_recorded_run_serialize_includes_minor_version() {
        // New archives should include store-format-minor-version in serialization.
        let run = make_test_run(RecordedRunStatusFormat::Incomplete);
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        assert!(
            json.contains("store-format-minor-version"),
            "serialized run should include store-format-minor-version"
        );
    }

    // --- Portable archive format version tests ---

    /// Helper to create a PortableRecordingFormatVersion.
    fn portable_version(major: u32, minor: u32) -> PortableRecordingFormatVersion {
        PortableRecordingFormatVersion::new(
            PortableRecordingFormatMajorVersion::new(major),
            PortableRecordingFormatMinorVersion::new(minor),
        )
    }

    #[test]
    fn test_portable_version_compatibility() {
        assert!(
            portable_version(1, 0)
                .check_readable_by(portable_version(1, 0))
                .is_ok(),
            "same version should be compatible"
        );

        assert!(
            portable_version(1, 0)
                .check_readable_by(portable_version(1, 2))
                .is_ok(),
            "older minor version should be compatible"
        );

        let error = portable_version(1, 3)
            .check_readable_by(portable_version(1, 2))
            .unwrap_err();
        assert_eq!(
            error,
            PortableRecordingVersionIncompatibility::MinorTooNew {
                archive_minor: PortableRecordingFormatMinorVersion::new(3),
                supported_minor: PortableRecordingFormatMinorVersion::new(2),
            },
            "newer minor version should be incompatible"
        );
        insta::assert_snapshot!(error.to_string(), @"minor version 3 is newer than supported version 2");

        let error = portable_version(2, 0)
            .check_readable_by(portable_version(1, 5))
            .unwrap_err();
        assert_eq!(
            error,
            PortableRecordingVersionIncompatibility::MajorMismatch {
                archive_major: PortableRecordingFormatMajorVersion::new(2),
                supported_major: PortableRecordingFormatMajorVersion::new(1),
            },
            "different major version should be incompatible"
        );
        insta::assert_snapshot!(error.to_string(), @"major version 2 differs from supported version 1");

        insta::assert_snapshot!(portable_version(1, 2).to_string(), @"1.2");
    }

    #[test]
    fn test_portable_version_serialization() {
        // Test that PortableRecordingFormatVersion serializes to {major: ..., minor: ...}.
        let version = portable_version(1, 0);
        let json = serde_json::to_string(&version).expect("serialization should succeed");
        insta::assert_snapshot!(json, @r#"{"major":1,"minor":0}"#);

        // Test roundtrip.
        let roundtripped: PortableRecordingFormatVersion =
            serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(roundtripped, version);
    }

    #[test]
    fn test_portable_manifest_format_version() {
        // Verify the current PORTABLE_RECORDING_FORMAT_VERSION constant.
        assert_eq!(
            PORTABLE_RECORDING_FORMAT_VERSION,
            portable_version(1, 0),
            "current portable recording format version should be 1.0"
        );
    }
}
