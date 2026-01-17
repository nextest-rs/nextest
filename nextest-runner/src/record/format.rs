// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Archive format metadata shared between recorder and reader.

use super::{
    CompletedRunStats, ComponentSizes, RecordedRunInfo, RecordedRunStatus, RecordedSizes,
    StressCompletedRunStats,
};
use camino::Utf8Path;
use chrono::{DateTime, FixedOffset, Utc};
use quick_junit::ReportUuid;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, num::NonZero};

// ---
// runs.json format types
// ---

/// The current format version for runs.json.
///
/// Increment this when adding new semantically important fields. Readers can
/// read newer versions (assuming append-only evolution with serde defaults),
/// but writers must refuse to write if the file version is higher than this.
pub(super) const RUNS_JSON_FORMAT_VERSION: u32 = 1;

/// Whether a runs.json file can be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunsJsonWritePermission {
    /// Writing is allowed.
    Allowed,
    /// Writing is not allowed because the file has a newer format version.
    Denied {
        /// The format version in the file.
        file_version: u32,
        /// The maximum version this nextest can write.
        max_supported_version: u32,
    },
}

/// The list of recorded runs (serialization format for runs.json).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RecordedRunList {
    /// The format version of this file.
    pub(super) format_version: u32,

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

    /// Returns whether this runs.json can be written to by this nextest version.
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

/// Metadata about a recorded run (serialization format for runs.json).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RecordedRun {
    /// The unique identifier for this run.
    pub(super) run_id: ReportUuid,
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
    /// Environment variables that affect nextest behavior (NEXTEST_* and CARGO_*).
    ///
    /// This has a default for deserializing old runs.json files that don't have this field.
    #[serde(default)]
    pub(super) env_vars: BTreeMap<String, String>,
    /// Sizes broken down by component (log and store).
    ///
    /// This is all zeros until the run completes successfully.
    pub(super) sizes: RecordedSizesFormat,
    /// Status and statistics for the run.
    pub(super) status: RecordedRunStatusFormat,
}

/// Sizes broken down by component (serialization format for runs.json).
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
    Completed {
        /// The number of tests that were expected to run.
        #[serde(rename = "initial-run-count")]
        initial_run_count: usize,
        /// The number of tests that passed.
        passed: usize,
        /// The number of tests that failed.
        failed: usize,
    },
    /// A normal test run was cancelled.
    Cancelled {
        /// The number of tests that were expected to run.
        #[serde(rename = "initial-run-count")]
        initial_run_count: usize,
        /// The number of tests that passed.
        passed: usize,
        /// The number of tests that failed.
        failed: usize,
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
    },
    /// An unknown status from a newer version of nextest.
    ///
    /// This variant is used for forward compatibility when reading runs.json
    /// files created by newer nextest versions that may have new status types.
    #[serde(other)]
    Unknown,
}

impl From<RecordedRun> for RecordedRunInfo {
    fn from(run: RecordedRun) -> Self {
        Self {
            run_id: run.run_id,
            nextest_version: run.nextest_version,
            started_at: run.started_at,
            last_written_at: run.last_written_at,
            duration_secs: run.duration_secs,
            cli_args: run.cli_args,
            env_vars: run.env_vars,
            sizes: run.sizes.into(),
            status: run.status.into(),
        }
    }
}

impl From<&RecordedRunInfo> for RecordedRun {
    fn from(run: &RecordedRunInfo) -> Self {
        Self {
            run_id: run.run_id,
            nextest_version: run.nextest_version.clone(),
            started_at: run.started_at,
            last_written_at: run.last_written_at,
            duration_secs: run.duration_secs,
            cli_args: run.cli_args.clone(),
            env_vars: run.env_vars.clone(),
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
            } => Self::Completed(CompletedRunStats {
                initial_run_count,
                passed,
                failed,
            }),
            RecordedRunStatusFormat::Cancelled {
                initial_run_count,
                passed,
                failed,
            } => Self::Cancelled(CompletedRunStats {
                initial_run_count,
                passed,
                failed,
            }),
            RecordedRunStatusFormat::StressCompleted {
                initial_iteration_count,
                success_count,
                failed_count,
            } => Self::StressCompleted(StressCompletedRunStats {
                initial_iteration_count,
                success_count,
                failed_count,
            }),
            RecordedRunStatusFormat::StressCancelled {
                initial_iteration_count,
                success_count,
                failed_count,
            } => Self::StressCancelled(StressCompletedRunStats {
                initial_iteration_count,
                success_count,
                failed_count,
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
            },
            RecordedRunStatus::Cancelled(stats) => Self::Cancelled {
                initial_run_count: stats.initial_run_count,
                passed: stats.passed,
                failed: stats.failed,
            },
            RecordedRunStatus::StressCompleted(stats) => Self::StressCompleted {
                initial_iteration_count: stats.initial_iteration_count,
                success_count: stats.success_count,
                failed_count: stats.failed_count,
            },
            RecordedRunStatus::StressCancelled(stats) => Self::StressCancelled {
                initial_iteration_count: stats.initial_iteration_count,
                success_count: stats.success_count,
                failed_count: stats.failed_count,
            },
        }
    }
}

// ---
// Archive format types
// ---

/// The current format version for recorded test runs.
///
/// Increment this when making breaking changes to the archive structure or
/// event format. Readers should check this version and refuse to read archives
/// with a version higher than they support.
pub(super) const RECORD_FORMAT_VERSION: u32 = 1;

// Archive file names.
pub(super) static STORE_ZIP_FILE_NAME: &str = "store.zip";
pub(super) static RUN_LOG_FILE_NAME: &str = "run.log.zst";

// Paths within the zip archive.
pub(super) static FORMAT_JSON_PATH: &str = "meta/format.json";
pub(super) static CARGO_METADATA_JSON_PATH: &str = "meta/cargo-metadata.json";
pub(super) static TEST_LIST_JSON_PATH: &str = "meta/test-list.json";
pub(super) static RECORD_OPTS_JSON_PATH: &str = "meta/record-opts.json";
pub(super) static STDOUT_DICT_PATH: &str = "meta/stdout.dict";
pub(super) static STDERR_DICT_PATH: &str = "meta/stderr.dict";

/// Format metadata stored in `meta/format.json` in the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct FormatMetadata {
    /// The format version of this archive.
    pub version: u32,
}

impl FormatMetadata {
    /// Creates metadata for a new archive with the current format version.
    pub fn new() -> Self {
        Self {
            version: RECORD_FORMAT_VERSION,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_dict_for_path() {
        // Metadata files should not use dictionaries.
        assert_eq!(
            OutputDict::for_path("meta/format.json".as_ref()),
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
        // runs.json without format-version should fail to deserialize.
        let json = r#"{"runs": []}"#;
        let result: Result<RecordedRunList, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error for missing format-version");
    }

    #[test]
    fn test_runs_json_current_version() {
        // runs.json with current version should deserialize and allow writes.
        let json = format!(
            r#"{{"format-version": {}, "runs": []}}"#,
            RUNS_JSON_FORMAT_VERSION
        );
        let list: RecordedRunList = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(list.write_permission(), RunsJsonWritePermission::Allowed);
    }

    #[test]
    fn test_runs_json_older_version() {
        // runs.json with older version (if any existed) should allow writes.
        // Since we only have version 1, test version 0 if we supported it.
        // For now, this test just ensures version 1 allows writes.
        let json = r#"{"format-version": 1, "runs": []}"#;
        let list: RecordedRunList = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(list.write_permission(), RunsJsonWritePermission::Allowed);
    }

    #[test]
    fn test_runs_json_newer_version() {
        // runs.json with newer version should deserialize but deny writes.
        let json = r#"{"format-version": 99, "runs": []}"#;
        let list: RecordedRunList = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(
            list.write_permission(),
            RunsJsonWritePermission::Denied {
                file_version: 99,
                max_supported_version: RUNS_JSON_FORMAT_VERSION,
            }
        );
    }

    #[test]
    fn test_runs_json_serialization_includes_version() {
        // Serialized runs.json should always include format-version.
        let list = RecordedRunList::from_data(&[], None);
        let json = serde_json::to_string(&list).expect("should serialize");
        assert!(
            json.contains("format-version"),
            "serialized runs.json should include format-version"
        );

        // Verify it's the current version.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");
        assert_eq!(
            parsed["format-version"], RUNS_JSON_FORMAT_VERSION,
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
            env_vars: BTreeMap::from([
                ("CARGO_TERM_COLOR".to_owned(), "always".to_owned()),
                ("NEXTEST_PROFILE".to_owned(), "ci".to_owned()),
            ]),
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
        });
        let json = serde_json::to_string_pretty(&run).expect("serialization should succeed");
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_recorded_run_deserialize_unknown_status() {
        // Simulate a run from a future nextest version with an unknown status.
        let json = r#"{
            "run-id": "550e8400-e29b-41d4-a716-446655440000",
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
}
