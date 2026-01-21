// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Serializable summary types for test events.
//!
//! This module provides types that can be serialized to JSON for recording test runs.
//! The types here mirror the runtime types in [`crate::reporter::events`] but are
//! designed for serialization rather than runtime use.
//!
//! The `O` type parameter represents how output is stored:
//! - [`ChildSingleOutput`]: Output stored in memory with lazy string conversion.
//! - [`ZipStoreOutput`]: Reference to a file stored in the zip archive.

use crate::{
    config::scripts::ScriptId,
    list::OwnedTestInstanceId,
    reporter::{
        TestOutputDisplay,
        events::{
            CancelReason, ExecuteStatus, ExecutionStatuses, RetryData, RunFinishedStats, RunStats,
            SetupScriptExecuteStatus, StressIndex, StressProgress, TestEvent, TestEventKind,
        },
    },
    run_mode::NextestRunMode,
    runner::StressCondition,
    test_output::ChildSingleOutput,
};
use chrono::{DateTime, FixedOffset};
use nextest_metadata::MismatchReason;
use quick_junit::ReportUuid;
use serde::{Deserialize, Serialize};
use std::{fmt, num::NonZero, time::Duration};

// ---
// Record options
// ---

/// Options that affect how test results are interpreted during replay.
///
/// These options are captured at record time and stored in the archive,
/// allowing replay to produce the same exit code as the original run.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct RecordOpts {
    /// The run mode (test or benchmark).
    #[serde(default)]
    pub run_mode: NextestRunMode,
}

impl RecordOpts {
    /// Creates a new `RecordOpts` with the given settings.
    pub fn new(run_mode: NextestRunMode) -> Self {
        Self { run_mode }
    }
}

// ---
// Test event summaries
// ---

/// A serializable form of a test event.
///
/// The `O` parameter represents how test outputs (stdout/stderr) are stored:
///
/// * [`ChildSingleOutput`]: Output stored in memory with lazy string conversion.
///   This is the first stage after converting from a [`TestEvent`].
/// * [`ZipStoreOutput`]: Reference to a file in the zip archive. This is the
///   final form after writing outputs to the store.
#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(
    rename_all = "kebab-case",
    bound(
        serialize = "O: Serialize",
        deserialize = "O: serde::de::DeserializeOwned"
    )
)]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + PartialEq + 'static))
)]
pub struct TestEventSummary<O> {
    /// The timestamp of the event.
    #[cfg_attr(
        test,
        strategy(crate::reporter::test_helpers::arb_datetime_fixed_offset())
    )]
    pub timestamp: DateTime<FixedOffset>,

    /// The time elapsed since the start of the test run.
    #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
    pub elapsed: Duration,

    /// The kind of test event this is.
    pub kind: TestEventKindSummary<O>,
}

impl TestEventSummary<ChildSingleOutput> {
    /// Converts a [`TestEvent`] to a serializable summary.
    ///
    /// Returns `None` for events that should not be recorded (informational and
    /// interactive events like `InfoStarted`, `InputEnter`, etc.).
    pub(crate) fn from_test_event(event: TestEvent<'_>) -> Option<Self> {
        let kind = TestEventKindSummary::from_test_event_kind(event.kind)?;
        Some(Self {
            timestamp: event.timestamp,
            elapsed: event.elapsed,
            kind,
        })
    }
}

/// The kind of test event.
///
/// This is a combined enum that wraps either a [`CoreEventKind`] (events
/// without output) or an [`OutputEventKind`] (events with output). The split
/// design allows conversion between output representations to only touch the
/// output-carrying variants.
#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + PartialEq + 'static))
)]
pub enum TestEventKindSummary<O> {
    /// An event that doesn't carry output.
    Core(CoreEventKind),
    /// An event that carries output.
    Output(OutputEventKind<O>),
}

/// Events that don't carry test output.
///
/// These events pass through unchanged during conversion between output
/// representations (e.g., from [`ChildSingleOutput`] to [`ZipStoreOutput`]).
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CoreEventKind {
    /// A test run started.
    #[serde(rename_all = "kebab-case")]
    RunStarted {
        /// The run ID.
        run_id: ReportUuid,
        /// The profile name.
        profile_name: String,
        /// The CLI arguments.
        cli_args: Vec<String>,
        /// The stress condition, if any.
        stress_condition: Option<StressConditionSummary>,
    },

    /// A stress sub-run started.
    #[serde(rename_all = "kebab-case")]
    StressSubRunStarted {
        /// The stress progress.
        progress: StressProgress,
    },

    /// A setup script started.
    #[serde(rename_all = "kebab-case")]
    SetupScriptStarted {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The index of this setup script.
        index: usize,
        /// The total number of setup scripts.
        total: usize,
        /// The script ID.
        script_id: ScriptId,
        /// The program being run.
        program: String,
        /// The arguments to the program.
        args: Vec<String>,
        /// Whether output capture is disabled.
        no_capture: bool,
    },

    /// A setup script is slow.
    #[serde(rename_all = "kebab-case")]
    SetupScriptSlow {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The script ID.
        script_id: ScriptId,
        /// The program being run.
        program: String,
        /// The arguments to the program.
        args: Vec<String>,
        /// The time elapsed.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        elapsed: Duration,
        /// Whether the script will be terminated.
        will_terminate: bool,
    },

    /// A test started.
    #[serde(rename_all = "kebab-case")]
    TestStarted {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// The current run statistics.
        current_stats: RunStats,
        /// The number of tests currently running.
        running: usize,
        /// The command line used to run this test.
        command_line: Vec<String>,
    },

    /// A test is slow.
    #[serde(rename_all = "kebab-case")]
    TestSlow {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// Retry data.
        retry_data: RetryData,
        /// The time elapsed.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        elapsed: Duration,
        /// Whether the test will be terminated.
        will_terminate: bool,
    },

    /// A test retry started.
    #[serde(rename_all = "kebab-case")]
    TestRetryStarted {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// Retry data.
        retry_data: RetryData,
        /// The number of tests currently running.
        running: usize,
        /// The command line used to run this test.
        command_line: Vec<String>,
    },

    /// A test was skipped.
    #[serde(rename_all = "kebab-case")]
    TestSkipped {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// The reason the test was skipped.
        reason: MismatchReason,
    },

    /// A run began being cancelled.
    #[serde(rename_all = "kebab-case")]
    RunBeginCancel {
        /// The number of setup scripts currently running.
        setup_scripts_running: usize,
        /// The number of tests currently running.
        running: usize,
        /// The reason for cancellation.
        reason: CancelReason,
    },

    /// A run was paused.
    #[serde(rename_all = "kebab-case")]
    RunPaused {
        /// The number of setup scripts currently running.
        setup_scripts_running: usize,
        /// The number of tests currently running.
        running: usize,
    },

    /// A run was continued after being paused.
    #[serde(rename_all = "kebab-case")]
    RunContinued {
        /// The number of setup scripts currently running.
        setup_scripts_running: usize,
        /// The number of tests currently running.
        running: usize,
    },

    /// A stress sub-run finished.
    #[serde(rename_all = "kebab-case")]
    StressSubRunFinished {
        /// The stress progress.
        progress: StressProgress,
        /// The time taken for this sub-run.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        sub_elapsed: Duration,
        /// The run statistics for this sub-run.
        sub_stats: RunStats,
    },

    /// A run finished.
    #[serde(rename_all = "kebab-case")]
    RunFinished {
        /// The run ID.
        run_id: ReportUuid,
        /// The start time.
        #[cfg_attr(
            test,
            strategy(crate::reporter::test_helpers::arb_datetime_fixed_offset())
        )]
        start_time: DateTime<FixedOffset>,
        /// The total elapsed time.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        elapsed: Duration,
        /// The final run statistics.
        run_stats: RunFinishedStats,
        /// Tests that were expected to run but were not seen during this run.
        outstanding_not_seen: Option<TestsNotSeenSummary>,
    },
}

/// Tests that were expected to run but were not seen during a rerun.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct TestsNotSeenSummary {
    /// A sample of test instance IDs that were not seen.
    pub not_seen: Vec<OwnedTestInstanceId>,
    /// The total number of tests not seen.
    pub total_not_seen: usize,
}

/// Events that carry test output.
///
/// These events require conversion when changing output representations
/// (e.g., from [`ChildSingleOutput`] to [`ZipStoreOutput`]).
#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + PartialEq + 'static))
)]
pub enum OutputEventKind<O> {
    /// A setup script finished.
    #[serde(rename_all = "kebab-case")]
    SetupScriptFinished {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The index of this setup script.
        index: usize,
        /// The total number of setup scripts.
        total: usize,
        /// The script ID.
        script_id: ScriptId,
        /// The program that was run.
        program: String,
        /// The arguments to the program.
        args: Vec<String>,
        /// Whether output capture was disabled.
        no_capture: bool,
        /// The execution status.
        run_status: SetupScriptExecuteStatus<O>,
    },

    /// A test attempt failed and will be retried.
    #[serde(rename_all = "kebab-case")]
    TestAttemptFailedWillRetry {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// The execution status.
        run_status: ExecuteStatus<O>,
        /// The delay before the next attempt.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        delay_before_next_attempt: Duration,
        /// How to display failure output.
        failure_output: TestOutputDisplay,
        /// The number of tests currently running.
        running: usize,
    },

    /// A test finished.
    #[serde(rename_all = "kebab-case")]
    TestFinished {
        /// The stress index, if running a stress test.
        stress_index: Option<StressIndexSummary>,
        /// The test instance.
        test_instance: OwnedTestInstanceId,
        /// How to display success output.
        success_output: TestOutputDisplay,
        /// How to display failure output.
        failure_output: TestOutputDisplay,
        /// Whether to store success output in JUnit.
        junit_store_success_output: bool,
        /// Whether to store failure output in JUnit.
        junit_store_failure_output: bool,
        /// The execution statuses.
        run_statuses: ExecutionStatuses<O>,
        /// The current run statistics.
        current_stats: RunStats,
        /// The number of tests currently running.
        running: usize,
    },
}

impl TestEventKindSummary<ChildSingleOutput> {
    fn from_test_event_kind(kind: TestEventKind<'_>) -> Option<Self> {
        Some(match kind {
            TestEventKind::RunStarted {
                run_id,
                test_list: _,
                profile_name,
                cli_args,
                stress_condition,
            } => Self::Core(CoreEventKind::RunStarted {
                run_id,
                profile_name,
                cli_args,
                stress_condition: stress_condition.map(StressConditionSummary::from),
            }),
            TestEventKind::StressSubRunStarted { progress } => {
                Self::Core(CoreEventKind::StressSubRunStarted { progress })
            }
            TestEventKind::SetupScriptStarted {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                no_capture,
            } => Self::Core(CoreEventKind::SetupScriptStarted {
                stress_index: stress_index.map(StressIndexSummary::from),
                index,
                total,
                script_id,
                program,
                args: args.to_vec(),
                no_capture,
            }),
            TestEventKind::SetupScriptSlow {
                stress_index,
                script_id,
                program,
                args,
                elapsed,
                will_terminate,
            } => Self::Core(CoreEventKind::SetupScriptSlow {
                stress_index: stress_index.map(StressIndexSummary::from),
                script_id,
                program,
                args: args.to_vec(),
                elapsed,
                will_terminate,
            }),
            TestEventKind::TestStarted {
                stress_index,
                test_instance,
                current_stats,
                running,
                command_line,
            } => Self::Core(CoreEventKind::TestStarted {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                current_stats,
                running,
                command_line,
            }),
            TestEventKind::TestSlow {
                stress_index,
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => Self::Core(CoreEventKind::TestSlow {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                retry_data,
                elapsed,
                will_terminate,
            }),
            TestEventKind::TestRetryStarted {
                stress_index,
                test_instance,
                retry_data,
                running,
                command_line,
            } => Self::Core(CoreEventKind::TestRetryStarted {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                retry_data,
                running,
                command_line,
            }),
            TestEventKind::TestSkipped {
                stress_index,
                test_instance,
                reason,
            } => Self::Core(CoreEventKind::TestSkipped {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                reason,
            }),
            TestEventKind::RunBeginCancel {
                setup_scripts_running,
                current_stats,
                running,
            } => Self::Core(CoreEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason: current_stats
                    .cancel_reason
                    .expect("RunBeginCancel event has cancel reason"),
            }),
            TestEventKind::RunPaused {
                setup_scripts_running,
                running,
            } => Self::Core(CoreEventKind::RunPaused {
                setup_scripts_running,
                running,
            }),
            TestEventKind::RunContinued {
                setup_scripts_running,
                running,
            } => Self::Core(CoreEventKind::RunContinued {
                setup_scripts_running,
                running,
            }),
            TestEventKind::StressSubRunFinished {
                progress,
                sub_elapsed,
                sub_stats,
            } => Self::Core(CoreEventKind::StressSubRunFinished {
                progress,
                sub_elapsed,
                sub_stats,
            }),
            TestEventKind::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
                outstanding_not_seen,
            } => Self::Core(CoreEventKind::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
                outstanding_not_seen: outstanding_not_seen.map(|t| TestsNotSeenSummary {
                    not_seen: t.not_seen,
                    total_not_seen: t.total_not_seen,
                }),
            }),

            TestEventKind::SetupScriptFinished {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                junit_store_success_output: _,
                junit_store_failure_output: _,
                no_capture,
                run_status,
            } => Self::Output(OutputEventKind::SetupScriptFinished {
                stress_index: stress_index.map(StressIndexSummary::from),
                index,
                total,
                script_id,
                program,
                args: args.to_vec(),
                no_capture,
                run_status,
            }),
            TestEventKind::TestAttemptFailedWillRetry {
                stress_index,
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
                running,
            } => Self::Output(OutputEventKind::TestAttemptFailedWillRetry {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                run_status,
                delay_before_next_attempt,
                failure_output,
                running,
            }),
            TestEventKind::TestFinished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
            } => Self::Output(OutputEventKind::TestFinished {
                stress_index: stress_index.map(StressIndexSummary::from),
                test_instance: test_instance.to_owned(),
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
            }),

            TestEventKind::InfoStarted { .. }
            | TestEventKind::InfoResponse { .. }
            | TestEventKind::InfoFinished { .. }
            | TestEventKind::InputEnter { .. }
            | TestEventKind::RunBeginKill { .. } => return None,
        })
    }
}

/// Serializable version of [`StressIndex`].
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct StressIndexSummary {
    /// The current stress index (0-indexed).
    pub current: u32,
    /// The total number of stress runs, if known.
    pub total: Option<NonZero<u32>>,
}

impl From<StressIndex> for StressIndexSummary {
    fn from(index: StressIndex) -> Self {
        Self {
            current: index.current,
            total: index.total,
        }
    }
}

/// Serializable version of [`StressCondition`].
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum StressConditionSummary {
    /// Run for a specific count.
    Count {
        /// The count value, or None for infinite.
        count: Option<u32>,
    },
    /// Run for a specific duration.
    Duration {
        /// The duration to run for.
        #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
        duration: Duration,
    },
}

impl From<StressCondition> for StressConditionSummary {
    fn from(condition: StressCondition) -> Self {
        use crate::runner::StressCount;
        match condition {
            StressCondition::Count(count) => Self::Count {
                count: match count {
                    StressCount::Count { count: n } => Some(n.get()),
                    StressCount::Infinite => None,
                },
            },
            StressCondition::Duration(duration) => Self::Duration { duration },
        }
    }
}

/// Output kind for content-addressed file names.
///
/// Used to determine which dictionary to use for compression and to construct
/// content-addressed file names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputKind {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
    /// Combined stdout and stderr.
    Combined,
}

impl OutputKind {
    /// Returns the string suffix for this output kind.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Combined => "combined",
        }
    }
}

/// A validated output file name in the zip archive.
///
/// File names use content-addressed format: `{content_hash}-{stdout|stderr|combined}`
/// where `content_hash` is a 16-digit hex XXH3 hash of the output content.
///
/// This enables deduplication: identical outputs produce identical file names,
/// so stress runs with many iterations store only one copy of each unique output.
///
/// This type validates the format during deserialization to prevent path
/// traversal attacks from maliciously crafted archives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputFileName(String);

impl OutputFileName {
    /// Creates a content-addressed file name from output bytes and kind.
    ///
    /// The file name is based on a hash of the content, enabling deduplication
    /// of identical outputs across stress iterations, retries, and tests.
    pub(crate) fn from_content(content: &[u8], kind: OutputKind) -> Self {
        let hash = xxhash_rust::xxh3::xxh3_64(content);
        Self(format!("{hash:016x}-{}", kind.as_str()))
    }

    /// Returns the file name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates that a string is a valid output file name.
    ///
    /// Content-addressed format: `{16_hex_chars}-{stdout|stderr|combined}`
    fn validate(s: &str) -> bool {
        if s.contains('/') || s.contains('\\') || s.contains("..") {
            return false;
        }

        let valid_suffixes = ["-stdout", "-stderr", "-combined"];
        for suffix in valid_suffixes {
            if let Some(hash_part) = s.strip_suffix(suffix)
                && hash_part.len() == 16
                && hash_part
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            {
                return true;
            }
        }

        false
    }
}

impl fmt::Display for OutputFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for OutputFileName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for OutputFileName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutputFileName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if Self::validate(&s) {
            Ok(Self(s))
        } else {
            Err(serde::de::Error::custom(format!(
                "invalid output file name: {s}"
            )))
        }
    }
}

/// Output stored as a reference to a file in the zip archive.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ZipStoreOutput {
    /// The output was empty or not captured.
    Empty,

    /// The output was stored in full.
    #[serde(rename_all = "kebab-case")]
    Full {
        /// The file name in the archive.
        file_name: OutputFileName,
    },

    /// The output was truncated to fit within size limits.
    #[serde(rename_all = "kebab-case")]
    Truncated {
        /// The file name in the archive.
        file_name: OutputFileName,
        /// The original size in bytes before truncation.
        original_size: u64,
    },
}

impl ZipStoreOutput {
    /// Returns the file name if output was stored, or `None` if empty.
    pub fn file_name(&self) -> Option<&OutputFileName> {
        match self {
            ZipStoreOutput::Empty => None,
            ZipStoreOutput::Full { file_name } | ZipStoreOutput::Truncated { file_name, .. } => {
                Some(file_name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_strategy::proptest;

    #[proptest]
    fn test_event_summary_roundtrips(value: TestEventSummary<ZipStoreOutput>) {
        let json = serde_json::to_string(&value).expect("serialization succeeds");
        let roundtrip: TestEventSummary<ZipStoreOutput> =
            serde_json::from_str(&json).expect("deserialization succeeds");
        proptest::prop_assert_eq!(value, roundtrip);
    }

    #[test]
    fn test_output_file_name_from_content_stdout() {
        let content = b"hello world";
        let file_name = OutputFileName::from_content(content, OutputKind::Stdout);

        let s = file_name.as_str();
        assert!(s.ends_with("-stdout"), "should end with -stdout: {s}");
        assert_eq!(s.len(), 16 + 1 + 6, "should be 16 hex + hyphen + 'stdout'");

        let hash_part = &s[..16];
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hash portion should be hex: {hash_part}"
        );
    }

    #[test]
    fn test_output_file_name_from_content_stderr() {
        let content = b"error message";
        let file_name = OutputFileName::from_content(content, OutputKind::Stderr);

        let s = file_name.as_str();
        assert!(s.ends_with("-stderr"), "should end with -stderr: {s}");
        assert_eq!(s.len(), 16 + 1 + 6, "should be 16 hex + hyphen + 'stderr'");
    }

    #[test]
    fn test_output_file_name_from_content_combined() {
        let content = b"combined output";
        let file_name = OutputFileName::from_content(content, OutputKind::Combined);

        let s = file_name.as_str();
        assert!(s.ends_with("-combined"), "should end with -combined: {s}");
        assert_eq!(
            s.len(),
            16 + 1 + 8,
            "should be 16 hex + hyphen + 'combined'"
        );
    }

    #[test]
    fn test_output_file_name_deterministic() {
        let content = b"deterministic content";
        let name1 = OutputFileName::from_content(content, OutputKind::Stdout);
        let name2 = OutputFileName::from_content(content, OutputKind::Stdout);
        assert_eq!(name1.as_str(), name2.as_str());
    }

    #[test]
    fn test_output_file_name_different_content_different_hash() {
        let content1 = b"content one";
        let content2 = b"content two";
        let name1 = OutputFileName::from_content(content1, OutputKind::Stdout);
        let name2 = OutputFileName::from_content(content2, OutputKind::Stdout);
        assert_ne!(name1.as_str(), name2.as_str());
    }

    #[test]
    fn test_output_file_name_same_content_different_kind() {
        let content = b"same content";
        let stdout = OutputFileName::from_content(content, OutputKind::Stdout);
        let stderr = OutputFileName::from_content(content, OutputKind::Stderr);
        assert_ne!(stdout.as_str(), stderr.as_str());

        let stdout_hash = &stdout.as_str()[..16];
        let stderr_hash = &stderr.as_str()[..16];
        assert_eq!(stdout_hash, stderr_hash);
    }

    #[test]
    fn test_output_file_name_empty_content() {
        let file_name = OutputFileName::from_content(b"", OutputKind::Stdout);
        let s = file_name.as_str();
        assert!(s.ends_with("-stdout"), "should end with -stdout: {s}");
        assert!(OutputFileName::validate(s), "should be valid: {s}");
    }

    #[test]
    fn test_output_file_name_validate_valid_content_addressed() {
        // Valid content-addressed patterns.
        assert!(OutputFileName::validate("0123456789abcdef-stdout"));
        assert!(OutputFileName::validate("fedcba9876543210-stderr"));
        assert!(OutputFileName::validate("aaaaaaaaaaaaaaaa-combined"));
        assert!(OutputFileName::validate("0000000000000000-stdout"));
        assert!(OutputFileName::validate("ffffffffffffffff-stderr"));
    }

    #[test]
    fn test_output_file_name_validate_invalid_patterns() {
        // Too short hash.
        assert!(!OutputFileName::validate("0123456789abcde-stdout"));
        assert!(!OutputFileName::validate("abc-stdout"));

        // Too long hash.
        assert!(!OutputFileName::validate("0123456789abcdef0-stdout"));

        // Invalid suffix.
        assert!(!OutputFileName::validate("0123456789abcdef-unknown"));
        assert!(!OutputFileName::validate("0123456789abcdef-out"));
        assert!(!OutputFileName::validate("0123456789abcdef"));

        // Non-hex characters in hash.
        assert!(!OutputFileName::validate("0123456789abcdeg-stdout"));
        assert!(!OutputFileName::validate("0123456789ABCDEF-stdout")); // uppercase not allowed

        // Path traversal attempts.
        assert!(!OutputFileName::validate("../0123456789abcdef-stdout"));
        assert!(!OutputFileName::validate("0123456789abcdef-stdout/"));
        assert!(!OutputFileName::validate("foo/0123456789abcdef-stdout"));
        assert!(!OutputFileName::validate("..\\0123456789abcdef-stdout"));
    }

    #[test]
    fn test_output_file_name_validate_rejects_old_format() {
        // Old identity-based format should be rejected.
        assert!(!OutputFileName::validate("test-abc123-1-stdout"));
        assert!(!OutputFileName::validate("test-abc123-s5-1-stderr"));
        assert!(!OutputFileName::validate("script-def456-stdout"));
        assert!(!OutputFileName::validate("script-def456-s3-stderr"));
    }

    #[test]
    fn test_output_file_name_serde_round_trip() {
        let content = b"test content for serde";
        let original = OutputFileName::from_content(content, OutputKind::Stdout);

        let json = serde_json::to_string(&original).expect("serialization failed");
        let deserialized: OutputFileName =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(original.as_str(), deserialized.as_str());
    }

    #[test]
    fn test_output_file_name_deserialize_invalid() {
        // Invalid patterns should fail deserialization.
        let json = r#""invalid-file-name""#;
        let result: Result<OutputFileName, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "should fail to deserialize invalid pattern"
        );

        let json = r#""test-abc123-1-stdout""#; // Old format.
        let result: Result<OutputFileName, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject old format");
    }

    #[test]
    fn test_zip_store_output_file_name() {
        let content = b"some output";
        let file_name = OutputFileName::from_content(content, OutputKind::Stdout);

        let empty = ZipStoreOutput::Empty;
        assert!(empty.file_name().is_none());

        let full = ZipStoreOutput::Full {
            file_name: file_name.clone(),
        };
        assert_eq!(
            full.file_name().map(|f| f.as_str()),
            Some(file_name.as_str())
        );

        let truncated = ZipStoreOutput::Truncated {
            file_name: file_name.clone(),
            original_size: 1000,
        };
        assert_eq!(
            truncated.file_name().map(|f| f.as_str()),
            Some(file_name.as_str())
        );
    }
}
