// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Replay infrastructure for recorded test runs.
//!
//! This module provides the [`ReplayContext`] type for converting recorded events
//! back into [`TestEvent`]s that can be displayed through the normal reporter
//! infrastructure.

use crate::{
    errors::RecordReadError,
    list::{OwnedTestInstanceId, TestInstanceId, TestList},
    record::{
        CoreEventKind, OutputEventKind, OutputFileName, RecordReader, StressConditionSummary,
        StressIndexSummary, TestEventKindSummary, TestEventSummary, ZipStoreOutput,
    },
    reporter::events::{
        ChildExecutionOutputDescription, ChildOutputDescription, ExecuteStatus, ExecutionStatuses,
        RunStats, SetupScriptExecuteStatus, StressIndex, TestEvent, TestEventKind, TestsNotSeen,
    },
    run_mode::NextestRunMode,
    runner::{StressCondition, StressCount},
    test_output::ChildSingleOutput,
};
use bytes::Bytes;
use nextest_metadata::{RustBinaryId, TestCaseName};
use std::{collections::HashSet, num::NonZero};

/// Context for replaying recorded test events.
///
/// This struct owns the data necessary to convert [`TestEventSummary`] back into
/// [`TestEvent`] for display through the normal reporter infrastructure.
///
/// The lifetime `'a` is tied to the [`TestList`] that was reconstructed from the
/// archived metadata.
pub struct ReplayContext<'a> {
    /// Set of test instances, used for lifetime ownership.
    test_data: HashSet<OwnedTestInstanceId>,

    /// The test list reconstructed from the archive.
    test_list: &'a TestList<'a>,
}

impl<'a> ReplayContext<'a> {
    /// Creates a new replay context with the given test list.
    ///
    /// The test list should be reconstructed from the archived metadata using
    /// [`TestList::from_summary`].
    pub fn new(test_list: &'a TestList<'a>) -> Self {
        Self {
            test_data: HashSet::new(),
            test_list,
        }
    }

    /// Returns the run mode.
    pub fn mode(&self) -> NextestRunMode {
        self.test_list.mode()
    }

    /// Returns the total number of tests in the archived run.
    pub fn test_count(&self) -> usize {
        self.test_list.test_count()
    }

    /// Registers a test instance.
    ///
    /// This is required for lifetime reasons. This must be called before
    /// converting events that reference this test.
    pub fn register_test(&mut self, test_instance: OwnedTestInstanceId) {
        self.test_data.insert(test_instance);
    }

    /// Looks up a test instance ID by its owned form.
    ///
    /// Returns `None` if the test was not previously registered.
    pub fn lookup_test_instance_id(
        &self,
        test_instance: &OwnedTestInstanceId,
    ) -> Option<TestInstanceId<'_>> {
        self.test_data.get(test_instance).map(|data| data.as_ref())
    }

    /// Converts a test event summary to a test event.
    ///
    /// Returns `None` for events that cannot be converted (e.g., because they
    /// reference tests that weren't registered).
    pub fn convert_event<'cx>(
        &'cx self,
        summary: &TestEventSummary<ZipStoreOutput>,
        reader: &mut RecordReader,
    ) -> Result<TestEvent<'cx>, ReplayConversionError> {
        let kind = self.convert_event_kind(&summary.kind, reader)?;
        Ok(TestEvent {
            timestamp: summary.timestamp,
            elapsed: summary.elapsed,
            kind,
        })
    }

    fn convert_event_kind<'cx>(
        &'cx self,
        kind: &TestEventKindSummary<ZipStoreOutput>,
        reader: &mut RecordReader,
    ) -> Result<TestEventKind<'cx>, ReplayConversionError> {
        match kind {
            TestEventKindSummary::Core(core) => self.convert_core_event(core),
            TestEventKindSummary::Output(output) => self.convert_output_event(output, reader),
        }
    }

    fn convert_core_event<'cx>(
        &'cx self,
        kind: &CoreEventKind,
    ) -> Result<TestEventKind<'cx>, ReplayConversionError> {
        match kind {
            CoreEventKind::RunStarted {
                run_id,
                profile_name,
                cli_args,
                stress_condition,
            } => {
                let stress_condition = stress_condition
                    .as_ref()
                    .map(convert_stress_condition)
                    .transpose()?;
                Ok(TestEventKind::RunStarted {
                    test_list: self.test_list,
                    run_id: *run_id,
                    profile_name: profile_name.clone(),
                    cli_args: cli_args.clone(),
                    stress_condition,
                })
            }

            CoreEventKind::StressSubRunStarted { progress } => {
                Ok(TestEventKind::StressSubRunStarted {
                    progress: *progress,
                })
            }

            CoreEventKind::SetupScriptStarted {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                no_capture,
            } => Ok(TestEventKind::SetupScriptStarted {
                stress_index: stress_index.as_ref().map(convert_stress_index),
                index: *index,
                total: *total,
                script_id: script_id.clone(),
                program: program.clone(),
                args: args.clone(),
                no_capture: *no_capture,
            }),

            CoreEventKind::SetupScriptSlow {
                stress_index,
                script_id,
                program,
                args,
                elapsed,
                will_terminate,
            } => Ok(TestEventKind::SetupScriptSlow {
                stress_index: stress_index.as_ref().map(convert_stress_index),
                script_id: script_id.clone(),
                program: program.clone(),
                args: args.clone(),
                elapsed: *elapsed,
                will_terminate: *will_terminate,
            }),

            CoreEventKind::TestStarted {
                stress_index,
                test_instance,
                current_stats,
                running,
                command_line,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestStarted {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    current_stats: *current_stats,
                    running: *running,
                    command_line: command_line.clone(),
                })
            }

            CoreEventKind::TestSlow {
                stress_index,
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestSlow {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    retry_data: *retry_data,
                    elapsed: *elapsed,
                    will_terminate: *will_terminate,
                })
            }

            CoreEventKind::TestRetryStarted {
                stress_index,
                test_instance,
                retry_data,
                running,
                command_line,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestRetryStarted {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    retry_data: *retry_data,
                    running: *running,
                    command_line: command_line.clone(),
                })
            }

            CoreEventKind::TestSkipped {
                stress_index,
                test_instance,
                reason,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestSkipped {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    reason: *reason,
                })
            }

            CoreEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } => {
                let stats = RunStats {
                    cancel_reason: Some(*reason),
                    ..Default::default()
                };
                Ok(TestEventKind::RunBeginCancel {
                    setup_scripts_running: *setup_scripts_running,
                    current_stats: stats,
                    running: *running,
                })
            }

            CoreEventKind::RunPaused {
                setup_scripts_running,
                running,
            } => Ok(TestEventKind::RunPaused {
                setup_scripts_running: *setup_scripts_running,
                running: *running,
            }),

            CoreEventKind::RunContinued {
                setup_scripts_running,
                running,
            } => Ok(TestEventKind::RunContinued {
                setup_scripts_running: *setup_scripts_running,
                running: *running,
            }),

            CoreEventKind::StressSubRunFinished {
                progress,
                sub_elapsed,
                sub_stats,
            } => Ok(TestEventKind::StressSubRunFinished {
                progress: *progress,
                sub_elapsed: *sub_elapsed,
                sub_stats: *sub_stats,
            }),

            CoreEventKind::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
                outstanding_not_seen,
            } => Ok(TestEventKind::RunFinished {
                run_id: *run_id,
                start_time: *start_time,
                elapsed: *elapsed,
                run_stats: *run_stats,
                outstanding_not_seen: outstanding_not_seen.as_ref().map(|t| TestsNotSeen {
                    not_seen: t.not_seen.clone(),
                    total_not_seen: t.total_not_seen,
                }),
            }),
        }
    }

    fn convert_output_event<'cx>(
        &'cx self,
        kind: &OutputEventKind<ZipStoreOutput>,
        reader: &mut RecordReader,
    ) -> Result<TestEventKind<'cx>, ReplayConversionError> {
        match kind {
            OutputEventKind::SetupScriptFinished {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                no_capture,
                run_status,
            } => Ok(TestEventKind::SetupScriptFinished {
                stress_index: stress_index.as_ref().map(convert_stress_index),
                index: *index,
                total: *total,
                script_id: script_id.clone(),
                program: program.clone(),
                args: args.clone(),
                junit_store_success_output: false,
                junit_store_failure_output: false,
                no_capture: *no_capture,
                run_status: convert_setup_script_status(run_status, reader)?,
            }),

            OutputEventKind::TestAttemptFailedWillRetry {
                stress_index,
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
                running,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestAttemptFailedWillRetry {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    run_status: convert_execute_status(run_status, reader)?,
                    delay_before_next_attempt: *delay_before_next_attempt,
                    failure_output: *failure_output,
                    running: *running,
                })
            }

            OutputEventKind::TestFinished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
            } => {
                let instance_id = self.lookup_test_instance_id(test_instance).ok_or_else(|| {
                    ReplayConversionError::TestNotFound {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                    }
                })?;
                Ok(TestEventKind::TestFinished {
                    stress_index: stress_index.as_ref().map(convert_stress_index),
                    test_instance: instance_id,
                    success_output: *success_output,
                    failure_output: *failure_output,
                    junit_store_success_output: *junit_store_success_output,
                    junit_store_failure_output: *junit_store_failure_output,
                    run_statuses: convert_execution_statuses(run_statuses, reader)?,
                    current_stats: *current_stats,
                    running: *running,
                })
            }
        }
    }
}

/// Error during replay event conversion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayConversionError {
    /// Test not found in replay context.
    #[error("test not found under `{binary_id}`: {test_name}")]
    TestNotFound {
        /// The binary ID.
        binary_id: RustBinaryId,
        /// The test name.
        test_name: TestCaseName,
    },

    /// Error reading a record.
    #[error("error reading record")]
    RecordRead(#[from] RecordReadError),

    /// Invalid stress count in recorded data.
    #[error("invalid stress count: expected non-zero value, got 0")]
    InvalidStressCount,
}

// --- Conversion helpers ---

fn convert_stress_condition(
    summary: &StressConditionSummary,
) -> Result<StressCondition, ReplayConversionError> {
    match summary {
        StressConditionSummary::Count { count } => {
            let stress_count = match count {
                Some(n) => {
                    let non_zero =
                        NonZero::new(*n).ok_or(ReplayConversionError::InvalidStressCount)?;
                    StressCount::Count { count: non_zero }
                }
                None => StressCount::Infinite,
            };
            Ok(StressCondition::Count(stress_count))
        }
        StressConditionSummary::Duration { duration } => Ok(StressCondition::Duration(*duration)),
    }
}

fn convert_stress_index(summary: &StressIndexSummary) -> StressIndex {
    StressIndex {
        current: summary.current,
        total: summary.total,
    }
}

fn convert_execute_status(
    status: &ExecuteStatus<ZipStoreOutput>,
    reader: &mut RecordReader,
) -> Result<ExecuteStatus<ChildSingleOutput>, ReplayConversionError> {
    let output = convert_child_execution_output(&status.output, reader)?;
    Ok(ExecuteStatus {
        retry_data: status.retry_data,
        output,
        result: status.result.clone(),
        start_time: status.start_time,
        time_taken: status.time_taken,
        is_slow: status.is_slow,
        delay_before_start: status.delay_before_start,
        error_summary: status.error_summary.clone(),
        output_error_slice: status.output_error_slice.clone(),
    })
}

fn convert_execution_statuses(
    statuses: &ExecutionStatuses<ZipStoreOutput>,
    reader: &mut RecordReader,
) -> Result<ExecutionStatuses<ChildSingleOutput>, ReplayConversionError> {
    let statuses: Vec<ExecuteStatus<ChildSingleOutput>> = statuses
        .iter()
        .map(|s| convert_execute_status(s, reader))
        .collect::<Result<_, _>>()?;

    Ok(ExecutionStatuses::new(statuses))
}

fn convert_setup_script_status(
    status: &SetupScriptExecuteStatus<ZipStoreOutput>,
    reader: &mut RecordReader,
) -> Result<SetupScriptExecuteStatus<ChildSingleOutput>, ReplayConversionError> {
    let output = convert_child_execution_output(&status.output, reader)?;
    Ok(SetupScriptExecuteStatus {
        output,
        result: status.result.clone(),
        start_time: status.start_time,
        time_taken: status.time_taken,
        is_slow: status.is_slow,
        env_map: status.env_map.clone(),
        error_summary: status.error_summary.clone(),
    })
}

fn convert_child_execution_output(
    output: &ChildExecutionOutputDescription<ZipStoreOutput>,
    reader: &mut RecordReader,
) -> Result<ChildExecutionOutputDescription<ChildSingleOutput>, ReplayConversionError> {
    match output {
        ChildExecutionOutputDescription::Output {
            result,
            output,
            errors,
        } => {
            let output = convert_child_output(output, reader)?;
            Ok(ChildExecutionOutputDescription::Output {
                result: result.clone(),
                output,
                errors: errors.clone(),
            })
        }
        ChildExecutionOutputDescription::StartError(err) => {
            Ok(ChildExecutionOutputDescription::StartError(err.clone()))
        }
    }
}

fn convert_child_output(
    output: &ChildOutputDescription<ZipStoreOutput>,
    reader: &mut RecordReader,
) -> Result<ChildOutputDescription<ChildSingleOutput>, ReplayConversionError> {
    match output {
        ChildOutputDescription::Split { stdout, stderr } => {
            let stdout = stdout
                .as_ref()
                .map(|o| read_output_as_child_single(reader, o))
                .transpose()?;
            let stderr = stderr
                .as_ref()
                .map(|o| read_output_as_child_single(reader, o))
                .transpose()?;
            Ok(ChildOutputDescription::Split { stdout, stderr })
        }
        ChildOutputDescription::Combined { output } => {
            let output = read_output_as_child_single(reader, output)?;
            Ok(ChildOutputDescription::Combined { output })
        }
    }
}

fn read_output_as_child_single(
    reader: &mut RecordReader,
    output: &ZipStoreOutput,
) -> Result<ChildSingleOutput, ReplayConversionError> {
    let bytes = read_output_file(reader, output.file_name().map(OutputFileName::as_str))?;
    Ok(ChildSingleOutput::from(bytes.unwrap_or_default()))
}

fn read_output_file(
    reader: &mut RecordReader,
    file_name: Option<&str>,
) -> Result<Option<Bytes>, ReplayConversionError> {
    match file_name {
        Some(name) => {
            let bytes = reader.read_output(name)?;
            Ok(Some(Bytes::from(bytes)))
        }
        None => Ok(None),
    }
}

// --- ReplayReporter ---

use crate::{
    config::overrides::CompiledDefaultFilter,
    errors::WriteEventError,
    record::{
        run_id_index::{RunIdIndex, ShortestRunIdPrefix},
        store::{RecordedRunInfo, RecordedRunStatus},
    },
    reporter::{
        DisplayReporter, DisplayReporterBuilder, FinalStatusLevel, MaxProgressRunning,
        ReporterOutput, ShowProgress, ShowTerminalProgress, StatusLevel, StatusLevels,
        TestOutputDisplay,
    },
};
use chrono::{DateTime, FixedOffset};
use quick_junit::ReportUuid;

/// Header information for a replay session.
///
/// This struct contains metadata about the recorded run being replayed,
/// which is displayed at the start of replay output.
#[derive(Clone, Debug)]
pub struct ReplayHeader {
    /// The run ID being replayed.
    pub run_id: ReportUuid,
    /// The shortest unique prefix for the run ID, used for highlighting.
    ///
    /// This is `None` if a run ID index was not provided during construction
    /// (e.g., when replaying a single run without store context).
    pub unique_prefix: Option<ShortestRunIdPrefix>,
    /// When the run started.
    pub started_at: DateTime<FixedOffset>,
    /// The status of the run.
    pub status: RecordedRunStatus,
}

impl ReplayHeader {
    /// Creates a new replay header from run info.
    ///
    /// The `run_id_index` parameter enables unique prefix highlighting similar
    /// to `cargo nextest store list`. If provided, the shortest unique prefix
    /// for this run ID will be computed and stored for highlighted display.
    pub fn new(
        run_id: ReportUuid,
        run_info: &RecordedRunInfo,
        run_id_index: Option<&RunIdIndex>,
    ) -> Self {
        let unique_prefix = run_id_index.and_then(|index| index.shortest_unique_prefix(run_id));
        Self {
            run_id,
            unique_prefix,
            started_at: run_info.started_at,
            status: run_info.status.clone(),
        }
    }
}

/// Builder for creating a [`ReplayReporter`].
#[derive(Debug)]
pub struct ReplayReporterBuilder {
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
    success_output: Option<TestOutputDisplay>,
    failure_output: Option<TestOutputDisplay>,
    should_colorize: bool,
    verbose: bool,
    show_progress: ShowProgress,
    max_progress_running: MaxProgressRunning,
    no_output_indent: bool,
}

impl Default for ReplayReporterBuilder {
    fn default() -> Self {
        Self {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
            success_output: None,
            failure_output: None,
            should_colorize: false,
            verbose: false,
            show_progress: ShowProgress::Auto,
            max_progress_running: MaxProgressRunning::default(),
            no_output_indent: false,
        }
    }
}

impl ReplayReporterBuilder {
    /// Creates a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the status level for output during the run.
    pub fn set_status_level(&mut self, status_level: StatusLevel) -> &mut Self {
        self.status_level = status_level;
        self
    }

    /// Sets the final status level for output at the end of the run.
    pub fn set_final_status_level(&mut self, final_status_level: FinalStatusLevel) -> &mut Self {
        self.final_status_level = final_status_level;
        self
    }

    /// Sets the success output display mode.
    pub fn set_success_output(&mut self, output: TestOutputDisplay) -> &mut Self {
        self.success_output = Some(output);
        self
    }

    /// Sets the failure output display mode.
    pub fn set_failure_output(&mut self, output: TestOutputDisplay) -> &mut Self {
        self.failure_output = Some(output);
        self
    }

    /// Sets whether output should be colorized.
    pub fn set_colorize(&mut self, colorize: bool) -> &mut Self {
        self.should_colorize = colorize;
        self
    }

    /// Sets whether verbose output is enabled.
    pub fn set_verbose(&mut self, verbose: bool) -> &mut Self {
        self.verbose = verbose;
        self
    }

    /// Sets the progress display mode.
    pub fn set_show_progress(&mut self, show_progress: ShowProgress) -> &mut Self {
        self.show_progress = show_progress;
        self
    }

    /// Sets the maximum number of running tests to show in progress.
    pub fn set_max_progress_running(
        &mut self,
        max_progress_running: MaxProgressRunning,
    ) -> &mut Self {
        self.max_progress_running = max_progress_running;
        self
    }

    /// Sets whether to disable output indentation.
    pub fn set_no_output_indent(&mut self, no_output_indent: bool) -> &mut Self {
        self.no_output_indent = no_output_indent;
        self
    }

    /// Builds the replay reporter with the given output destination.
    pub fn build<'a>(
        self,
        mode: NextestRunMode,
        test_count: usize,
        output: ReporterOutput<'a>,
    ) -> ReplayReporter<'a> {
        let display_reporter = DisplayReporterBuilder {
            mode,
            default_filter: CompiledDefaultFilter::for_default_config(),
            status_levels: StatusLevels {
                status_level: self.status_level,
                final_status_level: self.final_status_level,
            },
            test_count,
            success_output: self.success_output,
            failure_output: self.failure_output,
            should_colorize: self.should_colorize,
            no_capture: false,
            verbose: self.verbose,
            show_progress: self.show_progress,
            no_output_indent: self.no_output_indent,
            max_progress_running: self.max_progress_running,
            // For replay, we don't show terminal progress (OSC 9;4 codes) since
            // we're replaying events, not running live tests.
            show_term_progress: ShowTerminalProgress::No,
        }
        .build(output);

        ReplayReporter { display_reporter }
    }
}

/// Reporter for replaying recorded test runs.
///
/// This struct wraps a `DisplayReporter` configured for replay mode. It does
/// not include terminal progress reporting (OSC 9;4 codes) since replays are
/// not live test runs.
///
/// The lifetime `'a` represents the lifetime of the data backing the events.
/// Typically this is the lifetime of the [`ReplayContext`] being used to
/// convert recorded events.
pub struct ReplayReporter<'a> {
    display_reporter: DisplayReporter<'a>,
}

impl<'a> ReplayReporter<'a> {
    /// Writes the replay header to the output.
    ///
    /// This should be called before processing any recorded events to display
    /// information about the run being replayed.
    pub fn write_header(&mut self, header: &ReplayHeader) -> Result<(), WriteEventError> {
        self.display_reporter.write_replay_header(header)
    }

    /// Writes a test event to the reporter.
    pub fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        self.display_reporter.write_event(event)
    }

    /// Finishes the reporter, writing any final output.
    pub fn finish(mut self) {
        self.display_reporter.finish();
    }
}
