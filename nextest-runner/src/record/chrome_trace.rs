// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Converts recorded nextest events to Chrome Trace Event Format.
//!
//! The Chrome Trace Event Format is a JSON format understood by Chrome's
//! `chrome://tracing` and [Perfetto UI](https://ui.perfetto.dev). It provides a
//! timeline view of test parallelism and execution.
//!
//! This module operates directly on the storage format, reading
//! `TestEventSummary<RecordingSpec>` events and converting them to trace
//! events. No replay infrastructure is needed since we only need timing data.

use super::summary::{
    CoreEventKind, OutputEventKind, StressConditionSummary, StressIndexSummary,
    TestEventKindSummary, TestEventSummary, TestsNotSeenSummary,
};
use crate::{
    config::elements::FlakyResult,
    errors::{ChromeTraceError, RecordReadError},
    list::OwnedTestInstanceId,
    output_spec::RecordingSpec,
    reporter::events::{
        CancelReason, ErrorSummary, ExecuteStatus, ExecutionResultDescription, RunFinishedStats,
        RunStats, StressProgress, TestSlotAssignment,
    },
};
use chrono::{DateTime, FixedOffset};
use debug_ignore::DebugIgnore;
use nextest_metadata::{RustBinaryId, TestCaseName};
use quick_junit::ReportUuid;
use semver::Version;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
/// Controls the JSON serialization format for Chrome trace output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChromeTraceMessageFormat {
    /// JSON with no whitespace.
    Json,

    /// JSON, prettified.
    JsonPretty,
}

/// Controls how tests are grouped in the Chrome trace output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChromeTraceGroupBy {
    /// Group tests by binary: each `RustBinaryId` gets its own synthetic pid in
    /// the trace viewer, and event names show only the test name.
    Binary,

    /// Group tests by slot: all tests share a single synthetic pid, so each row
    /// in Perfetto represents a slot regardless of binary. Event names include
    /// the binary name for disambiguation.
    Slot,
}

impl ChromeTraceGroupBy {
    /// Returns the event name for a test, respecting the grouping mode.
    ///
    /// In binary mode, the name is the test name alone (the binary is encoded
    /// in the synthetic pid). In slot mode, the name is prefixed with the
    /// binary ID for disambiguation.
    fn test_event_name(self, id: &OwnedTestInstanceId) -> String {
        match self {
            Self::Binary => id.test_name.as_ref().to_string(),
            Self::Slot => format!("{} {}", id.binary_id, id.test_name.as_ref()),
        }
    }

    /// Returns the process display name for a test event's pid.
    ///
    /// In binary mode, returns the binary ID. In slot mode, returns `"tests"`
    /// (all tests share a single process).
    fn test_process_display_name(self, binary_id: &RustBinaryId) -> String {
        match self {
            Self::Binary => binary_id.to_string(),
            Self::Slot => "tests".to_string(),
        }
    }
}

/// Converts an iterator of recorded events to Chrome Trace Event Format JSON.
///
/// The output is a JSON object with `"traceEvents"` and `"displayTimeUnit"`
/// fields, suitable for loading into Chrome's tracing viewer or Perfetto UI.
///
/// # Chrome trace dimension mapping
///
/// - `pid`: For the binary grouping mode, a synthetic ID issued per
///   `RustBinaryId`. For the slot grouping mode, a single pid across all tests.
///   In both situations, run lifecycle events use pid 0, and setup scripts use
///   pid 1.
/// - `tid`: The global slot number plus TID_OFFSET. Setup scripts use their
///   script index plus TID_OFFSET.
/// - `name`: the test name or script ID.
/// - `cat`: `"test"`, `"setup-script"`, `"run"`, or `"stress"`.
/// - `ts`: event timestamps derived from `ExecuteStatus.start_time`. B/E
///   pairs are used instead of X events.
/// - `args`: relevant test metadata.
///
/// # Pause handling
///
/// We use B/E duration events instead of X events. This allows the converter to
/// split events around pause/resume boundaries: when `RunPaused` is seen, an E
/// event is emitted for every open span, and when `RunContinued` is seen, a
/// matching B event re-opens them. The result is a visible gap in the timeline
/// during pauses.
///
/// Run lifecycle events are also emitted:
///
/// - `RunStarted`/`RunFinished` produce a B/E pair spanning the entire run.
/// - `RunBeginCancel`, `RunPaused`, `RunContinued` produce process-scoped
///   instant events on the run lifecycle process.
pub fn convert_to_chrome_trace<I>(
    nextest_version: &Version,
    events: I,
    group_by: ChromeTraceGroupBy,
    message_format: ChromeTraceMessageFormat,
) -> Result<Vec<u8>, ChromeTraceError>
where
    I: IntoIterator<Item = Result<TestEventSummary<RecordingSpec>, RecordReadError>>,
{
    let mut converter = ChromeTraceConverter::new(nextest_version.clone(), group_by);

    for event_result in events {
        let event = event_result.map_err(ChromeTraceError::ReadError)?;
        converter.process_event(event)?;
    }

    converter.finish(message_format)
}

/// State of the run lifecycle bar in the trace.
#[derive(Debug)]
enum RunBarState {
    /// No run bar has been opened.
    Closed,
    /// The run bar has an open B event.
    Open,
    /// The run bar was open but is currently paused. An E event has been
    /// emitted; `reopen_all_spans` will emit a new B event on RunContinued.
    Paused,
}

/// Internal state machine that accumulates Chrome trace events.
///
/// Uses B/E (begin/end) duration events for tests, setup scripts, and the
/// run lifecycle. This allows splitting events around pause/resume
/// boundaries to show visible gaps in the timeline.
#[derive(Debug)]
struct ChromeTraceConverter {
    /// Maps test instances to their slot assignments. An entry here means the
    /// test has an open B event.
    slot_assignments: HashMap<OwnedTestInstanceId, TestSlotAssignment>,

    /// A map of running setup scripts from index to script name. An entry here
    /// means the script has an open B event. The BTreeMap ensures deterministic
    /// iteration order during pause/resume.
    running_scripts: BTreeMap<usize, String>,

    /// Whether a stress sub-run span is currently open.
    stress_subrun_open: bool,

    /// State of the run lifecycle bar.
    run_bar_state: RunBarState,

    /// How tests are grouped in the output trace.
    group_by: ChromeTraceGroupBy,

    /// A stable numeric pid for each binary ID.
    binary_pid_map: HashMap<RustBinaryId, u64>,

    /// The next pid to assign. Starts at 2 because 0 is reserved for the run
    /// lifecycle and 1 for setup scripts.
    next_pid: u64,

    /// Run metadata.
    nextest_version: Version,
    run_id: Option<ReportUuid>,
    profile_name: Option<String>,
    cli_args: Vec<String>,
    stress_condition: Option<StressConditionSummary>,

    /// Next flow ID for retry arrows connecting failed attempts to retries.
    next_flow_id: u64,

    /// Pending retry flows: test instance → flow ID. When
    /// TestAttemptFailedWillRetry emits a flow start, the ID is stored here.
    /// TestRetryStarted consumes it to emit the matching flow finish.
    pending_retry_flows: HashMap<OwnedTestInstanceId, u64>,

    /// Tracked running test count for counter events. Set from the
    /// authoritative `running` field in test events (`TestStarted`,
    /// `TestFinished`, etc.).
    running_test_count: usize,

    /// Tracked running script count for counter events. Manually
    /// incremented/decremented because `SetupScriptStarted`/`Finished` don't
    /// carry a running count. Periodically reset to the authoritative value
    /// by `RunPaused`/`RunContinued`/`RunBeginCancel` events, which do carry
    /// `setup_scripts_running`.
    running_script_count: usize,

    /// Accumulated trace events.
    trace_events: DebugIgnore<Vec<ChromeTraceEvent>>,

    /// Pids that have already had their process_name metadata event emitted.
    emitted_process_names: HashSet<u64>,

    /// (pid, tid) pairs that have already had their thread_name metadata event
    /// emitted.
    emitted_thread_names: HashSet<(u64, u64)>,
}

impl ChromeTraceConverter {
    fn new(nextest_version: Version, group_by: ChromeTraceGroupBy) -> Self {
        Self {
            slot_assignments: HashMap::new(),
            running_scripts: BTreeMap::new(),
            stress_subrun_open: false,
            run_bar_state: RunBarState::Closed,
            group_by,
            binary_pid_map: HashMap::new(),
            next_pid: FIRST_BINARY_PID,
            nextest_version,
            run_id: None,
            profile_name: None,
            cli_args: Vec::new(),
            stress_condition: None,
            next_flow_id: 0,
            pending_retry_flows: HashMap::new(),
            running_test_count: 0,
            running_script_count: 0,
            trace_events: DebugIgnore(Vec::new()),
            emitted_process_names: HashSet::new(),
            emitted_thread_names: HashSet::new(),
        }
    }

    fn process_event(
        &mut self,
        event: TestEventSummary<RecordingSpec>,
    ) -> Result<(), ChromeTraceError> {
        let timestamp = event.timestamp;
        match event.kind {
            TestEventKindSummary::Core(core) => self.process_core_event(core, timestamp)?,
            TestEventKindSummary::Output(output) => self.process_output_event(output, timestamp)?,
        }
        Ok(())
    }

    fn process_core_event(
        &mut self,
        event: CoreEventKind,
        timestamp: DateTime<FixedOffset>,
    ) -> Result<(), ChromeTraceError> {
        match event {
            CoreEventKind::RunStarted {
                run_id,
                profile_name,
                cli_args,
                stress_condition,
            } => {
                // Emit process metadata eagerly so that subsequent instant
                // events (pause, cancel) don't emit a generic name first.
                let process_name = format!("nextest run ({profile_name})");
                self.ensure_metadata_events(
                    RUN_LIFECYCLE_PID,
                    RUN_LIFECYCLE_TID,
                    &process_name,
                    "run",
                );

                let begin_args = ChromeTraceArgs::RunBegin(RunBeginArgs {
                    nextest_version: self.nextest_version.to_string(),
                    run_id: run_id.to_string(),
                    profile: profile_name.clone(),
                    cli_args: cli_args.clone(),
                    stress_condition: stress_condition.clone(),
                });

                // Open the run lifecycle bar.
                self.emit_begin(
                    RUN_LIFECYCLE_PID,
                    RUN_LIFECYCLE_TID,
                    "test run",
                    Category::Run,
                    datetime_to_microseconds(timestamp),
                    Some(begin_args),
                );
                self.run_bar_state = RunBarState::Open;
                self.run_id = Some(run_id);
                self.profile_name = Some(profile_name);
                self.cli_args = cli_args;
                self.stress_condition = stress_condition;
            }
            CoreEventKind::RunFinished {
                run_id: _,
                start_time: _,
                elapsed,
                run_stats,
                outstanding_not_seen,
            } => {
                // Close the run lifecycle bar.
                let ts_us = datetime_to_microseconds(timestamp);
                match self.run_bar_state {
                    RunBarState::Open => {
                        // Normal case: close the open B event.
                        let args = self.run_finished_args(
                            elapsed,
                            run_stats,
                            outstanding_not_seen.as_ref(),
                        );
                        self.emit_end(
                            RUN_LIFECYCLE_PID,
                            RUN_LIFECYCLE_TID,
                            "test run",
                            Category::Run,
                            ts_us,
                            Some(args),
                        );
                        self.run_bar_state = RunBarState::Closed;
                    }
                    RunBarState::Paused => {
                        // The run bar was already closed by close_all_open_spans
                        // during the pause. Re-open briefly so the run_stats
                        // args are preserved on the final E event.
                        self.emit_begin(
                            RUN_LIFECYCLE_PID,
                            RUN_LIFECYCLE_TID,
                            "test run",
                            Category::Run,
                            ts_us,
                            None,
                        );
                        let args = self.run_finished_args(
                            elapsed,
                            run_stats,
                            outstanding_not_seen.as_ref(),
                        );
                        self.emit_end(
                            RUN_LIFECYCLE_PID,
                            RUN_LIFECYCLE_TID,
                            "test run",
                            Category::Run,
                            ts_us,
                            Some(args),
                        );
                        self.run_bar_state = RunBarState::Closed;
                    }
                    RunBarState::Closed => {}
                }
            }
            CoreEventKind::RunBeginCancel {
                reason,
                setup_scripts_running,
                running,
            } => {
                self.running_test_count = running;
                self.running_script_count = setup_scripts_running;
                let ts_us = datetime_to_microseconds(timestamp);
                self.emit_counter_event(ts_us);

                self.emit_run_instant_event(
                    timestamp,
                    "cancel",
                    Some(reason),
                    running,
                    setup_scripts_running,
                );
            }
            CoreEventKind::RunPaused {
                setup_scripts_running,
                running,
            } => {
                self.running_test_count = running;
                self.running_script_count = setup_scripts_running;
                let ts_us = datetime_to_microseconds(timestamp);
                self.emit_counter_event(ts_us);

                // Close all open spans so the pause appears as a gap.
                self.close_all_open_spans(ts_us);

                self.emit_run_instant_event(
                    timestamp,
                    "paused",
                    None,
                    running,
                    setup_scripts_running,
                );
            }
            CoreEventKind::RunContinued {
                setup_scripts_running,
                running,
            } => {
                self.running_test_count = running;
                self.running_script_count = setup_scripts_running;
                let ts_us = datetime_to_microseconds(timestamp);
                self.emit_counter_event(ts_us);

                self.emit_run_instant_event(
                    timestamp,
                    "continued",
                    None,
                    running,
                    setup_scripts_running,
                );

                // Reopen all spans that were closed at pause.
                self.reopen_all_spans(ts_us);
            }
            CoreEventKind::TestStarted {
                test_instance,
                slot_assignment,
                stress_index: _,
                current_stats: _,
                running,
                command_line,
            } => {
                let pid = self.pid_for_test(&test_instance.binary_id);
                let tid = slot_assignment.global_slot + TID_OFFSET;
                let ts_us = datetime_to_microseconds(timestamp);

                let process_name = self
                    .group_by
                    .test_process_display_name(&test_instance.binary_id);
                self.ensure_metadata_events(
                    pid,
                    tid,
                    &process_name,
                    &format!("slot-{}", slot_assignment.global_slot),
                );

                let event_name = self.group_by.test_event_name(&test_instance);
                self.emit_begin(
                    pid,
                    tid,
                    &event_name,
                    Category::Test,
                    ts_us,
                    Some(ChromeTraceArgs::TestBegin(TestBeginArgs {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                        command_line,
                    })),
                );

                self.running_test_count = running;
                self.emit_counter_event(ts_us);

                self.slot_assignments.insert(test_instance, slot_assignment);
            }
            CoreEventKind::TestRetryStarted {
                test_instance,
                slot_assignment,
                stress_index: _,
                retry_data,
                running,
                command_line,
            } => {
                let pid = self.pid_for_test(&test_instance.binary_id);
                let tid = slot_assignment.global_slot + TID_OFFSET;
                let ts_us = datetime_to_microseconds(timestamp);

                // Ensure metadata is emitted for this pid/tid in case the
                // event stream starts mid-run (e.g., a truncated log that
                // begins at a retry).
                let process_name = self
                    .group_by
                    .test_process_display_name(&test_instance.binary_id);
                self.ensure_metadata_events(
                    pid,
                    tid,
                    &process_name,
                    &format!("slot-{}", slot_assignment.global_slot),
                );

                let event_name = self.group_by.test_event_name(&test_instance);
                self.emit_begin(
                    pid,
                    tid,
                    &event_name,
                    Category::Test,
                    ts_us,
                    Some(ChromeTraceArgs::TestRetryBegin(TestRetryBeginArgs {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                        attempt: retry_data.attempt,
                        total_attempts: retry_data.total_attempts,
                        command_line,
                    })),
                );

                // Complete the flow arrow from the previous failed attempt.
                if let Some(flow_id) = self.pending_retry_flows.remove(&test_instance) {
                    self.emit_flow_finish(pid, tid, ts_us, flow_id);
                }

                self.running_test_count = running;
                self.emit_counter_event(ts_us);

                self.slot_assignments.insert(test_instance, slot_assignment);
            }
            CoreEventKind::SetupScriptStarted {
                index,
                script_id,
                stress_index: _,
                total: _,
                program: _,
                args: _,
                no_capture: _,
            } => {
                let tid = index as u64 + TID_OFFSET;
                let name = script_id.to_string();
                let ts_us = datetime_to_microseconds(timestamp);

                self.ensure_metadata_events(
                    SETUP_SCRIPT_PID,
                    tid,
                    "setup-scripts",
                    &format!("script-{index}"),
                );

                self.emit_begin(
                    SETUP_SCRIPT_PID,
                    tid,
                    &name,
                    Category::SetupScript,
                    ts_us,
                    None,
                );

                self.running_script_count += 1;
                self.emit_counter_event(ts_us);

                self.running_scripts.insert(index, name);
            }
            CoreEventKind::TestSlow {
                stress_index,
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => {
                let pid = self.pid_for_test(&test_instance.binary_id);
                let tid = self.tid_for_test(&test_instance)?;
                let ts_us = datetime_to_microseconds(timestamp);

                self.trace_events.push(ChromeTraceEvent {
                    name: "slow".to_string(),
                    cat: Category::Test,
                    ph: Phase::Instant,
                    ts: ts_us,
                    pid,
                    tid,
                    s: Some(InstantScope::Thread),
                    id: None,
                    bp: None,
                    args: Some(ChromeTraceArgs::TestSlow(TestSlowArgs {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.clone(),
                        elapsed_secs: elapsed.as_secs_f64(),
                        will_terminate,
                        attempt: retry_data.attempt,
                        stress_index: stress_index.as_ref().map(StressIndexArgs::new),
                    })),
                });
            }
            CoreEventKind::StressSubRunStarted { progress } => {
                let ts_us = datetime_to_microseconds(timestamp);

                self.ensure_metadata_events(
                    RUN_LIFECYCLE_PID,
                    STRESS_SUBRUN_TID,
                    &self.run_process_name(),
                    "stress sub-runs",
                );

                self.emit_begin(
                    RUN_LIFECYCLE_PID,
                    STRESS_SUBRUN_TID,
                    "sub-run",
                    Category::Stress,
                    ts_us,
                    Some(ChromeTraceArgs::StressSubRunBegin(StressSubRunBeginArgs {
                        progress,
                    })),
                );
                self.stress_subrun_open = true;
            }
            CoreEventKind::StressSubRunFinished {
                progress,
                sub_elapsed,
                sub_stats,
            } => {
                if !self.stress_subrun_open {
                    return Err(ChromeTraceError::MissingStressSubRunStart);
                }

                let ts_us = datetime_to_microseconds(timestamp);

                self.emit_end(
                    RUN_LIFECYCLE_PID,
                    STRESS_SUBRUN_TID,
                    "sub-run",
                    Category::Stress,
                    ts_us,
                    Some(ChromeTraceArgs::StressSubRunEnd(StressSubRunEndArgs {
                        progress,
                        time_taken_ms: duration_to_millis(sub_elapsed),
                        sub_stats,
                    })),
                );
                self.stress_subrun_open = false;
            }
            CoreEventKind::SetupScriptSlow {
                stress_index,
                script_id,
                program: _,
                args: _,
                elapsed,
                will_terminate,
            } => {
                let script_name = script_id.to_string();
                let tid = self
                    .running_scripts
                    .iter()
                    .find(|(_, name)| **name == script_name)
                    .map(|(&index, _)| index as u64 + TID_OFFSET)
                    .ok_or_else(|| ChromeTraceError::MissingScriptStart {
                        script_id: script_id.clone(),
                    })?;
                let ts_us = datetime_to_microseconds(timestamp);

                self.trace_events.push(ChromeTraceEvent {
                    name: "slow".to_string(),
                    cat: Category::SetupScript,
                    ph: Phase::Instant,
                    ts: ts_us,
                    pid: SETUP_SCRIPT_PID,
                    tid,
                    s: Some(InstantScope::Thread),
                    id: None,
                    bp: None,
                    args: Some(ChromeTraceArgs::SetupScriptSlow(SetupScriptSlowArgs {
                        script_id: script_id.as_identifier().as_str().to_string(),
                        elapsed_secs: elapsed.as_secs_f64(),
                        will_terminate,
                        stress_index: stress_index.as_ref().map(StressIndexArgs::new),
                    })),
                });
            }
            // Skipped tests don't produce trace spans (they have no duration).
            CoreEventKind::TestSkipped { .. } => {}
        }
        Ok(())
    }

    fn process_output_event(
        &mut self,
        event: OutputEventKind<RecordingSpec>,
        timestamp: DateTime<FixedOffset>,
    ) -> Result<(), ChromeTraceError> {
        // Use the outer event timestamp for E events rather than computing
        // from ExecuteStatus.start_time + time_taken. The computed end time
        // doesn't account for pause duration (the process timer stops during
        // SIGTSTP), so it can land inside or before a pause gap, producing
        // broken B/E sequences.
        let end_us = datetime_to_microseconds(timestamp);

        match event {
            OutputEventKind::TestAttemptFailedWillRetry {
                stress_index,
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output: _,
                running,
            } => {
                let pid = self.pid_for_test(&test_instance.binary_id);
                let tid = self.tid_for_test(&test_instance)?;

                let end_args = self.test_end_args(
                    &test_instance,
                    &run_status,
                    stress_index.as_ref(),
                    Some(delay_before_next_attempt),
                    None,
                );

                let event_name = self.group_by.test_event_name(&test_instance);
                self.emit_end(
                    pid,
                    tid,
                    &event_name,
                    Category::Test,
                    end_us,
                    Some(end_args),
                );

                // Emit a flow start arrow to connect to the upcoming retry.
                let flow_id = self.next_flow_id;
                self.next_flow_id += 1;
                self.emit_flow_start(pid, tid, end_us, flow_id);
                self.pending_retry_flows
                    .insert(test_instance.clone(), flow_id);

                self.running_test_count = running;
                self.emit_counter_event(end_us);

                // Close the B/E pair. TestRetryStarted will open a new one.
                self.slot_assignments.remove(&test_instance);
            }
            OutputEventKind::TestFinished {
                stress_index,
                test_instance,
                run_statuses,
                success_output: _,
                failure_output: _,
                junit_store_success_output: _,
                junit_store_failure_output: _,
                junit_flaky_fail_status: _,
                current_stats,
                running,
            } => {
                // Only emit E for the last attempt; earlier attempts were
                // already closed by TestAttemptFailedWillRetry.
                let last = run_statuses.last_status();
                let pid = self.pid_for_test(&test_instance.binary_id);
                let tid = self.tid_for_test(&test_instance)?;

                // Include flaky_result only when the test was actually
                // flaky (retried and eventually passed).
                let flaky_result = (run_statuses.len() > 1 && last.result.is_success())
                    .then(|| run_statuses.flaky_result());

                let end_args = self.test_end_args(
                    &test_instance,
                    last,
                    stress_index.as_ref(),
                    None,
                    flaky_result,
                );

                let event_name = self.group_by.test_event_name(&test_instance);
                self.emit_end(
                    pid,
                    tid,
                    &event_name,
                    Category::Test,
                    end_us,
                    Some(end_args),
                );

                self.running_test_count = running;
                self.emit_counter_event(end_us);
                self.emit_results_counter_event(end_us, &current_stats);

                self.slot_assignments.remove(&test_instance);
            }
            OutputEventKind::SetupScriptFinished {
                stress_index,
                index,
                total: _,
                script_id,
                program: _,
                args: _,
                no_capture: _,
                run_status,
            } => {
                // Validate that a matching SetupScriptStarted was seen.
                if !self.running_scripts.contains_key(&index) {
                    return Err(ChromeTraceError::MissingScriptStart {
                        script_id: script_id.clone(),
                    });
                }

                let tid = index as u64 + TID_OFFSET;
                let script_id_str = script_id.as_identifier().as_str().to_string();
                let script_name = script_id.to_string();

                let end_args = ChromeTraceArgs::SetupScriptEnd(SetupScriptEndArgs {
                    script_id: script_id_str,
                    time_taken_ms: duration_to_millis(run_status.time_taken),
                    result: run_status.result.clone(),
                    is_slow: run_status.is_slow,
                    stress_index: stress_index.as_ref().map(StressIndexArgs::new),
                    error: run_status.error_summary.as_ref().map(ErrorSummaryArgs::new),
                });

                self.emit_end(
                    SETUP_SCRIPT_PID,
                    tid,
                    &script_name,
                    Category::SetupScript,
                    end_us,
                    Some(end_args),
                );

                self.running_script_count = self.running_script_count.saturating_sub(1);
                self.emit_counter_event(end_us);

                self.running_scripts.remove(&index);
            }
        }
        Ok(())
    }

    // --- Span open/close helpers for pause/resume ---

    /// Closes all open B events (tests, setup scripts, stress sub-run, run
    /// bar) at the given timestamp. Called when the run is paused.
    fn close_all_open_spans(&mut self, ts_us: f64) {
        match self.run_bar_state {
            RunBarState::Open => {
                self.emit_end(
                    RUN_LIFECYCLE_PID,
                    RUN_LIFECYCLE_TID,
                    "test run",
                    Category::Run,
                    ts_us,
                    None,
                );
                self.run_bar_state = RunBarState::Paused;
            }
            RunBarState::Paused | RunBarState::Closed => {}
        }

        if self.stress_subrun_open {
            self.emit_end(
                RUN_LIFECYCLE_PID,
                STRESS_SUBRUN_TID,
                "sub-run",
                Category::Stress,
                ts_us,
                None,
            );
        }

        self.emit_test_and_script_span_events(Phase::End, ts_us);
    }

    /// Reopens all spans that were closed by `close_all_open_spans`. Called
    /// when the run is continued after a pause.
    fn reopen_all_spans(&mut self, ts_us: f64) {
        match self.run_bar_state {
            RunBarState::Paused => {
                self.emit_begin(
                    RUN_LIFECYCLE_PID,
                    RUN_LIFECYCLE_TID,
                    "test run",
                    Category::Run,
                    ts_us,
                    None,
                );
                self.run_bar_state = RunBarState::Open;
            }
            RunBarState::Open | RunBarState::Closed => {}
        }

        if self.stress_subrun_open {
            self.emit_begin(
                RUN_LIFECYCLE_PID,
                STRESS_SUBRUN_TID,
                "sub-run",
                Category::Stress,
                ts_us,
                None,
            );
        }

        self.emit_test_and_script_span_events(Phase::Begin, ts_us);
    }

    /// Emits B or E events for all currently open test and setup script
    /// spans, in deterministic order. Tests are sorted by global_slot;
    /// scripts by index (BTreeMap iteration is already sorted).
    ///
    /// Uses `self.group_by` methods directly instead of `pid_for_test`
    /// because `self.slot_assignments` is already borrowed for iteration.
    /// `self.group_by` is `Copy`, so it can be captured independently by
    /// the closure. For `Binary` mode, pids are looked up from
    /// `self.binary_pid_map` (already assigned by `TestStarted`).
    fn emit_test_and_script_span_events(&mut self, ph: Phase, ts_us: f64) {
        // Collect test spans sorted by global_slot.
        let mut test_spans: Vec<(u64, u64, String)> = self
            .slot_assignments
            .iter()
            .map(|(id, sa)| {
                let pid = match self.group_by {
                    ChromeTraceGroupBy::Binary => *self
                        .binary_pid_map
                        .get(&id.binary_id)
                        .expect("binary pid already assigned by TestStarted"),
                    ChromeTraceGroupBy::Slot => ALL_TESTS_PID,
                };
                let tid = sa.global_slot + TID_OFFSET;
                let name = self.group_by.test_event_name(id);
                (pid, tid, name)
            })
            .collect();
        test_spans.sort_by_key(|&(_, tid, _)| tid);
        for (pid, tid, name) in &test_spans {
            self.emit_duration_event(*pid, *tid, name, Category::Test, ph, ts_us, None);
        }

        // Scripts (BTreeMap iteration is already sorted by index).
        let script_spans: Vec<(u64, String)> = self
            .running_scripts
            .iter()
            .map(|(&index, name)| (index as u64 + TID_OFFSET, name.clone()))
            .collect();
        for (tid, name) in &script_spans {
            self.emit_duration_event(
                SETUP_SCRIPT_PID,
                *tid,
                name,
                Category::SetupScript,
                ph,
                ts_us,
                None,
            );
        }
    }

    // --- Low-level event emission ---

    fn emit_begin(
        &mut self,
        pid: u64,
        tid: u64,
        name: &str,
        cat: Category,
        ts_us: f64,
        args: Option<ChromeTraceArgs>,
    ) {
        self.emit_duration_event(pid, tid, name, cat, Phase::Begin, ts_us, args);
    }

    fn emit_end(
        &mut self,
        pid: u64,
        tid: u64,
        name: &str,
        cat: Category,
        ts_us: f64,
        args: Option<ChromeTraceArgs>,
    ) {
        self.emit_duration_event(pid, tid, name, cat, Phase::End, ts_us, args);
    }

    // Parameters directly correspond to ChromeTraceEvent fields.
    #[expect(clippy::too_many_arguments)]
    fn emit_duration_event(
        &mut self,
        pid: u64,
        tid: u64,
        name: &str,
        cat: Category,
        ph: Phase,
        ts_us: f64,
        args: Option<ChromeTraceArgs>,
    ) {
        self.trace_events.push(ChromeTraceEvent {
            name: name.to_string(),
            cat,
            ph,
            ts: ts_us,
            pid,
            tid,
            s: None,
            id: None,
            bp: None,
            args,
        });
    }

    /// Emits a counter event tracking running tests and scripts.
    fn emit_counter_event(&mut self, ts_us: f64) {
        self.trace_events.push(ChromeTraceEvent {
            name: "concurrency".to_string(),
            cat: Category::Run,
            ph: Phase::Counter,
            ts: ts_us,
            pid: RUN_LIFECYCLE_PID,
            tid: 0,
            s: None,
            id: None,
            bp: None,
            args: Some(ChromeTraceArgs::Counter(CounterArgs {
                running_tests: self.running_test_count,
                running_scripts: self.running_script_count,
            })),
        });
    }

    /// Emits a counter event tracking cumulative test results. Produces a
    /// stacked area chart in Perfetto with passed/flaky/failed bands.
    fn emit_results_counter_event(&mut self, ts_us: f64, stats: &RunStats) {
        self.trace_events.push(ChromeTraceEvent {
            name: "test results".to_string(),
            cat: Category::Run,
            ph: Phase::Counter,
            ts: ts_us,
            pid: RUN_LIFECYCLE_PID,
            tid: 0,
            s: None,
            id: None,
            bp: None,
            args: Some(ChromeTraceArgs::ResultsCounter(ResultsCounterArgs {
                passed: stats.passed,
                flaky: stats.flaky,
                failed: stats.failed_count(),
            })),
        });
    }

    /// Emits a flow start event (arrow origin) for retry connections.
    fn emit_flow_start(&mut self, pid: u64, tid: u64, ts_us: f64, flow_id: u64) {
        self.trace_events.push(ChromeTraceEvent {
            name: "retry".to_string(),
            cat: Category::Test,
            ph: Phase::FlowStart,
            ts: ts_us,
            pid,
            tid,
            s: None,
            id: Some(flow_id),
            bp: None,
            args: None,
        });
    }

    /// Emits a flow finish event (arrow destination) for retry connections.
    fn emit_flow_finish(&mut self, pid: u64, tid: u64, ts_us: f64, flow_id: u64) {
        self.trace_events.push(ChromeTraceEvent {
            name: "retry".to_string(),
            cat: Category::Test,
            ph: Phase::FlowFinish,
            ts: ts_us,
            pid,
            tid,
            s: None,
            id: Some(flow_id),
            bp: Some(FlowBindingPoint::EnclosingSlice),
            args: None,
        });
    }

    /// Emits a process-scoped instant event for run lifecycle markers (cancel,
    /// pause, continue). Placed on the run lifecycle process so they appear
    /// alongside the run bar in Perfetto.
    fn emit_run_instant_event(
        &mut self,
        timestamp: DateTime<FixedOffset>,
        name: &str,
        cancel_reason: Option<CancelReason>,
        running: usize,
        setup_scripts_running: usize,
    ) {
        let pid = RUN_LIFECYCLE_PID;
        let tid = RUN_LIFECYCLE_TID;
        let ts_us = datetime_to_microseconds(timestamp);

        // Ensure the run lifecycle process metadata is emitted. Uses the
        // stored profile name if RunStarted was seen, otherwise a generic
        // fallback for truncated logs.
        self.ensure_metadata_events(pid, tid, &self.run_process_name(), "run");

        self.trace_events.push(ChromeTraceEvent {
            name: name.to_string(),
            cat: Category::Run,
            ph: Phase::Instant,
            ts: ts_us,
            pid,
            tid,
            s: Some(InstantScope::Process),
            id: None,
            bp: None,
            args: Some(ChromeTraceArgs::RunInstant(RunInstantArgs {
                running,
                setup_scripts_running,
                reason: cancel_reason,
            })),
        });
    }

    // --- Utilities ---

    /// Returns the display name for the run lifecycle process. Uses the stored
    /// profile name if `RunStarted` was seen, otherwise falls back to a generic
    /// name for truncated logs.
    fn run_process_name(&self) -> String {
        match &self.profile_name {
            Some(name) => format!("nextest run ({name})"),
            None => "nextest run".to_string(),
        }
    }

    /// Builds `TestEndArgs` from an `ExecuteStatus` and related context.
    ///
    /// `flaky_result` should be `Some` only for the final `TestFinished` event
    /// when the test was flaky (passed after retries). For
    /// `TestAttemptFailedWillRetry`, pass `None`.
    fn test_end_args(
        &self,
        test_instance: &OwnedTestInstanceId,
        status: &ExecuteStatus<RecordingSpec>,
        stress_index: Option<&StressIndexSummary>,
        delay_before_next_attempt: Option<Duration>,
        flaky_result: Option<FlakyResult>,
    ) -> ChromeTraceArgs {
        // If the test is flaky-fail, synthesize an error message. This
        // supplements (not replaces) any error from the test itself.
        let error = match (
            status.error_summary.as_ref(),
            flaky_result.and_then(|fr| {
                fr.fail_message(status.retry_data.attempt, status.retry_data.total_attempts)
            }),
        ) {
            (Some(summary), _) => Some(ErrorSummaryArgs::new(summary)),
            (None, Some(flaky_msg)) => Some(ErrorSummaryArgs {
                short_message: "flaky failure".to_string(),
                description: flaky_msg,
            }),
            (None, None) => None,
        };

        ChromeTraceArgs::TestEnd(TestEndArgs {
            binary_id: test_instance.binary_id.clone(),
            test_name: test_instance.test_name.clone(),
            time_taken_ms: duration_to_millis(status.time_taken),
            result: status.result.clone(),
            attempt: status.retry_data.attempt,
            total_attempts: status.retry_data.total_attempts,
            is_slow: status.is_slow,
            test_group: self
                .slot_assignments
                .get(test_instance)
                .map(|s| s.test_group.to_string()),
            stress_index: stress_index.map(StressIndexArgs::new),
            delay_before_start_secs: non_zero_duration_secs(status.delay_before_start),
            error,
            delay_before_next_attempt_secs: delay_before_next_attempt
                .and_then(non_zero_duration_secs),
            flaky_result,
        })
    }

    /// Builds the args for the run lifecycle E event.
    fn run_finished_args(
        &self,
        elapsed: Duration,
        run_stats: RunFinishedStats,
        outstanding_not_seen: Option<&TestsNotSeenSummary>,
    ) -> ChromeTraceArgs {
        ChromeTraceArgs::RunEnd(RunEndArgs {
            nextest_version: self.nextest_version.to_string(),
            time_taken_ms: duration_to_millis(elapsed),
            profile: self.profile_name.clone(),
            run_stats,
            outstanding_not_seen: outstanding_not_seen.map(|ns| OutstandingNotSeenArgs {
                total_not_seen: ns.total_not_seen,
            }),
        })
    }

    /// Returns the pid for a binary, assigning one if not yet mapped.
    fn pid_for_binary(&mut self, binary_id: &RustBinaryId) -> u64 {
        if let Some(&pid) = self.binary_pid_map.get(binary_id) {
            pid
        } else {
            let pid = self.next_pid;
            self.next_pid += 1;
            self.binary_pid_map.insert(binary_id.clone(), pid);
            pid
        }
    }

    /// Returns the pid to use for a test event, respecting the grouping mode.
    ///
    /// In `Binary` mode, delegates to `pid_for_binary` (one pid per binary).
    /// In `Slot` mode, returns `ALL_TESTS_PID` (all tests share one pid).
    fn pid_for_test(&mut self, binary_id: &RustBinaryId) -> u64 {
        match self.group_by {
            ChromeTraceGroupBy::Binary => self.pid_for_binary(binary_id),
            ChromeTraceGroupBy::Slot => ALL_TESTS_PID,
        }
    }

    /// Returns the tid for a test's global slot, with the offset applied.
    ///
    /// Returns an error if the test has no slot assignment (i.e. no prior
    /// `TestStarted` event was seen).
    fn tid_for_test(&self, test_instance: &OwnedTestInstanceId) -> Result<u64, ChromeTraceError> {
        match self.slot_assignments.get(test_instance) {
            Some(sa) => Ok(sa.global_slot + TID_OFFSET),
            None => Err(ChromeTraceError::MissingTestStart {
                test_name: test_instance.test_name.clone(),
                binary_id: test_instance.binary_id.clone(),
            }),
        }
    }

    /// Emits process_name, process_sort_index, thread_name, and
    /// thread_sort_index metadata events if not already emitted for the given
    /// pid/tid.
    ///
    /// The sort indexes ensure deterministic ordering in Perfetto: run
    /// lifecycle first, then setup scripts, then test binaries.
    fn ensure_metadata_events(
        &mut self,
        pid: u64,
        tid: u64,
        process_display_name: &str,
        thread_display_name: &str,
    ) {
        if !self.emitted_process_names.contains(&pid) {
            self.emit_m_event(
                pid,
                0,
                "process_name",
                ChromeTraceArgs::MetadataName(MetadataNameArgs {
                    name: process_display_name.to_string(),
                }),
            );
            self.emit_m_event(
                pid,
                0,
                "process_sort_index",
                ChromeTraceArgs::MetadataSortIndex(MetadataSortIndexArgs { sort_index: pid }),
            );
            self.emitted_process_names.insert(pid);
        }

        if !self.emitted_thread_names.contains(&(pid, tid)) {
            self.emit_m_event(
                pid,
                tid,
                "thread_name",
                ChromeTraceArgs::MetadataName(MetadataNameArgs {
                    name: thread_display_name.to_string(),
                }),
            );
            self.emit_m_event(
                pid,
                tid,
                "thread_sort_index",
                ChromeTraceArgs::MetadataSortIndex(MetadataSortIndexArgs { sort_index: tid }),
            );
            self.emitted_thread_names.insert((pid, tid));
        }
    }

    /// Emits a metadata event.
    fn emit_m_event(&mut self, pid: u64, tid: u64, event_name: &str, args: ChromeTraceArgs) {
        self.trace_events.push(ChromeTraceEvent {
            name: event_name.to_string(),
            cat: Category::Empty,
            ph: Phase::Metadata,
            ts: 0.0,
            pid,
            tid,
            s: None,
            id: None,
            bp: None,
            args: Some(args),
        });
    }

    /// Serializes accumulated events into Chrome Trace Event Format JSON.
    fn finish(self, message_format: ChromeTraceMessageFormat) -> Result<Vec<u8>, ChromeTraceError> {
        // Always emit otherData: nextest_version is always available, and the
        // remaining fields use skip_serializing_if to omit when empty.
        let other_data = ChromeTraceOtherData {
            nextest_version: self.nextest_version.to_string(),
            run_id: self.run_id.map(|id| id.to_string()),
            profile_name: self.profile_name,
            cli_args: self.cli_args,
            stress_condition: self.stress_condition,
        };

        let output = ChromeTraceOutput {
            trace_events: self.trace_events.0,
            display_time_unit: "ms",
            other_data,
        };

        let serialize_fn = match message_format {
            ChromeTraceMessageFormat::Json => serde_json::to_vec,
            ChromeTraceMessageFormat::JsonPretty => serde_json::to_vec_pretty,
        };
        serialize_fn(&output).map_err(ChromeTraceError::SerializeError)
    }
}

/// Pid reserved for run lifecycle events (appears first in the trace viewer).
const RUN_LIFECYCLE_PID: u64 = 0;

/// Pid reserved for setup scripts.
const SETUP_SCRIPT_PID: u64 = 1;

/// Pid used for all tests in `Slot` grouping mode.
const ALL_TESTS_PID: u64 = 2;

/// The first pid assigned to test binaries in `Binary` grouping mode.
/// Starts at 2 because 0 is reserved for the run lifecycle and 1 for setup
/// scripts.
const FIRST_BINARY_PID: u64 = 2;

/// Tid used for the run lifecycle thread on `RUN_LIFECYCLE_PID`.
const RUN_LIFECYCLE_TID: u64 = 1;

/// Tid used for stress sub-run spans on `RUN_LIFECYCLE_PID`.
const STRESS_SUBRUN_TID: u64 = 2;

/// Tid offset applied to all test slots and setup script indexes. Perfetto
/// treats the thread where tid == pid as the process's main thread, which
/// causes rendering artifacts. Using a large offset ensures test and
/// script tids never collide with process pids (which start at 2 and
/// increment with the number of unique binaries).
const TID_OFFSET: u64 = 10_000;

/// Chrome trace event phase (the `ph` field in the trace format).
#[derive(Copy, Clone, Serialize)]
enum Phase {
    #[serde(rename = "B")]
    Begin,
    #[serde(rename = "E")]
    End,
    #[serde(rename = "M")]
    Metadata,
    #[serde(rename = "i")]
    Instant,
    #[serde(rename = "C")]
    Counter,
    #[serde(rename = "s")]
    FlowStart,
    #[serde(rename = "f")]
    FlowFinish,
}

/// Chrome trace event category (the `cat` field in the trace format).
#[derive(Copy, Clone, Serialize)]
enum Category {
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "setup-script")]
    SetupScript,
    #[serde(rename = "stress")]
    Stress,
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "")]
    Empty,
}

/// Instant event scope (the `s` field in the trace format).
#[derive(Copy, Clone, Serialize)]
enum InstantScope {
    #[serde(rename = "p")]
    Process,
    #[serde(rename = "t")]
    Thread,
}

/// Flow event binding point (the `bp` field in the trace format).
#[derive(Copy, Clone, Serialize)]
enum FlowBindingPoint {
    #[serde(rename = "e")]
    EnclosingSlice,
}

/// A single Chrome Trace Event.
#[derive(Serialize)]
struct ChromeTraceEvent {
    /// Event name (test name or script ID).
    name: String,

    /// Event category.
    cat: Category,

    /// Event phase.
    ph: Phase,

    /// Timestamp in microseconds.
    ts: f64,

    /// Process ID (binary ID or setup-script group).
    pid: u64,

    /// Thread ID (global slot or script index).
    tid: u64,

    /// Instant event scope. Only meaningful for instant events.
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<InstantScope>,

    /// Flow event ID. Connects flow start and flow finish events.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,

    /// Flow event binding point.
    #[serde(skip_serializing_if = "Option::is_none")]
    bp: Option<FlowBindingPoint>,

    /// Typed event arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<ChromeTraceArgs>,
}

// -------------------------------------------------------------------
// Typed args for Chrome trace events
// -------------------------------------------------------------------
//
// Each variant represents the args for a specific event type. Using typed
// structs instead of `serde_json::Value` ensures field names and types are
// checked at compile time.
//
// `#[serde(untagged)]` is used for serialization only (this type is never
// deserialized, so the guideline about untagged deserializers does not apply).
// Each variant serializes as a plain JSON object.

/// Args attached to Chrome trace events.
#[derive(Serialize)]
#[serde(untagged)]
enum ChromeTraceArgs {
    RunBegin(RunBeginArgs),
    RunEnd(RunEndArgs),
    RunInstant(RunInstantArgs),
    TestBegin(TestBeginArgs),
    TestRetryBegin(TestRetryBeginArgs),
    TestEnd(TestEndArgs),
    TestSlow(TestSlowArgs),
    SetupScriptEnd(SetupScriptEndArgs),
    SetupScriptSlow(SetupScriptSlowArgs),
    StressSubRunBegin(StressSubRunBeginArgs),
    StressSubRunEnd(StressSubRunEndArgs),
    Counter(CounterArgs),
    ResultsCounter(ResultsCounterArgs),
    MetadataName(MetadataNameArgs),
    MetadataSortIndex(MetadataSortIndexArgs),
}

// --- Helper types used within args ---

/// Stress index information. Uses plain `u32` for `total` rather than
/// `NonZeroU32` to match the Chrome trace output convention.
#[derive(Serialize)]
struct StressIndexArgs {
    current: u32,
    total: Option<u32>,
}

impl StressIndexArgs {
    fn new(si: &StressIndexSummary) -> Self {
        Self {
            current: si.current,
            total: si.total.map(|t| t.get()),
        }
    }
}

/// Error summary for Chrome trace args.
#[derive(Serialize)]
struct ErrorSummaryArgs {
    short_message: String,
    description: String,
}

impl ErrorSummaryArgs {
    fn new(summary: &ErrorSummary) -> Self {
        Self {
            short_message: summary.short_message.clone(),
            description: summary.description.clone(),
        }
    }
}

/// Outstanding tests not seen, for `RunEndArgs`.
#[derive(Serialize)]
struct OutstandingNotSeenArgs {
    total_not_seen: usize,
}

// --- Run lifecycle args ---

/// Args for `RunStarted` B events.
#[derive(Serialize)]
struct RunBeginArgs {
    nextest_version: String,
    run_id: String,
    profile: String,
    cli_args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_condition: Option<StressConditionSummary>,
}

/// Args for `RunFinished` E events.
#[derive(Serialize)]
struct RunEndArgs {
    nextest_version: String,
    time_taken_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    run_stats: RunFinishedStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    outstanding_not_seen: Option<OutstandingNotSeenArgs>,
}

/// Args for run lifecycle instant events (cancel, pause, continue).
#[derive(Serialize)]
struct RunInstantArgs {
    running: usize,
    setup_scripts_running: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<CancelReason>,
}

// --- Test args ---

/// Args for `TestStarted` B events.
#[derive(Serialize)]
struct TestBeginArgs {
    binary_id: RustBinaryId,
    test_name: TestCaseName,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_line: Vec<String>,
}

/// Args for `TestRetryStarted` B events.
#[derive(Serialize)]
struct TestRetryBeginArgs {
    binary_id: RustBinaryId,
    test_name: TestCaseName,
    attempt: u32,
    total_attempts: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_line: Vec<String>,
}

/// Args for `TestFinished` and `TestAttemptFailedWillRetry` E events.
#[derive(Serialize)]
struct TestEndArgs {
    binary_id: RustBinaryId,
    test_name: TestCaseName,
    time_taken_ms: f64,
    result: ExecutionResultDescription,
    attempt: u32,
    total_attempts: u32,
    is_slow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_index: Option<StressIndexArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_before_start_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorSummaryArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_before_next_attempt_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flaky_result: Option<FlakyResult>,
}

/// Args for `TestSlow` instant events.
#[derive(Serialize)]
struct TestSlowArgs {
    binary_id: RustBinaryId,
    test_name: TestCaseName,
    elapsed_secs: f64,
    will_terminate: bool,
    attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_index: Option<StressIndexArgs>,
}

// --- Setup script args ---

/// Args for `SetupScriptFinished` E events.
#[derive(Serialize)]
struct SetupScriptEndArgs {
    script_id: String,
    time_taken_ms: f64,
    result: ExecutionResultDescription,
    is_slow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_index: Option<StressIndexArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorSummaryArgs>,
}

/// Args for `SetupScriptSlow` instant events.
#[derive(Serialize)]
struct SetupScriptSlowArgs {
    script_id: String,
    elapsed_secs: f64,
    will_terminate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_index: Option<StressIndexArgs>,
}

// --- Stress args ---

/// Args for `StressSubRunStarted` B events.
#[derive(Serialize)]
struct StressSubRunBeginArgs {
    progress: StressProgress,
}

/// Args for `StressSubRunFinished` E events.
#[derive(Serialize)]
struct StressSubRunEndArgs {
    progress: StressProgress,
    time_taken_ms: f64,
    sub_stats: RunStats,
}

// --- Counter and metadata args ---

/// Args for counter events tracking running tests and scripts.
#[derive(Serialize)]
struct CounterArgs {
    running_tests: usize,
    running_scripts: usize,
}

/// Args for counter events tracking cumulative test results. Produces a
/// stacked area chart with passed (clean), flaky, and failed bands.
#[derive(Serialize)]
struct ResultsCounterArgs {
    /// Tests that passed on the first attempt (excludes flaky).
    passed: usize,
    /// Tests that passed on retry.
    flaky: usize,
    /// Tests that failed all attempts, including exec failures.
    failed: usize,
}

/// Args for `process_name` and `thread_name` metadata events.
///
/// Field names are defined by the Chrome Trace Event Format spec.
#[derive(Serialize)]
struct MetadataNameArgs {
    name: String,
}

/// Args for `process_sort_index` and `thread_sort_index` metadata events.
///
/// Field names are defined by the Chrome Trace Event Format spec.
#[derive(Serialize)]
struct MetadataSortIndexArgs {
    sort_index: u64,
}

/// Top-level Chrome Trace Event Format output.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")] // required by the Chrome Trace Event Format spec
struct ChromeTraceOutput {
    trace_events: Vec<ChromeTraceEvent>,
    display_time_unit: &'static str,
    /// Arbitrary key/value data included at the top level of the trace output.
    /// Uses the spec-defined `otherData` field for session-level metadata.
    /// Always emitted because `nextest_version` is always available.
    other_data: ChromeTraceOtherData,
}

/// Session-level data included in the `otherData` field of the trace output.
#[derive(Serialize)]
struct ChromeTraceOtherData {
    nextest_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cli_args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stress_condition: Option<StressConditionSummary>,
}

/// Converts a `DateTime<FixedOffset>` to microseconds since the Unix epoch.
fn datetime_to_microseconds(dt: DateTime<FixedOffset>) -> f64 {
    // Use timestamp_micros() which is infallible for all valid DateTime values,
    // unlike timestamp_nanos_opt() which overflows outside ~1677-2262.
    // The i64 → f64 cast is exact for timestamps through ~year 2255 (2^53 µs).
    dt.timestamp_micros() as f64
}

/// Converts a `Duration` to fractional milliseconds.
fn duration_to_millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Returns `Some(secs)` if the duration is non-zero, `None` otherwise.
fn non_zero_duration_secs(d: Duration) -> Option<f64> {
    if d.is_zero() {
        None
    } else {
        Some(d.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            elements::{FlakyResult, JunitFlakyFailStatus, TestGroup},
            scripts::ScriptId,
        },
        list::OwnedTestInstanceId,
        output_spec::RecordingSpec,
        record::summary::{
            CoreEventKind, OutputEventKind, TestEventKindSummary, TestEventSummary,
            ZipStoreOutputDescription,
        },
        reporter::{
            TestOutputDisplay,
            events::{
                ChildExecutionOutputDescription, ExecuteStatus, ExecutionResultDescription,
                ExecutionStatuses, FailureDescription, RetryData, RunFinishedStats, RunStats,
                SetupScriptExecuteStatus, TestSlotAssignment,
            },
        },
        runner::StressCount,
    };
    use chrono::{FixedOffset, TimeZone};
    use nextest_metadata::{RustBinaryId, TestCaseName};
    use std::{
        collections::{BTreeSet, HashMap},
        num::NonZero,
        time::Duration,
    };

    /// Asserts that begin and end events are balanced per (pid, tid, cat)
    /// triple. Every begin must have a matching end. This catches unbalanced
    /// spans that would render as infinite-length bars in Perfetto.
    fn assert_be_balanced(trace_events: &[serde_json::Value]) {
        let mut b_counts: HashMap<(u64, u64, &str), usize> = HashMap::new();
        let mut e_counts: HashMap<(u64, u64, &str), usize> = HashMap::new();

        for event in trace_events {
            let ph = event["ph"].as_str().unwrap_or("");
            let cat = event["cat"].as_str().unwrap_or("");
            let pid = event["pid"].as_u64().unwrap_or(0);
            let tid = event["tid"].as_u64().unwrap_or(0);
            match ph {
                "B" => *b_counts.entry((pid, tid, cat)).or_default() += 1,
                "E" => *e_counts.entry((pid, tid, cat)).or_default() += 1,
                _ => {}
            }
        }

        // Collect all keys from both maps.
        let all_keys: BTreeSet<_> = b_counts.keys().chain(e_counts.keys()).copied().collect();

        for key in all_keys {
            let b = b_counts.get(&key).copied().unwrap_or(0);
            let e = e_counts.get(&key).copied().unwrap_or(0);
            assert_eq!(
                b, e,
                "B/E mismatch for (pid={}, tid={}, cat={:?}): {} B events vs {} E events",
                key.0, key.1, key.2, b, e,
            );
        }
    }

    fn test_version() -> Version {
        Version::new(0, 9, 9999)
    }

    /// Creates a fixed timestamp at the given number of seconds after the
    /// epoch.
    fn ts(secs: i64) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .timestamp_opt(secs, 0)
            .unwrap()
    }

    fn test_id(binary: &str, test: &str) -> OwnedTestInstanceId {
        OwnedTestInstanceId {
            binary_id: RustBinaryId::new(binary),
            test_name: TestCaseName::new(test),
        }
    }

    fn slot(global: u64) -> TestSlotAssignment {
        TestSlotAssignment {
            global_slot: global,
            group_slot: None,
            test_group: TestGroup::Global,
        }
    }

    fn core_event(
        timestamp: DateTime<FixedOffset>,
        kind: CoreEventKind,
    ) -> TestEventSummary<RecordingSpec> {
        TestEventSummary {
            timestamp,
            elapsed: Duration::ZERO,
            kind: TestEventKindSummary::Core(kind),
        }
    }

    fn output_event(
        timestamp: DateTime<FixedOffset>,
        kind: OutputEventKind<RecordingSpec>,
    ) -> TestEventSummary<RecordingSpec> {
        TestEventSummary {
            timestamp,
            elapsed: Duration::ZERO,
            kind: TestEventKindSummary::Output(kind),
        }
    }

    fn empty_output() -> ChildExecutionOutputDescription<RecordingSpec> {
        ChildExecutionOutputDescription::Output {
            result: Some(ExecutionResultDescription::Pass),
            output: ZipStoreOutputDescription::Split {
                stdout: None,
                stderr: None,
            },
            errors: None,
        }
    }

    fn passing_status(
        start_time: DateTime<FixedOffset>,
        time_taken: Duration,
        attempt: u32,
        total_attempts: u32,
    ) -> ExecuteStatus<RecordingSpec> {
        ExecuteStatus {
            retry_data: RetryData {
                attempt,
                total_attempts,
            },
            output: empty_output(),
            result: ExecutionResultDescription::Pass,
            start_time,
            time_taken,
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        }
    }

    fn failing_status(
        start_time: DateTime<FixedOffset>,
        time_taken: Duration,
        attempt: u32,
        total_attempts: u32,
    ) -> ExecuteStatus<RecordingSpec> {
        ExecuteStatus {
            retry_data: RetryData {
                attempt,
                total_attempts,
            },
            output: empty_output(),
            result: ExecutionResultDescription::Fail {
                failure: FailureDescription::ExitCode { code: 1 },
                leaked: false,
            },
            start_time,
            time_taken,
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        }
    }

    /// Converts events and parses the resulting JSON, returning the top-level
    /// object and the `traceEvents` array. Automatically asserts B/E balance.
    fn convert_and_parse(
        events: Vec<Result<TestEventSummary<RecordingSpec>, RecordReadError>>,
        group_by: ChromeTraceGroupBy,
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        let json_bytes = convert_to_chrome_trace(
            &test_version(),
            events,
            group_by,
            ChromeTraceMessageFormat::JsonPretty,
        )
        .expect("conversion succeeded");
        let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).expect("valid JSON");
        let trace_events = parsed["traceEvents"]
            .as_array()
            .expect("traceEvents is an array")
            .clone();
        assert_be_balanced(&trace_events);
        (parsed, trace_events)
    }

    fn run_started(timestamp: DateTime<FixedOffset>) -> TestEventSummary<RecordingSpec> {
        core_event(
            timestamp,
            CoreEventKind::RunStarted {
                run_id: quick_junit::ReportUuid::nil(),
                profile_name: "default".to_string(),
                cli_args: vec![],
                stress_condition: None,
            },
        )
    }

    fn run_finished(
        timestamp: DateTime<FixedOffset>,
        start_time: DateTime<FixedOffset>,
        elapsed: Duration,
    ) -> TestEventSummary<RecordingSpec> {
        core_event(
            timestamp,
            CoreEventKind::RunFinished {
                run_id: quick_junit::ReportUuid::nil(),
                start_time,
                elapsed,
                run_stats: RunFinishedStats::Single(RunStats::default()),
                outstanding_not_seen: None,
            },
        )
    }

    fn test_started(
        timestamp: DateTime<FixedOffset>,
        binary: &str,
        test: &str,
        global_slot: u64,
        running: usize,
    ) -> TestEventSummary<RecordingSpec> {
        core_event(
            timestamp,
            CoreEventKind::TestStarted {
                stress_index: None,
                test_instance: test_id(binary, test),
                slot_assignment: slot(global_slot),
                current_stats: RunStats::default(),
                running,
                command_line: vec![],
            },
        )
    }

    fn test_finished_pass(
        timestamp: DateTime<FixedOffset>,
        binary: &str,
        test: &str,
        start_time: DateTime<FixedOffset>,
        time_taken: Duration,
        running: usize,
    ) -> TestEventSummary<RecordingSpec> {
        output_event(
            timestamp,
            OutputEventKind::TestFinished {
                stress_index: None,
                test_instance: test_id(binary, test),
                success_output: TestOutputDisplay::Never,
                failure_output: TestOutputDisplay::Never,
                junit_store_success_output: false,
                junit_store_failure_output: false,
                junit_flaky_fail_status: JunitFlakyFailStatus::default(),
                run_statuses: ExecutionStatuses::new(
                    vec![passing_status(start_time, time_taken, 1, 1)],
                    FlakyResult::Pass,
                ),
                current_stats: RunStats::default(),
                running,
            },
        )
    }

    #[test]
    fn basic_test_run() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(
                ts(1000),
                "my-crate::bin/my-test",
                "tests::basic",
                0,
                1,
            )),
            Ok(test_finished_pass(
                ts(1001),
                "my-crate::bin/my-test",
                "tests::basic",
                ts(1000),
                Duration::from_millis(500),
                0,
            )),
            Ok(run_finished(ts(1002), ts(1000), Duration::from_secs(2))),
        ];

        let (parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // B/E pairs: run bar (B+E) + test (B+E) = 4 duration events.
        // Metadata: 2 process_name + 2 thread_name + 2 process_sort_index
        //   + 2 thread_sort_index = 8.
        // Concurrency counter: TestStarted + TestFinished = 2.
        // Results counter: TestFinished = 1.
        // Total = 15.
        assert_eq!(
            trace_events.len(),
            15,
            "expected 15 trace events, got: {trace_events:#?}"
        );

        // Find test B/E pair.
        let test_begins: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "test")
            .collect();
        let test_ends: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "test")
            .collect();
        assert_eq!(test_begins.len(), 1);
        assert_eq!(test_ends.len(), 1);

        let test_b = test_begins[0];
        assert_eq!(test_b["name"], "tests::basic");
        assert_eq!(test_b["pid"], 2); // First binary gets pid 2.
        assert_eq!(test_b["tid"], TID_OFFSET); // Global slot 0 + TID_OFFSET.

        // B timestamp should be the TestStarted event time = 1000s = 1e9 us.
        let b_ts = test_b["ts"].as_f64().unwrap();
        assert!(
            (b_ts - 1_000_000_000.0).abs() < 1.0,
            "expected ~1e9 us, got {b_ts}"
        );

        // E timestamp should be the outer event timestamp ts(1001) = 1001s.
        let test_e = test_ends[0];
        let e_ts = test_e["ts"].as_f64().unwrap();
        assert!(
            (e_ts - 1_001_000_000.0).abs() < 1.0,
            "expected ~1001000000 us, got {e_ts}"
        );

        // Result args should be on the E event.
        assert_eq!(test_e["args"]["attempt"], 1);

        // Run bar B/E.
        let run_begins: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let run_ends: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();
        assert_eq!(run_begins.len(), 1);
        assert_eq!(run_ends.len(), 1);
        assert_eq!(run_ends[0]["args"]["profile"], "default");

        assert_eq!(parsed["displayTimeUnit"], "ms");

        // otherData should include run information.
        let other_data = &parsed["otherData"];
        assert_eq!(other_data["profile_name"], "default");
        assert!(!other_data["run_id"].is_null(), "runId should be present");
        assert!(
            !other_data["nextest_version"].is_null(),
            "nextest version should be present"
        );

        // Sort index metadata events.
        let proc_sort: Vec<_> = trace_events
            .iter()
            .filter(|e| e["name"] == "process_sort_index")
            .collect();
        assert!(
            proc_sort.len() >= 2,
            "should have sort indexes for run lifecycle and test binary"
        );

        // Run lifecycle should have sort_index 0.
        let run_sort = proc_sort
            .iter()
            .find(|e| e["pid"] == 0)
            .expect("run lifecycle sort index");
        assert_eq!(run_sort["args"]["sort_index"], 0);

        // Test binary (pid 2) should have sort_index 2.
        let binary_sort = proc_sort
            .iter()
            .find(|e| e["pid"] == 2)
            .expect("binary sort index");
        assert_eq!(binary_sort["args"]["sort_index"], 2);

        // Thread sort indexes should be emitted.
        let thread_sort: Vec<_> = trace_events
            .iter()
            .filter(|e| e["name"] == "thread_sort_index")
            .collect();
        assert!(!thread_sort.is_empty(), "should have thread sort indexes");
    }

    #[test]
    fn setup_script() {
        let script_id = ScriptId::new("db-setup".into()).expect("valid script ID");

        let events = vec![
            Ok(core_event(
                ts(1000),
                CoreEventKind::SetupScriptStarted {
                    stress_index: None,
                    index: 0,
                    total: 1,
                    script_id: script_id.clone(),
                    program: "/bin/setup".to_string(),
                    args: vec![],
                    no_capture: false,
                },
            )),
            // Slow event at T=1005.
            Ok(core_event(
                ts(1005),
                CoreEventKind::SetupScriptSlow {
                    stress_index: None,
                    script_id: script_id.clone(),
                    program: "/bin/setup".to_string(),
                    args: vec![],
                    elapsed: Duration::from_secs(5),
                    will_terminate: true,
                },
            )),
            Ok(output_event(
                ts(1010),
                OutputEventKind::SetupScriptFinished {
                    stress_index: None,
                    index: 0,
                    total: 1,
                    script_id: script_id.clone(),
                    program: "/bin/setup".to_string(),
                    args: vec![],
                    no_capture: false,
                    run_status: SetupScriptExecuteStatus {
                        output: empty_output(),
                        result: ExecutionResultDescription::Pass,
                        start_time: ts(1000),
                        time_taken: Duration::from_secs(10),
                        is_slow: true,
                        env_map: None,
                        error_summary: None,
                    },
                },
            )),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        let b_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "setup-script")
            .collect();
        let e_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "setup-script")
            .collect();
        assert_eq!(b_events.len(), 1);
        assert_eq!(e_events.len(), 1);

        assert_eq!(b_events[0]["name"], "db-setup");
        assert_eq!(b_events[0]["pid"], 1); // Setup scripts use pid 1.
        assert_eq!(b_events[0]["tid"], TID_OFFSET); // Script index 0 + TID_OFFSET.

        // E event should have the result args.
        assert_eq!(e_events[0]["args"]["script_id"], "db-setup");

        // Slow instant event should be emitted.
        let slow_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "i" && e["name"] == "slow")
            .collect();
        assert_eq!(slow_events.len(), 1, "expected 1 slow instant event");

        let slow = slow_events[0];
        assert_eq!(slow["cat"], "setup-script");
        assert_eq!(slow["s"], "t", "should be thread-scoped");
        assert_eq!(slow["args"]["will_terminate"], true);
        assert_eq!(slow["args"]["elapsed_secs"], 5.0);
        assert_eq!(slow["args"]["script_id"], "db-setup");
        assert_eq!(slow["pid"], SETUP_SCRIPT_PID);
    }

    #[test]
    fn empty_run_produces_only_run_lifecycle() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(run_finished(ts(1000), ts(1000), Duration::ZERO)),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Run lifecycle: 1 process_name + 1 process_sort_index +
        //   1 thread_name + 1 thread_sort_index + B + E = 6.
        // No test events.
        assert_eq!(
            trace_events.len(),
            6,
            "empty run should produce only run lifecycle events, got: {trace_events:#?}"
        );

        let b_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let e_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();
        assert_eq!(b_events.len(), 1);
        assert_eq!(e_events.len(), 1);
        assert_eq!(b_events[0]["name"], "test run");
    }

    /// Verifies that pause/resume creates gaps in the timeline by splitting
    /// B/E events. Also checks instant event markers and cancel events.
    #[test]
    fn pause_resume_splits_events() {
        let events = vec![
            // Uses non-default profile "ci" and cli_args, so kept explicit.
            Ok(core_event(
                ts(1000),
                CoreEventKind::RunStarted {
                    run_id: quick_junit::ReportUuid::nil(),
                    profile_name: "ci".to_string(),
                    cli_args: vec!["--run-ignored".to_string()],
                    stress_condition: None,
                },
            )),
            Ok(test_started(ts(1000), "crate::bin/test", "test_a", 0, 1)),
            // Pause at T=1001.
            Ok(core_event(
                ts(1001),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Resume at T=1005.
            Ok(core_event(
                ts(1005),
                CoreEventKind::RunContinued {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            Ok(test_finished_pass(
                ts(1010),
                "crate::bin/test",
                "test_a",
                ts(1000),
                Duration::from_secs(10),
                0,
            )),
            Ok(core_event(
                ts(1011),
                CoreEventKind::RunBeginCancel {
                    setup_scripts_running: 0,
                    running: 0,
                    reason: CancelReason::TestFailure,
                },
            )),
            Ok(run_finished(ts(1012), ts(1000), Duration::from_secs(12))),
        ];

        let (parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // The test should have 2 B events (start + resume) and 2 E events
        // (pause + finish), creating a gap from T=1001 to T=1005.
        let test_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "test")
            .collect();
        let test_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "test")
            .collect();
        assert_eq!(test_b.len(), 2, "expected 2 test B events (start + resume)");
        assert_eq!(test_e.len(), 2, "expected 2 test E events (pause + finish)");

        // First segment: B at T=1000, E at T=1001 (pause).
        let b1_ts = test_b[0]["ts"].as_f64().unwrap();
        let e1_ts = test_e[0]["ts"].as_f64().unwrap();
        assert!(
            (b1_ts - 1_000_000_000.0).abs() < 1.0,
            "first B should be at T=1000"
        );
        assert!(
            (e1_ts - 1_001_000_000.0).abs() < 1.0,
            "first E should be at T=1001 (pause)"
        );

        // Second segment: B at T=1005 (resume), E at T=1010 (finish).
        let b2_ts = test_b[1]["ts"].as_f64().unwrap();
        let e2_ts = test_e[1]["ts"].as_f64().unwrap();
        assert!(
            (b2_ts - 1_005_000_000.0).abs() < 1.0,
            "second B should be at T=1005 (resume)"
        );
        assert!(
            (e2_ts - 1_010_000_000.0).abs() < 1.0,
            "second E should be at T=1010 (finish)"
        );

        // The run bar should also be split: 2 B + 2 E for the run.
        let run_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let run_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();
        assert_eq!(run_b.len(), 2, "expected 2 run B events (start + resume)");
        assert_eq!(run_e.len(), 2, "expected 2 run E events (pause + finish)");

        // Instant events: paused, continued, cancel.
        let instant_events: Vec<_> = trace_events.iter().filter(|e| e["ph"] == "i").collect();
        assert_eq!(instant_events.len(), 3);
        let names: Vec<&str> = instant_events
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["paused", "continued", "cancel"]);

        // Cancel event should include the reason.
        assert_eq!(instant_events[2]["args"]["reason"], "test-failure");

        // Process metadata should show the profile name.
        let process_meta: Vec<_> = trace_events
            .iter()
            .filter(|e| e["name"] == "process_name" && e["pid"] == 0)
            .collect();
        assert_eq!(process_meta.len(), 1);
        assert_eq!(process_meta[0]["args"]["name"], "nextest run (ci)");

        // otherData should use otherData (not metadata), include version,
        // profile, and CLI args.
        assert!(parsed["metadata"].is_null(), "metadata should not exist");
        let other_data = &parsed["otherData"];
        assert!(!other_data.is_null(), "otherData should exist");
        let version = other_data["nextest_version"].as_str().unwrap();
        assert!(!version.is_empty(), "nextest version should be non-empty");
        assert_eq!(other_data["profile_name"], "ci");
        assert_eq!(other_data["cli_args"][0], "--run-ignored");
    }

    /// Verifies that pause/resume correctly splits setup script B/E events,
    /// not just test events.
    #[test]
    fn pause_resume_with_setup_scripts() {
        let script_id = ScriptId::new("db-setup".into()).expect("valid script ID");

        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(core_event(
                ts(1000),
                CoreEventKind::SetupScriptStarted {
                    stress_index: None,
                    index: 0,
                    total: 1,
                    script_id: script_id.clone(),
                    program: "/bin/setup".to_string(),
                    args: vec![],
                    no_capture: false,
                },
            )),
            // Pause while script is running.
            Ok(core_event(
                ts(1002),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 1,
                    running: 0,
                },
            )),
            // Resume.
            Ok(core_event(
                ts(1005),
                CoreEventKind::RunContinued {
                    setup_scripts_running: 1,
                    running: 0,
                },
            )),
            // Script finishes.
            Ok(output_event(
                ts(1010),
                OutputEventKind::SetupScriptFinished {
                    stress_index: None,
                    index: 0,
                    total: 1,
                    script_id: script_id.clone(),
                    program: "/bin/setup".to_string(),
                    args: vec![],
                    no_capture: false,
                    run_status: SetupScriptExecuteStatus {
                        output: empty_output(),
                        result: ExecutionResultDescription::Pass,
                        start_time: ts(1000),
                        time_taken: Duration::from_secs(10),
                        is_slow: false,
                        env_map: None,
                        error_summary: None,
                    },
                },
            )),
            Ok(run_finished(ts(1012), ts(1000), Duration::from_secs(12))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // The setup script should have 2 B events (start + resume) and 2 E
        // events (pause + finish), creating a gap from T=1002 to T=1005.
        let script_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "setup-script")
            .collect();
        let script_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "setup-script")
            .collect();
        assert_eq!(
            script_b.len(),
            2,
            "expected 2 setup-script B events (start + resume)"
        );
        assert_eq!(
            script_e.len(),
            2,
            "expected 2 setup-script E events (pause + finish)"
        );

        // First segment: B at T=1000, E at T=1002 (pause).
        let b1_ts = script_b[0]["ts"].as_f64().unwrap();
        let e1_ts = script_e[0]["ts"].as_f64().unwrap();
        assert!(
            (b1_ts - 1_000_000_000.0).abs() < 1.0,
            "first B should be at T=1000"
        );
        assert!(
            (e1_ts - 1_002_000_000.0).abs() < 1.0,
            "first E should be at T=1002 (pause)"
        );

        // Second segment: B at T=1005 (resume).
        let b2_ts = script_b[1]["ts"].as_f64().unwrap();
        assert!(
            (b2_ts - 1_005_000_000.0).abs() < 1.0,
            "second B should be at T=1005 (resume)"
        );

        // All setup script events should use SETUP_SCRIPT_PID.
        for e in script_b.iter().chain(script_e.iter()) {
            assert_eq!(
                e["pid"], 1,
                "setup script events should use SETUP_SCRIPT_PID"
            );
        }

        // The run bar should also be split.
        let run_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let run_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();
        assert_eq!(run_b.len(), 2, "expected 2 run B events (start + resume)");
        assert_eq!(run_e.len(), 2, "expected 2 run E events (pause + finish)");
    }

    /// The run bar E event should use the RunFinished event's wall-clock
    /// timestamp, not `RunFinished.elapsed` (which is monotonic). This
    /// ensures the run bar is consistent with test events' coordinate system.
    #[test]
    fn run_bar_uses_event_timestamp() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(ts(1000), "crate::bin/test", "slow_test", 0, 1)),
            Ok(test_finished_pass(
                ts(1010),
                "crate::bin/test",
                "slow_test",
                ts(1000),
                Duration::from_secs(10),
                0,
            )),
            // RunFinished event timestamp is ts(1012) (wall-clock), but
            // elapsed is only 8 seconds (monotonic). The run bar E should
            // use the wall-clock timestamp ts(1012).
            Ok(run_finished(ts(1012), ts(1000), Duration::from_secs(8))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Run bar B should be at ts(1000), E should be at ts(1012).
        let run_b = trace_events
            .iter()
            .find(|e| e["ph"] == "B" && e["cat"] == "run")
            .expect("run B event");
        let run_e = trace_events
            .iter()
            .find(|e| e["ph"] == "E" && e["cat"] == "run")
            .expect("run E event");

        let run_b_ts = run_b["ts"].as_f64().unwrap();
        let run_e_ts = run_e["ts"].as_f64().unwrap();

        // Run bar should span ts(1000) → ts(1012) = 12 seconds, not 8.
        assert!(
            (run_b_ts - 1_000_000_000.0).abs() < 1.0,
            "run B should be at ts(1000)"
        );
        assert!(
            (run_e_ts - 1_012_000_000.0).abs() < 1.0,
            "run E should be at ts(1012), not ts(1008)"
        );

        // The test E should not extend past the run E.
        let test_e = trace_events
            .iter()
            .find(|e| e["ph"] == "E" && e["cat"] == "test")
            .expect("test E event");
        let test_e_ts = test_e["ts"].as_f64().unwrap();
        assert!(
            test_e_ts <= run_e_ts,
            "test E ({test_e_ts}) should not exceed run E ({run_e_ts})"
        );
    }

    /// RunFinished arriving while paused (without RunContinued) should
    /// produce well-formed B/E pairs by re-opening the run bar briefly.
    #[test]
    fn run_finished_while_paused() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(ts(1000), "crate::bin/test", "test_a", 0, 1)),
            // Pause at T=1002.
            Ok(core_event(
                ts(1002),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // RunFinished without RunContinued (e.g., process killed during
            // pause, or truncated log).
            Ok(run_finished(ts(1005), ts(1000), Duration::from_secs(5))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Run bar should have matching B/E pairs. The pause emits an E, and
        // RunFinished should re-open with a B and close with an E.
        let run_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let run_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();

        // Should be 2 B events (RunStarted + reopen at RunFinished) and
        // 2 E events (pause + RunFinished close).
        assert_eq!(run_b.len(), 2, "expected 2 run B events, got {run_b:#?}");
        assert_eq!(run_e.len(), 2, "expected 2 run E events, got {run_e:#?}");

        // The final E should have run_stats args.
        let last_e = run_e.last().unwrap();
        assert!(
            !last_e["args"]["run_stats"].is_null(),
            "final E should have run_stats"
        );

        // Every B/E pair should be properly nested (each B must precede its
        // corresponding E).
        for (b, e) in run_b.iter().zip(run_e.iter()) {
            let b_ts = b["ts"].as_f64().unwrap();
            let e_ts = e["ts"].as_f64().unwrap();
            assert!(
                b_ts <= e_ts,
                "B timestamp ({b_ts}) should not exceed E timestamp ({e_ts})"
            );
        }
    }

    /// Snapshot test for retry flow events, covering both `FlakyResult::Pass`
    /// and `FlakyResult::Fail`. Verifies B/E pairs, attempt tracking, flow
    /// arrows, and the flaky-fail error synthesis.
    #[test]
    fn snapshot_retry_flow_events() {
        let events = vec![
            // --- Flaky-pass test on slot 0: fails once, passes on retry ---
            Ok(test_started(
                ts(1000),
                "crate::bin/test",
                "flaky_pass",
                0,
                1,
            )),
            Ok(output_event(
                ts(1001),
                OutputEventKind::TestAttemptFailedWillRetry {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_pass"),
                    run_status: failing_status(ts(1000), Duration::from_millis(200), 1, 3),
                    delay_before_next_attempt: Duration::ZERO,
                    failure_output: TestOutputDisplay::Never,
                    running: 1,
                },
            )),
            Ok(core_event(
                ts(1001),
                CoreEventKind::TestRetryStarted {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_pass"),
                    slot_assignment: slot(0),
                    retry_data: RetryData {
                        attempt: 2,
                        total_attempts: 3,
                    },
                    running: 1,
                    command_line: vec![],
                },
            )),
            Ok(output_event(
                ts(1002),
                OutputEventKind::TestFinished {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_pass"),
                    success_output: TestOutputDisplay::Never,
                    failure_output: TestOutputDisplay::Never,
                    junit_store_success_output: false,
                    junit_store_failure_output: false,
                    junit_flaky_fail_status: JunitFlakyFailStatus::default(),
                    run_statuses: ExecutionStatuses::new(
                        vec![
                            failing_status(ts(1000), Duration::from_millis(200), 1, 3),
                            passing_status(ts(1001), Duration::from_millis(300), 2, 3),
                        ],
                        FlakyResult::Pass,
                    ),
                    current_stats: RunStats::default(),
                    running: 0,
                },
            )),
            // --- Flaky-fail test on slot 1: fails once, passes on retry,
            //     but configured to treat flaky as failure ---
            Ok(test_started(
                ts(1003),
                "crate::bin/test",
                "flaky_fail",
                1,
                1,
            )),
            Ok(output_event(
                ts(1004),
                OutputEventKind::TestAttemptFailedWillRetry {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_fail"),
                    run_status: failing_status(ts(1003), Duration::from_millis(150), 1, 2),
                    delay_before_next_attempt: Duration::ZERO,
                    failure_output: TestOutputDisplay::Never,
                    running: 1,
                },
            )),
            Ok(core_event(
                ts(1004),
                CoreEventKind::TestRetryStarted {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_fail"),
                    slot_assignment: slot(1),
                    retry_data: RetryData {
                        attempt: 2,
                        total_attempts: 2,
                    },
                    running: 1,
                    command_line: vec![],
                },
            )),
            Ok(output_event(
                ts(1005),
                OutputEventKind::TestFinished {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky_fail"),
                    success_output: TestOutputDisplay::Never,
                    failure_output: TestOutputDisplay::Never,
                    junit_store_success_output: false,
                    junit_store_failure_output: false,
                    junit_flaky_fail_status: JunitFlakyFailStatus::default(),
                    run_statuses: ExecutionStatuses::new(
                        vec![
                            failing_status(ts(1003), Duration::from_millis(150), 1, 2),
                            passing_status(ts(1004), Duration::from_millis(250), 2, 2),
                        ],
                        FlakyResult::Fail,
                    ),
                    current_stats: RunStats::default(),
                    running: 0,
                },
            )),
        ];

        let result = convert_to_chrome_trace(
            &test_version(),
            events,
            ChromeTraceGroupBy::Binary,
            ChromeTraceMessageFormat::JsonPretty,
        );
        let json_bytes = result.expect("conversion succeeded");
        let json_str = String::from_utf8(json_bytes).expect("valid UTF-8");

        insta::assert_snapshot!("retry_flow_chrome_trace", json_str);
    }

    /// Verifies that retry flow events work correctly across pause boundaries.
    #[test]
    fn retry_across_pause_boundary() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(ts(1000), "crate::bin/test", "flaky", 0, 1)),
            // First attempt fails.
            Ok(output_event(
                ts(1001),
                OutputEventKind::TestAttemptFailedWillRetry {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky"),
                    run_status: failing_status(ts(1000), Duration::from_millis(200), 1, 2),
                    delay_before_next_attempt: Duration::from_secs(1),
                    failure_output: TestOutputDisplay::Never,
                    running: 1,
                },
            )),
            // Pause during the retry delay.
            Ok(core_event(
                ts(1002),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Resume.
            Ok(core_event(
                ts(1005),
                CoreEventKind::RunContinued {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Retry starts after resume.
            Ok(core_event(
                ts(1005),
                CoreEventKind::TestRetryStarted {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky"),
                    slot_assignment: slot(0),
                    retry_data: RetryData {
                        attempt: 2,
                        total_attempts: 2,
                    },
                    running: 1,
                    command_line: vec![],
                },
            )),
            Ok(output_event(
                ts(1006),
                OutputEventKind::TestFinished {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "flaky"),
                    success_output: TestOutputDisplay::Never,
                    failure_output: TestOutputDisplay::Never,
                    junit_store_success_output: false,
                    junit_store_failure_output: false,
                    junit_flaky_fail_status: JunitFlakyFailStatus::default(),
                    run_statuses: ExecutionStatuses::new(
                        vec![
                            failing_status(ts(1000), Duration::from_millis(200), 1, 2),
                            passing_status(ts(1005), Duration::from_millis(300), 2, 2),
                        ],
                        FlakyResult::Pass,
                    ),
                    current_stats: RunStats::default(),
                    running: 0,
                },
            )),
            Ok(run_finished(ts(1007), ts(1000), Duration::from_secs(7))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Flow events should still be paired correctly despite the pause.
        let flow_starts: Vec<_> = trace_events.iter().filter(|e| e["ph"] == "s").collect();
        let flow_finishes: Vec<_> = trace_events.iter().filter(|e| e["ph"] == "f").collect();
        assert_eq!(flow_starts.len(), 1);
        assert_eq!(flow_finishes.len(), 1);
        assert_eq!(
            flow_starts[0]["id"].as_u64(),
            flow_finishes[0]["id"].as_u64(),
            "flow events should be paired"
        );

        // Test B/E events should be split across the pause: 2 B + 2 E from
        // attempt 1 (start + close at AttemptFailed), plus the pause
        // doesn't affect the slot_assignments (already removed). Retry gets
        // its own B (after resume) + E (finish).
        // Actually, attempt 1: B at start, E at AttemptFailedWillRetry
        // (removes from slot_assignments). Pause has nothing to close for
        // this test. Retry: B at RetryStarted, E at TestFinished.
        let test_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "test")
            .collect();
        let test_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "test")
            .collect();
        assert_eq!(test_b.len(), 2, "2 B events: initial + retry");
        assert_eq!(test_e.len(), 2, "2 E events: attempt failed + finished");
    }

    /// Verifies that TestSlow emits a thread-scoped instant event.
    #[test]
    fn test_slow_instant_event() {
        let events = vec![
            Ok(test_started(ts(1000), "crate::bin/test", "slow_test", 0, 1)),
            Ok(core_event(
                ts(1005),
                CoreEventKind::TestSlow {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "slow_test"),
                    retry_data: RetryData {
                        attempt: 1,
                        total_attempts: 1,
                    },
                    elapsed: Duration::from_secs(5),
                    will_terminate: false,
                },
            )),
            Ok(test_finished_pass(
                ts(1010),
                "crate::bin/test",
                "slow_test",
                ts(1000),
                Duration::from_secs(10),
                0,
            )),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Find the slow instant event.
        let slow_events: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "i" && e["name"] == "slow")
            .collect();
        assert_eq!(slow_events.len(), 1, "expected 1 slow instant event");

        let slow = slow_events[0];
        assert_eq!(slow["cat"], "test");
        assert_eq!(slow["s"], "t", "should be thread-scoped");
        assert_eq!(slow["args"]["will_terminate"], false);
        assert_eq!(slow["args"]["elapsed_secs"], 5.0);
        assert_eq!(slow["args"]["attempt"], 1);

        // Should be on the same pid/tid as the test.
        let test_b = trace_events
            .iter()
            .find(|e| e["ph"] == "B" && e["cat"] == "test")
            .expect("test B event");
        assert_eq!(slow["pid"], test_b["pid"]);
        assert_eq!(slow["tid"], test_b["tid"]);
    }

    #[test]
    fn snapshot_basic_trace() {
        // A small representative trace for snapshot testing.
        let events = vec![
            Ok(test_started(
                ts(1000),
                "my-crate::bin/my-test",
                "tests::it_works",
                0,
                1,
            )),
            // The outer timestamp (ts(1002)) deliberately differs from
            // start_time + time_taken (ts(1000) + 500ms = ts(1000.5)) to
            // verify that the E event uses the outer timestamp.
            Ok(test_finished_pass(
                ts(1002),
                "my-crate::bin/my-test",
                "tests::it_works",
                ts(1000),
                Duration::from_millis(500),
                0,
            )),
        ];

        let result = convert_to_chrome_trace(
            &test_version(),
            events,
            ChromeTraceGroupBy::Binary,
            ChromeTraceMessageFormat::JsonPretty,
        );
        let json_bytes = result.expect("conversion succeeded");
        let json_str = String::from_utf8(json_bytes).expect("valid UTF-8");

        insta::assert_snapshot!("basic_chrome_trace", json_str);
    }

    #[test]
    fn snapshot_pause_resume_trace() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(
                ts(1000),
                "my-crate::bin/my-test",
                "tests::slow",
                0,
                1,
            )),
            // Pause at T=1002.
            Ok(core_event(
                ts(1002),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Resume at T=1008.
            Ok(core_event(
                ts(1008),
                CoreEventKind::RunContinued {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // The outer timestamp (ts(1010)) deliberately differs from
            // start_time + time_taken (ts(1000) + 4s = ts(1004)). The
            // process timer doesn't advance during the 6-second pause
            // (T=1002..1008), so time_taken is 4s, not 10s. The E event
            // should use the outer timestamp ts(1010), not ts(1004).
            Ok(test_finished_pass(
                ts(1010),
                "my-crate::bin/my-test",
                "tests::slow",
                ts(1000),
                Duration::from_secs(4),
                0,
            )),
            Ok(run_finished(ts(1010), ts(1000), Duration::from_secs(10))),
        ];

        let result = convert_to_chrome_trace(
            &test_version(),
            events,
            ChromeTraceGroupBy::Binary,
            ChromeTraceMessageFormat::JsonPretty,
        );
        let json_bytes = result.expect("conversion succeeded");
        let json_str = String::from_utf8(json_bytes).expect("valid UTF-8");

        insta::assert_snapshot!("pause_resume_chrome_trace", json_str);
    }

    /// Helper to create a count-based `StressProgress` value.
    fn stress_progress(completed: u32, total: u32) -> StressProgress {
        StressProgress::Count {
            total: StressCount::Count {
                count: NonZero::new(total).expect("total is non-zero"),
            },
            elapsed: Duration::from_secs(completed as u64),
            completed,
        }
    }

    /// Verifies that stress sub-run events produce B/E pairs on the run
    /// lifecycle process.
    #[test]
    fn stress_subrun_events() {
        let events = vec![
            Ok(run_started(ts(1000))),
            // First sub-run.
            Ok(core_event(
                ts(1000),
                CoreEventKind::StressSubRunStarted {
                    progress: stress_progress(0, 3),
                },
            )),
            Ok(test_started(ts(1000), "crate::bin/test", "test_a", 0, 1)),
            Ok(test_finished_pass(
                ts(1002),
                "crate::bin/test",
                "test_a",
                ts(1000),
                Duration::from_secs(2),
                0,
            )),
            Ok(core_event(
                ts(1002),
                CoreEventKind::StressSubRunFinished {
                    progress: stress_progress(1, 3),
                    sub_elapsed: Duration::from_secs(2),
                    sub_stats: RunStats::default(),
                },
            )),
            // Second sub-run.
            Ok(core_event(
                ts(1003),
                CoreEventKind::StressSubRunStarted {
                    progress: stress_progress(1, 3),
                },
            )),
            Ok(test_started(ts(1003), "crate::bin/test", "test_a", 0, 1)),
            Ok(test_finished_pass(
                ts(1005),
                "crate::bin/test",
                "test_a",
                ts(1003),
                Duration::from_secs(2),
                0,
            )),
            Ok(core_event(
                ts(1005),
                CoreEventKind::StressSubRunFinished {
                    progress: stress_progress(2, 3),
                    sub_elapsed: Duration::from_secs(2),
                    sub_stats: RunStats::default(),
                },
            )),
            Ok(run_finished(ts(1006), ts(1000), Duration::from_secs(6))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Should have 2 sub-run B/E pairs.
        let subrun_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "stress")
            .collect();
        let subrun_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "stress")
            .collect();
        assert_eq!(subrun_b.len(), 2, "expected 2 sub-run B events");
        assert_eq!(subrun_e.len(), 2, "expected 2 sub-run E events");

        // All sub-run events should be on the run lifecycle pid with the
        // stress sub-run tid.
        for e in subrun_b.iter().chain(subrun_e.iter()) {
            assert_eq!(e["pid"], RUN_LIFECYCLE_PID);
            assert_eq!(e["tid"], STRESS_SUBRUN_TID);
            assert_eq!(e["name"], "sub-run");
        }

        // First B should have progress args.
        assert!(
            !subrun_b[0]["args"]["progress"].is_null(),
            "sub-run B should have progress args"
        );

        // E events should have sub_stats.
        assert!(
            !subrun_e[0]["args"]["sub_stats"].is_null(),
            "sub-run E should have sub-stats"
        );

        // Verify metadata events were emitted for the stress sub-run tid.
        let thread_meta: Vec<_> = trace_events
            .iter()
            .filter(|e| {
                e["name"] == "thread_name"
                    && e["pid"] == RUN_LIFECYCLE_PID
                    && e["tid"] == STRESS_SUBRUN_TID
            })
            .collect();
        assert_eq!(
            thread_meta.len(),
            1,
            "stress sub-run thread_name metadata should be emitted once"
        );
        assert_eq!(thread_meta[0]["args"]["name"], "stress sub-runs");
    }

    /// Verifies that pause/resume correctly splits stress sub-run spans,
    /// creating a visible gap in the timeline.
    #[test]
    fn pause_resume_with_stress_subrun() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(core_event(
                ts(1000),
                CoreEventKind::StressSubRunStarted {
                    progress: stress_progress(0, 2),
                },
            )),
            Ok(test_started(ts(1000), "crate::bin/test", "test_a", 0, 1)),
            // Pause at T=1002, while both the test and sub-run are active.
            Ok(core_event(
                ts(1002),
                CoreEventKind::RunPaused {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Resume at T=1005.
            Ok(core_event(
                ts(1005),
                CoreEventKind::RunContinued {
                    setup_scripts_running: 0,
                    running: 1,
                },
            )),
            // Test finishes after resume.
            Ok(test_finished_pass(
                ts(1008),
                "crate::bin/test",
                "test_a",
                ts(1000),
                Duration::from_secs(5),
                0,
            )),
            Ok(core_event(
                ts(1008),
                CoreEventKind::StressSubRunFinished {
                    progress: stress_progress(1, 2),
                    sub_elapsed: Duration::from_secs(5),
                    sub_stats: RunStats::default(),
                },
            )),
            Ok(run_finished(ts(1009), ts(1000), Duration::from_secs(9))),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // The sub-run should have 2 B events (start + resume) and 2 E events
        // (pause + finish), creating a gap from T=1002 to T=1005.
        let subrun_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "stress")
            .collect();
        let subrun_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "stress")
            .collect();
        assert_eq!(
            subrun_b.len(),
            2,
            "expected 2 sub-run B events (start + resume)"
        );
        assert_eq!(
            subrun_e.len(),
            2,
            "expected 2 sub-run E events (pause + finish)"
        );

        // First segment: B at T=1000, E at T=1002 (pause).
        let b1_ts = subrun_b[0]["ts"].as_f64().unwrap();
        let e1_ts = subrun_e[0]["ts"].as_f64().unwrap();
        assert!(
            (b1_ts - 1_000_000_000.0).abs() < 1.0,
            "first sub-run B should be at T=1000"
        );
        assert!(
            (e1_ts - 1_002_000_000.0).abs() < 1.0,
            "first sub-run E should be at T=1002 (pause)"
        );

        // Second segment: B at T=1005 (resume), E at T=1008 (finish).
        let b2_ts = subrun_b[1]["ts"].as_f64().unwrap();
        let e2_ts = subrun_e[1]["ts"].as_f64().unwrap();
        assert!(
            (b2_ts - 1_005_000_000.0).abs() < 1.0,
            "second sub-run B should be at T=1005 (resume)"
        );
        assert!(
            (e2_ts - 1_008_000_000.0).abs() < 1.0,
            "second sub-run E should be at T=1008 (finish)"
        );

        // The test should also be split: 2 B + 2 E.
        let test_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "test")
            .collect();
        let test_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "test")
            .collect();
        assert_eq!(test_b.len(), 2, "expected 2 test B events (start + resume)");
        assert_eq!(test_e.len(), 2, "expected 2 test E events (pause + finish)");

        // The run bar should also be split.
        let run_b: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "B" && e["cat"] == "run")
            .collect();
        let run_e: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "E" && e["cat"] == "run")
            .collect();
        assert_eq!(run_b.len(), 2, "expected 2 run B events (start + resume)");
        assert_eq!(run_e.len(), 2, "expected 2 run E events (pause + finish)");
    }

    /// Verifies that the "test results" counter emits cumulative
    /// pass/flaky/failed counts from `current_stats`.
    #[test]
    fn results_counter_events() {
        // Build stats that represent: 3 passed (including 1 flaky), 1 failed,
        // 1 exec_failed. The counter should show passed=3, flaky=1, failed=2.
        let stats = RunStats {
            initial_run_count: 5,
            finished_count: 5,
            passed: 3,
            flaky: 1,
            failed: 1,
            exec_failed: 1,
            ..RunStats::default()
        };

        let events = vec![
            Ok(test_started(ts(1000), "crate::bin/test", "test_a", 0, 1)),
            Ok(output_event(
                ts(1001),
                OutputEventKind::TestFinished {
                    stress_index: None,
                    test_instance: test_id("crate::bin/test", "test_a"),
                    success_output: TestOutputDisplay::Never,
                    failure_output: TestOutputDisplay::Never,
                    junit_store_success_output: false,
                    junit_store_failure_output: false,
                    junit_flaky_fail_status: JunitFlakyFailStatus::default(),
                    run_statuses: ExecutionStatuses::new(
                        vec![passing_status(ts(1000), Duration::from_millis(500), 1, 1)],
                        FlakyResult::Pass,
                    ),
                    current_stats: stats,
                    running: 0,
                },
            )),
        ];

        let (_parsed, trace_events) = convert_and_parse(events, ChromeTraceGroupBy::Binary);

        // Find the "test results" counter event.
        let results_counters: Vec<_> = trace_events
            .iter()
            .filter(|e| e["ph"] == "C" && e["name"] == "test results")
            .collect();
        assert_eq!(
            results_counters.len(),
            1,
            "expected 1 results counter event"
        );

        let args = &results_counters[0]["args"];
        // passed = 3.
        assert_eq!(args["passed"], 3);
        // flaky = 1.
        assert_eq!(args["flaky"], 1);
        // failed = 1 failed + 1 exec_failed = 2.
        assert_eq!(args["failed"], 2, "failed should include exec_failed");
    }

    /// Verifies both grouping modes for multi-binary runs: binary mode assigns
    /// different pids per binary, while slot mode groups all tests under a
    /// single pid with qualified names.
    #[test]
    fn multiple_binaries_grouping_modes() {
        let make_events = || {
            vec![
                Ok(run_started(ts(1000))),
                Ok(test_started(
                    ts(1000),
                    "crate-a::bin/test-a",
                    "test_1",
                    0,
                    1,
                )),
                Ok(test_started(
                    ts(1000),
                    "crate-b::bin/test-b",
                    "test_2",
                    1,
                    2,
                )),
                Ok(test_finished_pass(
                    ts(1001),
                    "crate-a::bin/test-a",
                    "test_1",
                    ts(1000),
                    Duration::from_millis(100),
                    1,
                )),
                Ok(test_finished_pass(
                    ts(1001),
                    "crate-b::bin/test-b",
                    "test_2",
                    ts(1000),
                    Duration::from_millis(100),
                    0,
                )),
                Ok(run_finished(ts(1002), ts(1000), Duration::from_secs(2))),
            ]
        };

        // Binary mode: different binaries get different pids.
        {
            let (_parsed, trace_events) =
                convert_and_parse(make_events(), ChromeTraceGroupBy::Binary);

            let b_events: Vec<_> = trace_events
                .iter()
                .filter(|e| e["ph"] == "B" && e["cat"] == "test")
                .collect();
            assert_eq!(b_events.len(), 2);

            // Different binaries should have different pids, assigned in
            // order starting from FIRST_BINARY_PID.
            assert_eq!(b_events[0]["pid"], FIRST_BINARY_PID);
            assert_eq!(b_events[1]["pid"], FIRST_BINARY_PID + 1);
        }

        // Slot mode: all tests share ALL_TESTS_PID with qualified names.
        {
            let (_parsed, trace_events) =
                convert_and_parse(make_events(), ChromeTraceGroupBy::Slot);

            // All test B events should share ALL_TESTS_PID.
            let test_b: Vec<_> = trace_events
                .iter()
                .filter(|e| e["ph"] == "B" && e["cat"] == "test")
                .collect();
            assert_eq!(test_b.len(), 2);
            for b in &test_b {
                assert_eq!(
                    b["pid"], ALL_TESTS_PID,
                    "slot mode: all tests should share ALL_TESTS_PID"
                );
            }

            // Event names should include the binary ID.
            let names: Vec<&str> = test_b.iter().map(|e| e["name"].as_str().unwrap()).collect();
            assert!(
                names.contains(&"crate-a::bin/test-a test_1"),
                "expected qualified name for test_1, got: {names:?}"
            );
            assert!(
                names.contains(&"crate-b::bin/test-b test_2"),
                "expected qualified name for test_2, got: {names:?}"
            );

            // E event names should also be qualified.
            let test_e: Vec<_> = trace_events
                .iter()
                .filter(|e| e["ph"] == "E" && e["cat"] == "test")
                .collect();
            for e in &test_e {
                assert_eq!(
                    e["pid"], ALL_TESTS_PID,
                    "slot mode: all test E events should share ALL_TESTS_PID"
                );
                let name = e["name"].as_str().unwrap();
                assert!(
                    name.contains("crate-a::bin/test-a") || name.contains("crate-b::bin/test-b"),
                    "E event name should be qualified: {name}"
                );
            }

            // Process metadata for the tests pid should be "tests".
            let proc_names: Vec<_> = trace_events
                .iter()
                .filter(|e| e["name"] == "process_name" && e["pid"] == ALL_TESTS_PID)
                .collect();
            assert_eq!(proc_names.len(), 1);
            assert_eq!(proc_names[0]["args"]["name"], "tests");
        }
    }

    #[test]
    fn snapshot_slot_mode_chrome_trace() {
        let events = vec![
            Ok(run_started(ts(1000))),
            Ok(test_started(
                ts(1000),
                "crate-a::bin/test-a",
                "test_alpha",
                0,
                1,
            )),
            Ok(test_started(
                ts(1000),
                "crate-b::bin/test-b",
                "test_beta",
                1,
                2,
            )),
            Ok(test_finished_pass(
                ts(1002),
                "crate-a::bin/test-a",
                "test_alpha",
                ts(1000),
                Duration::from_millis(500),
                1,
            )),
            Ok(test_finished_pass(
                ts(1003),
                "crate-b::bin/test-b",
                "test_beta",
                ts(1000),
                Duration::from_millis(800),
                0,
            )),
            Ok(run_finished(ts(1004), ts(1000), Duration::from_secs(4))),
        ];

        let result = convert_to_chrome_trace(
            &test_version(),
            events,
            ChromeTraceGroupBy::Slot,
            ChromeTraceMessageFormat::JsonPretty,
        );
        let json_bytes = result.expect("conversion succeeded");
        let json_str = String::from_utf8(json_bytes).expect("valid UTF-8");

        insta::assert_snapshot!("slot_mode_chrome_trace", json_str);
    }
}
