// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Events for the reporter.
//!
//! These types form the interface between the test runner and the test
//! reporter. The root structure for all events is [`TestEvent`].

use super::{FinalStatusLevel, StatusLevel, TestOutputDisplay};
use crate::{
    config::{
        elements::{LeakTimeoutResult, SlowTimeoutResult},
        scripts::ScriptId,
    },
    errors::{ChildError, ChildFdError, ChildStartError, ErrorList},
    list::{OwnedTestInstanceId, TestInstanceId, TestList},
    runner::{StressCondition, StressCount},
    test_output::{ChildExecutionOutput, ChildOutput, ChildSingleOutput},
};
use chrono::{DateTime, FixedOffset};
use nextest_metadata::MismatchReason;
use quick_junit::ReportUuid;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    collections::BTreeMap, ffi::c_int, fmt, num::NonZero, process::ExitStatus, time::Duration,
};

/// The signal number for SIGTERM.
///
/// This is 15 on all platforms. We define it here rather than using `SIGTERM` because
/// `SIGTERM` is not available on Windows, but the value is platform-independent.
pub const SIGTERM: c_int = 15;

/// A reporter event.
#[derive(Clone, Debug)]
pub enum ReporterEvent<'a> {
    /// A periodic tick.
    Tick,

    /// A test event.
    Test(Box<TestEvent<'a>>),
}
/// A test event.
///
/// Events are produced by a [`TestRunner`](crate::runner::TestRunner) and
/// consumed by a [`Reporter`](crate::reporter::Reporter).
#[derive(Clone, Debug)]
pub struct TestEvent<'a> {
    /// The time at which the event was generated, including the offset from UTC.
    pub timestamp: DateTime<FixedOffset>,

    /// The amount of time elapsed since the start of the test run.
    pub elapsed: Duration,

    /// The kind of test event this is.
    pub kind: TestEventKind<'a>,
}

/// The kind of test event this is.
///
/// Forms part of [`TestEvent`].
#[derive(Clone, Debug)]
pub enum TestEventKind<'a> {
    /// The test run started.
    RunStarted {
        /// The list of tests that will be run.
        ///
        /// The methods on the test list indicate the number of tests that will be run.
        test_list: &'a TestList<'a>,

        /// The UUID for this run.
        run_id: ReportUuid,

        /// The nextest profile chosen for this run.
        profile_name: String,

        /// The command-line arguments for the process.
        cli_args: Vec<String>,

        /// The stress condition for this run, if any.
        stress_condition: Option<StressCondition>,
    },

    /// When running stress tests serially, a sub-run started.
    StressSubRunStarted {
        /// The amount of progress completed so far.
        progress: StressProgress,
    },

    /// A setup script started.
    SetupScriptStarted {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The setup script index.
        index: usize,

        /// The total number of setup scripts.
        total: usize,

        /// The script ID.
        script_id: ScriptId,

        /// The program to run.
        program: String,

        /// The arguments to the program.
        args: Vec<String>,

        /// True if some output from the setup script is being passed through.
        no_capture: bool,
    },

    /// A setup script was slow.
    SetupScriptSlow {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The script ID.
        script_id: ScriptId,

        /// The program to run.
        program: String,

        /// The arguments to the program.
        args: Vec<String>,

        /// The amount of time elapsed since the start of execution.
        elapsed: Duration,

        /// True if the script has hit its timeout and is about to be terminated.
        will_terminate: bool,
    },

    /// A setup script completed execution.
    SetupScriptFinished {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The setup script index.
        index: usize,

        /// The total number of setup scripts.
        total: usize,

        /// The script ID.
        script_id: ScriptId,

        /// The program to run.
        program: String,

        /// The arguments to the program.
        args: Vec<String>,

        /// Whether the JUnit report should store success output for this script.
        junit_store_success_output: bool,

        /// Whether the JUnit report should store failure output for this script.
        junit_store_failure_output: bool,

        /// True if some output from the setup script was passed through.
        no_capture: bool,

        /// The execution status of the setup script.
        run_status: SetupScriptExecuteStatus<ChildSingleOutput>,
    },

    // TODO: add events for BinaryStarted and BinaryFinished? May want a slightly different way to
    // do things, maybe a couple of reporter traits (one for the run as a whole and one for each
    // binary).
    /// A test started running.
    TestStarted {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that was started.
        test_instance: TestInstanceId<'a>,

        /// Current run statistics so far.
        current_stats: RunStats,

        /// The number of tests currently running, including this one.
        running: usize,

        /// The command line that will be used to run this test.
        command_line: Vec<String>,
    },

    /// A test was slower than a configured soft timeout.
    TestSlow {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that was slow.
        test_instance: TestInstanceId<'a>,

        /// Retry data.
        retry_data: RetryData,

        /// The amount of time that has elapsed since the beginning of the test.
        elapsed: Duration,

        /// True if the test has hit its timeout and is about to be terminated.
        will_terminate: bool,
    },

    /// A test attempt failed and will be retried in the future.
    ///
    /// This event does not occur on the final run of a failing test.
    TestAttemptFailedWillRetry {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that is being retried.
        test_instance: TestInstanceId<'a>,

        /// The status of this attempt to run the test. Will never be success.
        run_status: ExecuteStatus<ChildSingleOutput>,

        /// The delay before the next attempt to run the test.
        delay_before_next_attempt: Duration,

        /// Whether failure outputs are printed out.
        failure_output: TestOutputDisplay,

        /// The current number of running tests.
        running: usize,
    },

    /// A retry has started.
    TestRetryStarted {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that is being retried.
        test_instance: TestInstanceId<'a>,

        /// Data related to retries.
        retry_data: RetryData,

        /// The current number of running tests.
        running: usize,

        /// The command line that will be used to run this test.
        command_line: Vec<String>,
    },

    /// A test finished running.
    TestFinished {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that finished running.
        test_instance: TestInstanceId<'a>,

        /// Test setting for success output.
        success_output: TestOutputDisplay,

        /// Test setting for failure output.
        failure_output: TestOutputDisplay,

        /// Whether the JUnit report should store success output for this test.
        junit_store_success_output: bool,

        /// Whether the JUnit report should store failure output for this test.
        junit_store_failure_output: bool,

        /// Information about all the runs for this test.
        run_statuses: ExecutionStatuses<ChildSingleOutput>,

        /// Current statistics for number of tests so far.
        current_stats: RunStats,

        /// The number of tests that are currently running, excluding this one.
        running: usize,
    },

    /// A test was skipped.
    TestSkipped {
        /// If a stress test is being run, the stress index, starting from 0.
        stress_index: Option<StressIndex>,

        /// The test instance that was skipped.
        test_instance: TestInstanceId<'a>,

        /// The reason this test was skipped.
        reason: MismatchReason,
    },

    /// An information request was received.
    InfoStarted {
        /// The number of tasks currently running. This is the same as the
        /// number of expected responses.
        total: usize,

        /// Statistics for the run.
        run_stats: RunStats,
    },

    /// Information about a script or test was received.
    InfoResponse {
        /// The index of the response, starting from 0.
        index: usize,

        /// The total number of responses expected.
        total: usize,

        /// The response itself.
        response: InfoResponse<'a>,
    },

    /// An information request was completed.
    InfoFinished {
        /// The number of responses that were not received. In most cases, this
        /// is 0.
        missing: usize,
    },

    /// `Enter` was pressed. Either a newline or a progress bar snapshot needs
    /// to be printed.
    InputEnter {
        /// Current statistics for number of tests so far.
        current_stats: RunStats,

        /// The number of tests running.
        running: usize,
    },

    /// A cancellation notice was received.
    RunBeginCancel {
        /// The number of setup scripts still running.
        setup_scripts_running: usize,

        /// Current statistics for number of tests so far.
        ///
        /// `current_stats.cancel_reason` is set to `Some`.
        current_stats: RunStats,

        /// The number of tests still running.
        running: usize,
    },

    /// A forcible kill was requested due to receiving a signal.
    RunBeginKill {
        /// The number of setup scripts still running.
        setup_scripts_running: usize,

        /// Current statistics for number of tests so far.
        ///
        /// `current_stats.cancel_reason` is set to `Some`.
        current_stats: RunStats,

        /// The number of tests still running.
        running: usize,
    },

    /// A SIGTSTP event was received and the run was paused.
    RunPaused {
        /// The number of setup scripts running.
        setup_scripts_running: usize,

        /// The number of tests currently running.
        running: usize,
    },

    /// A SIGCONT event was received and the run is being continued.
    RunContinued {
        /// The number of setup scripts that will be started up again.
        setup_scripts_running: usize,

        /// The number of tests that will be started up again.
        running: usize,
    },

    /// When running stress tests serially, a sub-run finished.
    StressSubRunFinished {
        /// The amount of progress completed so far.
        progress: StressProgress,

        /// The amount of time it took for this sub-run to complete.
        sub_elapsed: Duration,

        /// Statistics for the sub-run.
        sub_stats: RunStats,
    },

    /// The test run finished.
    RunFinished {
        /// The unique ID for this run.
        run_id: ReportUuid,

        /// The time at which the run was started.
        start_time: DateTime<FixedOffset>,

        /// The amount of time it took for the tests to run.
        elapsed: Duration,

        /// Statistics for the run, or overall statistics for stress tests.
        run_stats: RunFinishedStats,

        /// Tests that were expected to run but were not seen during this run.
        ///
        /// This is only set for reruns when some tests from the outstanding set
        /// did not produce any events.
        outstanding_not_seen: Option<TestsNotSeen>,
    },
}

/// Tests that were expected to run but were not seen during a rerun.
#[derive(Clone, Debug)]
pub struct TestsNotSeen {
    /// A sample of test instance IDs that were not seen, up to a reasonable
    /// limit.
    ///
    /// This uses [`OwnedTestInstanceId`] rather than [`TestInstanceId`]
    /// because the tests may not be present in the current test list (they
    /// come from the expected outstanding set from a prior run).
    pub not_seen: Vec<OwnedTestInstanceId>,

    /// The total number of tests not seen (may exceed `not_seen.len()`).
    pub total_not_seen: usize,
}

/// Progress for a stress test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "progress-type", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum StressProgress {
    /// This is a count-based stress run.
    Count {
        /// The total number of stress runs.
        total: StressCount,

        /// The total time that has elapsed across all stress runs so far.
        elapsed: Duration,

        /// The number of stress runs that have been completed.
        completed: u32,
    },

    /// This is a time-based stress run.
    Time {
        /// The total time for the stress run.
        total: Duration,

        /// The total time that has elapsed across all stress runs so far.
        elapsed: Duration,

        /// The number of stress runs that have been completed.
        completed: u32,
    },
}

impl StressProgress {
    /// Returns the remaining amount of work if the progress indicates there's
    /// still more to do, otherwise `None`.
    pub fn remaining(&self) -> Option<StressRemaining> {
        match self {
            Self::Count {
                total: StressCount::Count { count },
                elapsed: _,
                completed,
            } => count
                .get()
                .checked_sub(*completed)
                .and_then(|remaining| NonZero::try_from(remaining).ok())
                .map(StressRemaining::Count),
            Self::Count {
                total: StressCount::Infinite,
                ..
            } => Some(StressRemaining::Infinite),
            Self::Time {
                total,
                elapsed,
                completed: _,
            } => total.checked_sub(*elapsed).map(StressRemaining::Time),
        }
    }

    /// Returns a unique ID for this stress sub-run, consisting of the run ID and stress index.
    pub fn unique_id(&self, run_id: ReportUuid) -> String {
        let stress_current = match self {
            Self::Count { completed, .. } | Self::Time { completed, .. } => *completed,
        };
        format!("{}:@stress-{}", run_id, stress_current)
    }
}

/// For a stress test, the amount of time or number of stress runs remaining.
#[derive(Clone, Debug)]
pub enum StressRemaining {
    /// The number of stress runs remaining, guaranteed to be non-zero.
    Count(NonZero<u32>),

    /// Infinite number of stress runs remaining.
    Infinite,

    /// The amount of time remaining.
    Time(Duration),
}

/// The index of the current stress run.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct StressIndex {
    /// The 0-indexed index.
    pub current: u32,

    /// The total number of stress runs, if that is available.
    pub total: Option<NonZero<u32>>,
}

/// Statistics for a completed test run or stress run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum RunFinishedStats {
    /// A single test run was completed.
    Single(RunStats),

    /// A stress run was completed.
    Stress(StressRunStats),
}

impl RunFinishedStats {
    /// For a single run, returns a summary of statistics as an enum. For a
    /// stress run, returns a summary for the last sub-run.
    pub fn final_stats(&self) -> FinalRunStats {
        match self {
            Self::Single(stats) => stats.summarize_final(),
            Self::Stress(stats) => stats.last_final_stats,
        }
    }
}

/// Statistics for a test run.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct RunStats {
    /// The total number of tests that were expected to be run at the beginning.
    ///
    /// If the test run is cancelled, this will be more than `finished_count` at the end.
    pub initial_run_count: usize,

    /// The total number of tests that finished running.
    pub finished_count: usize,

    /// The total number of setup scripts that were expected to be run at the beginning.
    ///
    /// If the test run is cancelled, this will be more than `finished_count` at the end.
    pub setup_scripts_initial_count: usize,

    /// The total number of setup scripts that finished running.
    pub setup_scripts_finished_count: usize,

    /// The number of setup scripts that passed.
    pub setup_scripts_passed: usize,

    /// The number of setup scripts that failed.
    pub setup_scripts_failed: usize,

    /// The number of setup scripts that encountered an execution failure.
    pub setup_scripts_exec_failed: usize,

    /// The number of setup scripts that timed out.
    pub setup_scripts_timed_out: usize,

    /// The number of tests that passed. Includes `passed_slow`, `passed_timed_out`, `flaky` and `leaky`.
    pub passed: usize,

    /// The number of slow tests that passed.
    pub passed_slow: usize,

    /// The number of timed out tests that passed.
    pub passed_timed_out: usize,

    /// The number of tests that passed on retry.
    pub flaky: usize,

    /// The number of tests that failed. Includes `leaky_failed`.
    pub failed: usize,

    /// The number of failed tests that were slow.
    pub failed_slow: usize,

    /// The number of timed out tests that failed.
    pub failed_timed_out: usize,

    /// The number of tests that passed but leaked handles.
    pub leaky: usize,

    /// The number of tests that otherwise passed, but leaked handles and were
    /// treated as failed as a result.
    pub leaky_failed: usize,

    /// The number of tests that encountered an execution failure.
    pub exec_failed: usize,

    /// The number of tests that were skipped.
    pub skipped: usize,

    /// If the run is cancelled, the reason the cancellation is happening.
    pub cancel_reason: Option<CancelReason>,
}

impl RunStats {
    /// Returns true if there are any failures recorded in the stats.
    pub fn has_failures(&self) -> bool {
        self.failed_setup_script_count() > 0 || self.failed_count() > 0
    }

    /// Returns count of setup scripts that did not pass.
    pub fn failed_setup_script_count(&self) -> usize {
        self.setup_scripts_failed + self.setup_scripts_exec_failed + self.setup_scripts_timed_out
    }

    /// Returns count of tests that did not pass.
    pub fn failed_count(&self) -> usize {
        self.failed + self.exec_failed + self.failed_timed_out
    }

    /// Summarizes the stats as an enum at the end of a test run.
    pub fn summarize_final(&self) -> FinalRunStats {
        // Check for failures first. The order of setup scripts vs tests should
        // not be important, though we don't assert that here.
        if self.failed_setup_script_count() > 0 {
            // Is this related to a cancellation other than one directly caused
            // by the failure?
            if self.cancel_reason > Some(CancelReason::TestFailure) {
                FinalRunStats::Cancelled {
                    reason: self.cancel_reason,
                    kind: RunStatsFailureKind::SetupScript,
                }
            } else {
                FinalRunStats::Failed {
                    kind: RunStatsFailureKind::SetupScript,
                }
            }
        } else if self.setup_scripts_initial_count > self.setup_scripts_finished_count {
            FinalRunStats::Cancelled {
                reason: self.cancel_reason,
                kind: RunStatsFailureKind::SetupScript,
            }
        } else if self.failed_count() > 0 {
            let kind = RunStatsFailureKind::Test {
                initial_run_count: self.initial_run_count,
                not_run: self.initial_run_count.saturating_sub(self.finished_count),
            };

            // Is this related to a cancellation other than one directly caused
            // by the failure?
            if self.cancel_reason > Some(CancelReason::TestFailure) {
                FinalRunStats::Cancelled {
                    reason: self.cancel_reason,
                    kind,
                }
            } else {
                FinalRunStats::Failed { kind }
            }
        } else if self.initial_run_count > self.finished_count {
            FinalRunStats::Cancelled {
                reason: self.cancel_reason,
                kind: RunStatsFailureKind::Test {
                    initial_run_count: self.initial_run_count,
                    not_run: self.initial_run_count.saturating_sub(self.finished_count),
                },
            }
        } else if self.finished_count == 0 {
            FinalRunStats::NoTestsRun
        } else {
            FinalRunStats::Success
        }
    }

    pub(crate) fn on_setup_script_finished(
        &mut self,
        status: &SetupScriptExecuteStatus<ChildSingleOutput>,
    ) {
        self.setup_scripts_finished_count += 1;

        match status.result {
            ExecutionResultDescription::Pass
            | ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Pass,
            } => {
                self.setup_scripts_passed += 1;
            }
            ExecutionResultDescription::Fail { .. }
            | ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Fail,
            } => {
                self.setup_scripts_failed += 1;
            }
            ExecutionResultDescription::ExecFail => {
                self.setup_scripts_exec_failed += 1;
            }
            // Timed out setup scripts are always treated as failures.
            ExecutionResultDescription::Timeout { .. } => {
                self.setup_scripts_timed_out += 1;
            }
        }
    }

    pub(crate) fn on_test_finished(&mut self, run_statuses: &ExecutionStatuses<ChildSingleOutput>) {
        self.finished_count += 1;
        // run_statuses is guaranteed to have at least one element.
        // * If the last element is success, treat it as success (and possibly flaky).
        // * If the last element is a failure, use it to determine fail/exec fail.
        // Note that this is different from what Maven Surefire does (use the first failure):
        // https://maven.apache.org/surefire/maven-surefire-plugin/examples/rerun-failing-tests.html
        //
        // This is not likely to matter much in practice since failures are likely to be of the
        // same type.
        let last_status = run_statuses.last_status();
        match last_status.result {
            ExecutionResultDescription::Pass => {
                self.passed += 1;
                if last_status.is_slow {
                    self.passed_slow += 1;
                }
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Pass,
            } => {
                self.passed += 1;
                self.leaky += 1;
                if last_status.is_slow {
                    self.passed_slow += 1;
                }
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Fail,
            } => {
                self.failed += 1;
                self.leaky_failed += 1;
                if last_status.is_slow {
                    self.failed_slow += 1;
                }
            }
            ExecutionResultDescription::Fail { .. } => {
                self.failed += 1;
                if last_status.is_slow {
                    self.failed_slow += 1;
                }
            }
            ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Pass,
            } => {
                self.passed += 1;
                self.passed_timed_out += 1;
                if run_statuses.len() > 1 {
                    self.flaky += 1;
                }
            }
            ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Fail,
            } => {
                self.failed_timed_out += 1;
            }
            ExecutionResultDescription::ExecFail => self.exec_failed += 1,
        }
    }
}

/// A type summarizing the possible outcomes of a test run.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum FinalRunStats {
    /// The test run was successful, or is successful so far.
    Success,

    /// The test run was successful, or is successful so far, but no tests were selected to run.
    NoTestsRun,

    /// The test run was cancelled.
    Cancelled {
        /// The reason for cancellation, if available.
        ///
        /// This should generally be available, but may be None if some tests
        /// that were selected to run were not executed.
        reason: Option<CancelReason>,

        /// The kind of failure that occurred.
        kind: RunStatsFailureKind,
    },

    /// At least one test failed.
    Failed {
        /// The kind of failure that occurred.
        kind: RunStatsFailureKind,
    },
}

/// Statistics for a stress run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct StressRunStats {
    /// The number of stress runs completed.
    pub completed: StressIndex,

    /// The number of stress runs that succeeded.
    pub success_count: u32,

    /// The number of stress runs that failed.
    pub failed_count: u32,

    /// The last stress run's `FinalRunStats`.
    pub last_final_stats: FinalRunStats,
}

impl StressRunStats {
    /// Summarizes the stats as an enum at the end of a test run.
    pub fn summarize_final(&self) -> StressFinalRunStats {
        if self.failed_count > 0 {
            StressFinalRunStats::Failed
        } else if matches!(self.last_final_stats, FinalRunStats::Cancelled { .. }) {
            StressFinalRunStats::Cancelled
        } else if matches!(self.last_final_stats, FinalRunStats::NoTestsRun) {
            StressFinalRunStats::NoTestsRun
        } else {
            StressFinalRunStats::Success
        }
    }
}

/// A summary of final statistics for a stress run.
pub enum StressFinalRunStats {
    /// The stress run was successful.
    Success,

    /// No tests were run.
    NoTestsRun,

    /// The stress run was cancelled.
    Cancelled,

    /// At least one stress run failed.
    Failed,
}

/// A type summarizing the step at which a test run failed.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum RunStatsFailureKind {
    /// The run was interrupted during setup script execution.
    SetupScript,

    /// The run was interrupted during test execution.
    Test {
        /// The total number of tests scheduled.
        initial_run_count: usize,

        /// The number of tests not run, or for a currently-executing test the number queued up to
        /// run.
        not_run: usize,
    },
}

/// Information about executions of a test, including retries.
///
/// The type parameter `O` represents how test output is stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static))
)]
pub struct ExecutionStatuses<O> {
    /// This is guaranteed to be non-empty.
    #[cfg_attr(test, strategy(proptest::collection::vec(proptest::arbitrary::any::<ExecuteStatus<O>>(), 1..=3)))]
    statuses: Vec<ExecuteStatus<O>>,
}

impl<'de, O: Deserialize<'de>> Deserialize<'de> for ExecutionStatuses<O> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize as the wrapper struct that matches the Serialize output.
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct Helper<O> {
            statuses: Vec<ExecuteStatus<O>>,
        }

        let helper = Helper::<O>::deserialize(deserializer)?;
        if helper.statuses.is_empty() {
            return Err(serde::de::Error::custom("expected non-empty statuses"));
        }
        Ok(Self {
            statuses: helper.statuses,
        })
    }
}

#[expect(clippy::len_without_is_empty)] // RunStatuses is never empty
impl<O> ExecutionStatuses<O> {
    pub(crate) fn new(statuses: Vec<ExecuteStatus<O>>) -> Self {
        debug_assert!(!statuses.is_empty(), "ExecutionStatuses must be non-empty");
        Self { statuses }
    }

    /// Returns the last execution status.
    ///
    /// This status is typically used as the final result.
    pub fn last_status(&self) -> &ExecuteStatus<O> {
        self.statuses
            .last()
            .expect("execution statuses is non-empty")
    }

    /// Iterates over all the statuses.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &'_ ExecuteStatus<O>> + '_ {
        self.statuses.iter()
    }

    /// Returns the number of times the test was executed.
    pub fn len(&self) -> usize {
        self.statuses.len()
    }

    /// Returns a description of self.
    pub fn describe(&self) -> ExecutionDescription<'_, O> {
        let last_status = self.last_status();
        if last_status.result.is_success() {
            if self.statuses.len() > 1 {
                ExecutionDescription::Flaky {
                    last_status,
                    prior_statuses: &self.statuses[..self.statuses.len() - 1],
                }
            } else {
                ExecutionDescription::Success {
                    single_status: last_status,
                }
            }
        } else {
            let first_status = self
                .statuses
                .first()
                .expect("execution statuses is non-empty");
            let retries = &self.statuses[1..];
            ExecutionDescription::Failure {
                first_status,
                last_status,
                retries,
            }
        }
    }
}

impl<O> IntoIterator for ExecutionStatuses<O> {
    type Item = ExecuteStatus<O>;
    type IntoIter = std::vec::IntoIter<ExecuteStatus<O>>;

    fn into_iter(self) -> Self::IntoIter {
        self.statuses.into_iter()
    }
}

// TODO: Add Arbitrary impl for ExecutionStatuses when generic type support is added.
// This requires ExecuteStatus<O> to implement Arbitrary with proper bounds.

/// A description of test executions obtained from `ExecuteStatuses`.
///
/// This can be used to quickly determine whether a test passed, failed or was flaky.
///
/// The type parameter `O` represents how test output is stored.
#[derive(Debug)]
pub enum ExecutionDescription<'a, O> {
    /// The test was run once and was successful.
    Success {
        /// The status of the test.
        single_status: &'a ExecuteStatus<O>,
    },

    /// The test was run more than once. The final result was successful.
    Flaky {
        /// The last, successful status.
        last_status: &'a ExecuteStatus<O>,

        /// Previous statuses, none of which are successes.
        prior_statuses: &'a [ExecuteStatus<O>],
    },

    /// The test was run once, or possibly multiple times. All runs failed.
    Failure {
        /// The first, failing status.
        first_status: &'a ExecuteStatus<O>,

        /// The last, failing status. Same as the first status if no retries were performed.
        last_status: &'a ExecuteStatus<O>,

        /// Any retries that were performed. All of these runs failed.
        ///
        /// May be empty.
        retries: &'a [ExecuteStatus<O>],
    },
}

// Manual Copy and Clone implementations to avoid requiring O: Copy/Clone, since
// ExecutionDescription just stores references.
impl<O> Clone for ExecutionDescription<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O> Copy for ExecutionDescription<'_, O> {}

impl<'a, O> ExecutionDescription<'a, O> {
    /// Returns the status level for this `ExecutionDescription`.
    pub fn status_level(&self) -> StatusLevel {
        match self {
            ExecutionDescription::Success { single_status } => match single_status.result {
                ExecutionResultDescription::Leak {
                    result: LeakTimeoutResult::Pass,
                } => StatusLevel::Leak,
                ExecutionResultDescription::Pass => StatusLevel::Pass,
                ExecutionResultDescription::Timeout {
                    result: SlowTimeoutResult::Pass,
                } => StatusLevel::Slow,
                ref other => unreachable!(
                    "Success only permits Pass, Leak Pass, or Timeout Pass, found {other:?}"
                ),
            },
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => StatusLevel::Retry,
            ExecutionDescription::Failure { .. } => StatusLevel::Fail,
        }
    }

    /// Returns the final status level for this `ExecutionDescription`.
    pub fn final_status_level(&self) -> FinalStatusLevel {
        match self {
            ExecutionDescription::Success { single_status, .. } => {
                // Slow is higher priority than leaky, so return slow first here.
                if single_status.is_slow {
                    FinalStatusLevel::Slow
                } else {
                    match single_status.result {
                        ExecutionResultDescription::Pass => FinalStatusLevel::Pass,
                        ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Pass,
                        } => FinalStatusLevel::Leak,
                        // Timeout with Pass should return Slow, but this case
                        // shouldn't be reached because is_slow is true for
                        // timeout scenarios. Handle it for completeness.
                        ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Pass,
                        } => FinalStatusLevel::Slow,
                        ref other => unreachable!(
                            "Success only permits Pass, Leak Pass, or Timeout Pass, found {other:?}"
                        ),
                    }
                }
            }
            // A flaky test implies that we print out retry information for it.
            ExecutionDescription::Flaky { .. } => FinalStatusLevel::Flaky,
            ExecutionDescription::Failure { .. } => FinalStatusLevel::Fail,
        }
    }

    /// Returns the last run status.
    pub fn last_status(&self) -> &'a ExecuteStatus<O> {
        match self {
            ExecutionDescription::Success {
                single_status: last_status,
            }
            | ExecutionDescription::Flaky { last_status, .. }
            | ExecutionDescription::Failure { last_status, .. } => last_status,
        }
    }
}

/// Pre-computed error summary for display.
///
/// This contains the formatted error messages, pre-computed from the execution
/// output and result. Useful for record-replay scenarios where the rendering
/// is done on the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct ErrorSummary {
    /// A short summary of the error, suitable for display in a single line.
    pub short_message: String,

    /// A full description of the error chain, suitable for detailed display.
    pub description: String,
}

/// Pre-computed output error slice for display.
///
/// This contains an error message heuristically extracted from test output,
/// such as a panic message or error string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OutputErrorSlice {
    /// The extracted error slice as a string.
    pub slice: String,

    /// The byte offset in the original output where this slice starts.
    pub start: usize,
}

/// Information about a single execution of a test.
///
/// This is the external-facing type used by reporters. The `result` field uses
/// [`ExecutionResultDescription`], a platform-independent type that can be
/// serialized and deserialized across platforms.
///
/// The type parameter `O` represents how test output is stored:
/// - [`ChildSingleOutput`]: Output stored in memory with lazy string conversion.
/// - Other types may be used for serialization to archives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static))
)]
pub struct ExecuteStatus<O> {
    /// Retry-related data.
    pub retry_data: RetryData,
    /// The stdout and stderr output for this test.
    pub output: ChildExecutionOutputDescription<O>,
    /// The execution result for this test: pass, fail or execution error.
    pub result: ExecutionResultDescription,
    /// The time at which the test started.
    #[cfg_attr(
        test,
        strategy(crate::reporter::test_helpers::arb_datetime_fixed_offset())
    )]
    pub start_time: DateTime<FixedOffset>,
    /// The time it took for the test to run.
    #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
    pub time_taken: Duration,
    /// Whether this test counts as slow.
    pub is_slow: bool,
    /// The delay will be non-zero if this is a retry and delay was specified.
    #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
    pub delay_before_start: Duration,
    /// Pre-computed error summary, if available.
    ///
    /// This is computed from the execution output and result, and can be used
    /// for display without needing to re-compute the error chain.
    pub error_summary: Option<ErrorSummary>,
    /// Pre-computed output error slice, if available.
    ///
    /// This is a heuristically extracted error message from the test output,
    /// such as a panic message or error string.
    pub output_error_slice: Option<OutputErrorSlice>,
}

/// Information about the execution of a setup script.
///
/// This is the external-facing type used by reporters. The `result` field uses
/// [`ExecutionResultDescription`], a platform-independent type that can be
/// serialized and deserialized across platforms.
///
/// The type parameter `O` represents how test output is stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static))
)]
pub struct SetupScriptExecuteStatus<O> {
    /// Output for this setup script.
    pub output: ChildExecutionOutputDescription<O>,

    /// The execution result for this setup script: pass, fail or execution error.
    pub result: ExecutionResultDescription,

    /// The time at which the script started.
    #[cfg_attr(
        test,
        strategy(crate::reporter::test_helpers::arb_datetime_fixed_offset())
    )]
    pub start_time: DateTime<FixedOffset>,

    /// The time it took for the script to run.
    #[cfg_attr(test, strategy(crate::reporter::test_helpers::arb_duration()))]
    pub time_taken: Duration,

    /// Whether this script counts as slow.
    pub is_slow: bool,

    /// The map of environment variables that were set by this script.
    ///
    /// `None` if an error occurred while running the script or reading the
    /// environment map.
    pub env_map: Option<SetupScriptEnvMap>,

    /// Pre-computed error summary, if available.
    ///
    /// This is computed from the execution output and result, and can be used
    /// for display without needing to re-compute the error chain.
    pub error_summary: Option<ErrorSummary>,
}

/// A map of environment variables set by a setup script.
///
/// Part of [`SetupScriptExecuteStatus`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct SetupScriptEnvMap {
    /// The map of environment variables set by the script.
    pub env_map: BTreeMap<String, String>,
}

// ---
// Child execution output description types
// ---

/// The result of executing a child process, generic over output storage.
///
/// This is the external-facing counterpart to [`ChildExecutionOutput`]. The
/// type parameter `O` represents how output is stored:
///
/// - [`ChildSingleOutput`]: Output stored in memory with lazy string caching.
///   Used by reporter event types during live runs.
/// - [`ZipStoreOutput`](crate::record::ZipStoreOutput): Reference to a file in
///   a zip archive. Used for record/replay serialization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static))
)]
pub enum ChildExecutionOutputDescription<O> {
    /// The process was run and the output was captured.
    Output {
        /// If the process has finished executing, the final state it is in.
        ///
        /// `None` means execution is currently in progress.
        result: Option<ExecutionResultDescription>,

        /// The captured output.
        output: ChildOutputDescription<O>,

        /// Errors that occurred while waiting on the child process or parsing
        /// its output.
        errors: Option<ErrorList<ChildErrorDescription>>,
    },

    /// There was a failure to start the process.
    StartError(ChildStartErrorDescription),
}

impl<O> ChildExecutionOutputDescription<O> {
    /// Returns true if there are any errors in this output.
    pub fn has_errors(&self) -> bool {
        match self {
            Self::Output { errors, result, .. } => {
                if errors.is_some() {
                    return true;
                }
                if let Some(result) = result {
                    return !result.is_success();
                }
                false
            }
            Self::StartError(_) => true,
        }
    }
}

/// The output of a child process, generic over output storage.
///
/// This represents either split stdout/stderr or combined output. The `Option`
/// wrappers distinguish between "not captured" (`None`) and "captured but
/// empty" (`Some` with empty content).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(
    test,
    derive(test_strategy::Arbitrary),
    arbitrary(bound(O: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static))
)]
pub enum ChildOutputDescription<O> {
    /// The output was split into stdout and stderr.
    Split {
        /// Standard output, or `None` if not captured.
        stdout: Option<O>,
        /// Standard error, or `None` if not captured.
        stderr: Option<O>,
    },

    /// The output was combined into a single stream.
    Combined {
        /// The combined output.
        output: O,
    },
}

impl ChildOutputDescription<ChildSingleOutput> {
    /// Returns the lengths of stdout and stderr in bytes.
    ///
    /// Returns `None` for each stream that wasn't captured.
    pub fn stdout_stderr_len(&self) -> (Option<u64>, Option<u64>) {
        match self {
            Self::Split { stdout, stderr } => (
                stdout.as_ref().map(|s| s.buf.len() as u64),
                stderr.as_ref().map(|s| s.buf.len() as u64),
            ),
            Self::Combined { output } => (Some(output.buf.len() as u64), None),
        }
    }
}

/// A serializable description of an error that occurred while starting a child process.
///
/// This is the external-facing counterpart to [`ChildStartError`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum ChildStartErrorDescription {
    /// An error occurred while creating a temporary path for a setup script.
    TempPath {
        /// The source error.
        source: IoErrorDescription,
    },

    /// An error occurred while spawning the child process.
    Spawn {
        /// The source error.
        source: IoErrorDescription,
    },
}

impl fmt::Display for ChildStartErrorDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TempPath { .. } => {
                write!(f, "error creating temporary path for setup script")
            }
            Self::Spawn { .. } => write!(f, "error spawning child process"),
        }
    }
}

impl std::error::Error for ChildStartErrorDescription {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TempPath { source } | Self::Spawn { source } => Some(source),
        }
    }
}

/// A serializable description of an error that occurred while managing a child process.
///
/// This is the external-facing counterpart to [`ChildError`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum ChildErrorDescription {
    /// An error occurred while reading standard output.
    ReadStdout {
        /// The source error.
        source: IoErrorDescription,
    },

    /// An error occurred while reading standard error.
    ReadStderr {
        /// The source error.
        source: IoErrorDescription,
    },

    /// An error occurred while reading combined output.
    ReadCombined {
        /// The source error.
        source: IoErrorDescription,
    },

    /// An error occurred while waiting for the child process to exit.
    Wait {
        /// The source error.
        source: IoErrorDescription,
    },

    /// An error occurred while reading the output of a setup script.
    SetupScriptOutput {
        /// The source error.
        source: IoErrorDescription,
    },
}

impl fmt::Display for ChildErrorDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdout { .. } => write!(f, "error reading standard output"),
            Self::ReadStderr { .. } => write!(f, "error reading standard error"),
            Self::ReadCombined { .. } => {
                write!(f, "error reading combined stream")
            }
            Self::Wait { .. } => {
                write!(f, "error waiting for child process to exit")
            }
            Self::SetupScriptOutput { .. } => {
                write!(f, "error reading setup script output")
            }
        }
    }
}

impl std::error::Error for ChildErrorDescription {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdout { source }
            | Self::ReadStderr { source }
            | Self::ReadCombined { source }
            | Self::Wait { source }
            | Self::SetupScriptOutput { source } => Some(source),
        }
    }
}

/// A serializable description of an I/O error.
///
/// This captures the error message from an [`std::io::Error`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct IoErrorDescription {
    message: String,
}

impl fmt::Display for IoErrorDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for IoErrorDescription {}

impl From<ChildExecutionOutput> for ChildExecutionOutputDescription<ChildSingleOutput> {
    fn from(output: ChildExecutionOutput) -> Self {
        match output {
            ChildExecutionOutput::Output {
                result,
                output,
                errors,
            } => Self::Output {
                result: result.map(ExecutionResultDescription::from),
                output: ChildOutputDescription::from(output),
                errors: errors.map(|e| e.map(ChildErrorDescription::from)),
            },
            ChildExecutionOutput::StartError(error) => {
                Self::StartError(ChildStartErrorDescription::from(error))
            }
        }
    }
}

impl From<ChildOutput> for ChildOutputDescription<ChildSingleOutput> {
    fn from(output: ChildOutput) -> Self {
        match output {
            ChildOutput::Split(split) => Self::Split {
                stdout: split.stdout,
                stderr: split.stderr,
            },
            ChildOutput::Combined { output } => Self::Combined { output },
        }
    }
}

impl From<ChildStartError> for ChildStartErrorDescription {
    fn from(error: ChildStartError) -> Self {
        match error {
            ChildStartError::TempPath(e) => Self::TempPath {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
            ChildStartError::Spawn(e) => Self::Spawn {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
        }
    }
}

impl From<ChildError> for ChildErrorDescription {
    fn from(error: ChildError) -> Self {
        match error {
            ChildError::Fd(ChildFdError::ReadStdout(e)) => Self::ReadStdout {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
            ChildError::Fd(ChildFdError::ReadStderr(e)) => Self::ReadStderr {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
            ChildError::Fd(ChildFdError::ReadCombined(e)) => Self::ReadCombined {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
            ChildError::Fd(ChildFdError::Wait(e)) => Self::Wait {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
            ChildError::SetupScriptOutput(e) => Self::SetupScriptOutput {
                source: IoErrorDescription {
                    message: e.to_string(),
                },
            },
        }
    }
}

/// Data related to retries for a test.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct RetryData {
    /// The current attempt. In the range `[1, total_attempts]`.
    pub attempt: u32,

    /// The total number of times this test can be run. Equal to `1 + retries`.
    pub total_attempts: u32,
}

impl RetryData {
    /// Returns true if there are no more attempts after this.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.total_attempts
    }
}

/// Whether a test passed, failed or an error occurred while executing the test.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ExecutionResult {
    /// The test passed.
    Pass,
    /// The test passed but leaked handles. This usually indicates that
    /// a subprocess that inherit standard IO was created, but it didn't shut down when
    /// the test failed.
    Leak {
        /// Whether this leak was treated as a failure.
        ///
        /// Note the difference between `Fail { leaked: true }` and `Leak {
        /// failed: true }`. In the former case, the test failed and also leaked
        /// handles. In the latter case, the test passed but leaked handles, and
        /// configuration indicated that this is a failure.
        result: LeakTimeoutResult,
    },
    /// The test failed.
    Fail {
        /// The abort status of the test, if any (for example, the signal on Unix).
        failure_status: FailureStatus,

        /// Whether a test leaked handles. If set to true, this usually indicates that
        /// a subprocess that inherit standard IO was created, but it didn't shut down when
        /// the test failed.
        leaked: bool,
    },
    /// An error occurred while executing the test.
    ExecFail,
    /// The test was terminated due to a timeout.
    Timeout {
        /// Whether this timeout was treated as a failure.
        result: SlowTimeoutResult,
    },
}

impl ExecutionResult {
    /// Returns true if the test was successful.
    pub fn is_success(self) -> bool {
        match self {
            ExecutionResult::Pass
            | ExecutionResult::Timeout {
                result: SlowTimeoutResult::Pass,
            }
            | ExecutionResult::Leak {
                result: LeakTimeoutResult::Pass,
            } => true,
            ExecutionResult::Leak {
                result: LeakTimeoutResult::Fail,
            }
            | ExecutionResult::Fail { .. }
            | ExecutionResult::ExecFail
            | ExecutionResult::Timeout {
                result: SlowTimeoutResult::Fail,
            } => false,
        }
    }

    /// Returns a static string representation of the result.
    pub fn as_static_str(&self) -> &'static str {
        match self {
            ExecutionResult::Pass => "pass",
            ExecutionResult::Leak { .. } => "leak",
            ExecutionResult::Fail { .. } => "fail",
            ExecutionResult::ExecFail => "exec-fail",
            ExecutionResult::Timeout { .. } => "timeout",
        }
    }
}

/// Failure status: either an exit code or an abort status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureStatus {
    /// The test exited with a non-zero exit code.
    ExitCode(i32),

    /// The test aborted.
    Abort(AbortStatus),
}

impl FailureStatus {
    /// Extract the failure status from an `ExitStatus`.
    pub fn extract(exit_status: ExitStatus) -> Self {
        if let Some(abort_status) = AbortStatus::extract(exit_status) {
            FailureStatus::Abort(abort_status)
        } else {
            FailureStatus::ExitCode(
                exit_status
                    .code()
                    .expect("if abort_status is None, then code must be present"),
            )
        }
    }
}

/// A regular exit code or Windows NT abort status for a test.
///
/// Returned as part of the [`ExecutionResult::Fail`] variant.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum AbortStatus {
    /// The test was aborted due to a signal on Unix.
    #[cfg(unix)]
    UnixSignal(i32),

    /// The test was determined to have aborted because the high bit was set on Windows.
    #[cfg(windows)]
    WindowsNtStatus(windows_sys::Win32::Foundation::NTSTATUS),

    /// The test was terminated via job object on Windows.
    #[cfg(windows)]
    JobObject,
}

impl AbortStatus {
    /// Extract the abort status from an [`ExitStatus`].
    pub fn extract(exit_status: ExitStatus) -> Option<Self> {
        cfg_if::cfg_if! {
            if #[cfg(unix)] {
                // On Unix, extract the signal if it's found.
                use std::os::unix::process::ExitStatusExt;
                exit_status.signal().map(AbortStatus::UnixSignal)
            } else if #[cfg(windows)] {
                exit_status.code().and_then(|code| {
                    (code < 0).then_some(AbortStatus::WindowsNtStatus(code))
                })
            } else {
                None
            }
        }
    }
}

impl fmt::Debug for AbortStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            AbortStatus::UnixSignal(signal) => write!(f, "UnixSignal({signal})"),
            #[cfg(windows)]
            AbortStatus::WindowsNtStatus(status) => write!(f, "WindowsNtStatus({status:x})"),
            #[cfg(windows)]
            AbortStatus::JobObject => write!(f, "JobObject"),
        }
    }
}

/// A platform-independent description of an abort status.
///
/// This type can be serialized on one platform and deserialized on another,
/// containing all information needed for display without requiring
/// platform-specific lookups.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[non_exhaustive]
pub enum AbortDescription {
    /// The process was aborted by a Unix signal.
    UnixSignal {
        /// The signal number.
        signal: i32,
        /// The signal name without the "SIG" prefix (e.g., "TERM", "SEGV"),
        /// if known.
        #[cfg_attr(
            test,
            strategy(proptest::option::of(crate::reporter::test_helpers::arb_smol_str()))
        )]
        name: Option<SmolStr>,
    },

    /// The process was aborted with a Windows NT status code.
    WindowsNtStatus {
        /// The NTSTATUS code.
        code: i32,
        /// The human-readable message from the Win32 error code, if available.
        #[cfg_attr(
            test,
            strategy(proptest::option::of(crate::reporter::test_helpers::arb_smol_str()))
        )]
        message: Option<SmolStr>,
    },

    /// The process was terminated via a Windows job object.
    WindowsJobObject,
}

impl fmt::Display for AbortDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnixSignal { signal, name } => {
                write!(f, "aborted with signal {signal}")?;
                if let Some(name) = name {
                    write!(f, " (SIG{name})")?;
                }
                Ok(())
            }
            Self::WindowsNtStatus { code, message } => {
                write!(f, "aborted with code {code:#010x}")?;
                if let Some(message) = message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::WindowsJobObject => {
                write!(f, "terminated via job object")
            }
        }
    }
}

impl From<AbortStatus> for AbortDescription {
    fn from(status: AbortStatus) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(unix)] {
                match status {
                    AbortStatus::UnixSignal(signal) => Self::UnixSignal {
                        signal,
                        name: crate::helpers::signal_str(signal).map(SmolStr::new_static),
                    },
                }
            } else if #[cfg(windows)] {
                match status {
                    AbortStatus::WindowsNtStatus(code) => Self::WindowsNtStatus {
                        code,
                        message: crate::helpers::windows_nt_status_message(code),
                    },
                    AbortStatus::JobObject => Self::WindowsJobObject,
                }
            } else {
                match status {}
            }
        }
    }
}

/// A platform-independent description of a test failure status.
///
/// This is the platform-independent counterpart to [`FailureStatus`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[non_exhaustive]
pub enum FailureDescription {
    /// The test exited with a non-zero exit code.
    ExitCode {
        /// The exit code.
        code: i32,
    },

    /// The test was aborted (e.g., by a signal on Unix or NT status on Windows).
    ///
    /// Note: this is a struct variant rather than a newtype variant to ensure
    /// proper JSON nesting. Both `FailureDescription` and `AbortDescription`
    /// use `#[serde(tag = "kind")]`, and if this were a newtype variant, serde
    /// would flatten the inner type causing duplicate `"kind"` fields.
    Abort {
        /// The abort description.
        abort: AbortDescription,
    },
}

impl From<FailureStatus> for FailureDescription {
    fn from(status: FailureStatus) -> Self {
        match status {
            FailureStatus::ExitCode(code) => Self::ExitCode { code },
            FailureStatus::Abort(abort) => Self::Abort {
                abort: AbortDescription::from(abort),
            },
        }
    }
}

impl fmt::Display for FailureDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExitCode { code } => write!(f, "exited with code {code}"),
            Self::Abort { abort } => write!(f, "{abort}"),
        }
    }
}

/// A platform-independent description of a test execution result.
///
/// This is the platform-independent counterpart to [`ExecutionResult`], used
/// in external-facing types like [`ExecuteStatus`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[non_exhaustive]
pub enum ExecutionResultDescription {
    /// The test passed.
    Pass,

    /// The test passed but leaked handles.
    Leak {
        /// Whether this leak was treated as a failure.
        result: LeakTimeoutResult,
    },

    /// The test failed.
    Fail {
        /// The failure status.
        failure: FailureDescription,

        /// Whether the test leaked handles.
        leaked: bool,
    },

    /// An error occurred while executing the test.
    ExecFail,

    /// The test was terminated due to a timeout.
    Timeout {
        /// Whether this timeout was treated as a failure.
        result: SlowTimeoutResult,
    },
}

impl ExecutionResultDescription {
    /// Returns true if the test was successful.
    pub fn is_success(&self) -> bool {
        match self {
            Self::Pass
            | Self::Timeout {
                result: SlowTimeoutResult::Pass,
            }
            | Self::Leak {
                result: LeakTimeoutResult::Pass,
            } => true,
            Self::Leak {
                result: LeakTimeoutResult::Fail,
            }
            | Self::Fail { .. }
            | Self::ExecFail
            | Self::Timeout {
                result: SlowTimeoutResult::Fail,
            } => false,
        }
    }

    /// Returns a static string representation of the result.
    pub fn as_static_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Leak { .. } => "leak",
            Self::Fail { .. } => "fail",
            Self::ExecFail => "exec-fail",
            Self::Timeout { .. } => "timeout",
        }
    }

    /// Returns true if this result represents a test that was terminated by nextest
    /// (as opposed to failing naturally).
    ///
    /// This is used to suppress output spam when running under
    /// TestFailureImmediate.
    ///
    /// TODO: This is a heuristic that checks if the test was terminated by
    /// SIGTERM (Unix) or job object (Windows). In an edge case, a test could
    /// send SIGTERM to itself, which would incorrectly be detected as a
    /// nextest-initiated termination. A more robust solution would track which
    /// tests were explicitly sent termination signals by nextest.
    pub fn is_termination_failure(&self) -> bool {
        matches!(
            self,
            Self::Fail {
                failure: FailureDescription::Abort {
                    abort: AbortDescription::UnixSignal {
                        signal: SIGTERM,
                        ..
                    },
                },
                ..
            } | Self::Fail {
                failure: FailureDescription::Abort {
                    abort: AbortDescription::WindowsJobObject,
                },
                ..
            }
        )
    }
}

impl From<ExecutionResult> for ExecutionResultDescription {
    fn from(result: ExecutionResult) -> Self {
        match result {
            ExecutionResult::Pass => Self::Pass,
            ExecutionResult::Leak { result } => Self::Leak { result },
            ExecutionResult::Fail {
                failure_status,
                leaked,
            } => Self::Fail {
                failure: FailureDescription::from(failure_status),
                leaked,
            },
            ExecutionResult::ExecFail => Self::ExecFail,
            ExecutionResult::Timeout { result } => Self::Timeout { result },
        }
    }
}

// Note: the order here matters -- it indicates severity of cancellation
/// The reason why a test run is being cancelled.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CancelReason {
    /// A setup script failed.
    SetupScriptFailure,

    /// A test failed and --no-fail-fast wasn't specified.
    TestFailure,

    /// An error occurred while reporting results.
    ReportError,

    /// The global timeout was exceeded.
    GlobalTimeout,

    /// A test failed and fail-fast with immediate termination was specified.
    TestFailureImmediate,

    /// A termination signal (on Unix, SIGTERM or SIGHUP) was received.
    Signal,

    /// An interrupt (on Unix, Ctrl-C) was received.
    Interrupt,

    /// A second signal was received, and the run is being forcibly killed.
    SecondSignal,
}

impl CancelReason {
    pub(crate) fn to_static_str(self) -> &'static str {
        match self {
            CancelReason::SetupScriptFailure => "setup script failure",
            CancelReason::TestFailure => "test failure",
            CancelReason::ReportError => "reporting error",
            CancelReason::GlobalTimeout => "global timeout",
            CancelReason::TestFailureImmediate => "test failure",
            CancelReason::Signal => "signal",
            CancelReason::Interrupt => "interrupt",
            CancelReason::SecondSignal => "second signal",
        }
    }
}
/// The kind of unit of work that nextest is executing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    /// A test.
    Test,

    /// A script (e.g. a setup script).
    Script,
}

impl UnitKind {
    pub(crate) const WAITING_ON_TEST_MESSAGE: &str = "waiting on test process";
    pub(crate) const WAITING_ON_SCRIPT_MESSAGE: &str = "waiting on script process";

    pub(crate) const EXECUTING_TEST_MESSAGE: &str = "executing test";
    pub(crate) const EXECUTING_SCRIPT_MESSAGE: &str = "executing script";

    pub(crate) fn waiting_on_message(&self) -> &'static str {
        match self {
            UnitKind::Test => Self::WAITING_ON_TEST_MESSAGE,
            UnitKind::Script => Self::WAITING_ON_SCRIPT_MESSAGE,
        }
    }

    pub(crate) fn executing_message(&self) -> &'static str {
        match self {
            UnitKind::Test => Self::EXECUTING_TEST_MESSAGE,
            UnitKind::Script => Self::EXECUTING_SCRIPT_MESSAGE,
        }
    }
}

/// A response to an information request.
#[derive(Clone, Debug)]
pub enum InfoResponse<'a> {
    /// A setup script's response.
    SetupScript(SetupScriptInfoResponse),

    /// A test's response.
    Test(TestInfoResponse<'a>),
}

/// A setup script's response to an information request.
#[derive(Clone, Debug)]
pub struct SetupScriptInfoResponse {
    /// The stress index of the setup script.
    pub stress_index: Option<StressIndex>,

    /// The identifier of the setup script instance.
    pub script_id: ScriptId,

    /// The program to run.
    pub program: String,

    /// The list of arguments to the program.
    pub args: Vec<String>,

    /// The state of the setup script.
    pub state: UnitState,

    /// Output obtained from the setup script.
    pub output: ChildExecutionOutputDescription<ChildSingleOutput>,
}

/// A test's response to an information request.
#[derive(Clone, Debug)]
pub struct TestInfoResponse<'a> {
    /// The stress index of the test.
    pub stress_index: Option<StressIndex>,

    /// The test instance that the information is about.
    pub test_instance: TestInstanceId<'a>,

    /// Information about retries.
    pub retry_data: RetryData,

    /// The state of the test.
    pub state: UnitState,

    /// Output obtained from the test.
    pub output: ChildExecutionOutputDescription<ChildSingleOutput>,
}

/// The current state of a test or script process: running, exiting, or
/// terminating.
///
/// Part of information response requests.
#[derive(Clone, Debug)]
pub enum UnitState {
    /// The unit is currently running.
    Running {
        /// The process ID.
        pid: u32,

        /// The amount of time the unit has been running.
        time_taken: Duration,

        /// `Some` if the test is marked as slow, along with the duration after
        /// which it was marked as slow.
        slow_after: Option<Duration>,
    },

    /// The test has finished running, and is currently in the process of
    /// exiting.
    Exiting {
        /// The process ID.
        pid: u32,

        /// The amount of time the unit ran for.
        time_taken: Duration,

        /// `Some` if the unit is marked as slow, along with the duration after
        /// which it was marked as slow.
        slow_after: Option<Duration>,

        /// The tentative execution result before leaked status is determined.
        ///
        /// None means that the exit status could not be read, and should be
        /// treated as a failure.
        tentative_result: Option<ExecutionResult>,

        /// How long has been spent waiting for the process to exit.
        waiting_duration: Duration,

        /// How much longer nextest will wait until the test is marked leaky.
        remaining: Duration,
    },

    /// The child process is being terminated by nextest.
    Terminating(UnitTerminatingState),

    /// The unit has finished running and the process has exited.
    Exited {
        /// The result of executing the unit.
        result: ExecutionResult,

        /// The amount of time the unit ran for.
        time_taken: Duration,

        /// `Some` if the unit is marked as slow, along with the duration after
        /// which it was marked as slow.
        slow_after: Option<Duration>,
    },

    /// A delay is being waited out before the next attempt of the test is
    /// started. (Only relevant for tests.)
    DelayBeforeNextAttempt {
        /// The previous execution result.
        previous_result: ExecutionResult,

        /// Whether the previous attempt was marked as slow.
        previous_slow: bool,

        /// How long has been spent waiting so far.
        waiting_duration: Duration,

        /// How much longer nextest will wait until retrying the test.
        remaining: Duration,
    },
}

impl UnitState {
    /// Returns true if the state has a valid output attached to it.
    pub fn has_valid_output(&self) -> bool {
        match self {
            UnitState::Running { .. }
            | UnitState::Exiting { .. }
            | UnitState::Terminating(_)
            | UnitState::Exited { .. } => true,
            UnitState::DelayBeforeNextAttempt { .. } => false,
        }
    }
}

/// The current terminating state of a test or script process.
///
/// Part of [`UnitState::Terminating`].
#[derive(Clone, Debug)]
pub struct UnitTerminatingState {
    /// The process ID.
    pub pid: u32,

    /// The amount of time the unit ran for.
    pub time_taken: Duration,

    /// The reason for the termination.
    pub reason: UnitTerminateReason,

    /// The method by which the process is being terminated.
    pub method: UnitTerminateMethod,

    /// How long has been spent waiting for the process to exit.
    pub waiting_duration: Duration,

    /// How much longer nextest will wait until a kill command is sent to the process.
    pub remaining: Duration,
}

/// The reason for a script or test being forcibly terminated by nextest.
///
/// Part of information response requests.
#[derive(Clone, Copy, Debug)]
pub enum UnitTerminateReason {
    /// The unit is being terminated due to a test timeout being hit.
    Timeout,

    /// The unit is being terminated due to nextest receiving a signal.
    Signal,

    /// The unit is being terminated due to an interrupt (i.e. Ctrl-C).
    Interrupt,
}

impl fmt::Display for UnitTerminateReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitTerminateReason::Timeout => write!(f, "timeout"),
            UnitTerminateReason::Signal => write!(f, "signal"),
            UnitTerminateReason::Interrupt => write!(f, "interrupt"),
        }
    }
}

/// The way in which a script or test is being forcibly terminated by nextest.
#[derive(Clone, Copy, Debug)]
pub enum UnitTerminateMethod {
    /// The unit is being terminated by sending a signal.
    #[cfg(unix)]
    Signal(UnitTerminateSignal),

    /// The unit is being terminated by terminating the Windows job object.
    #[cfg(windows)]
    JobObject,

    /// The unit is being waited on to exit. A termination signal will be sent
    /// if it doesn't exit within the grace period.
    ///
    /// On Windows, this occurs when nextest receives Ctrl-C. In that case, it
    /// is assumed that tests will also receive Ctrl-C and exit on their own. If
    /// tests do not exit within the grace period configured for them, their
    /// corresponding job objects will be terminated.
    #[cfg(windows)]
    Wait,

    /// A fake method used for testing.
    #[cfg(test)]
    Fake,
}

#[cfg(unix)]
/// The signal that is or was sent to terminate a script or test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitTerminateSignal {
    /// The unit is being terminated by sending a SIGINT.
    Interrupt,

    /// The unit is being terminated by sending a SIGTERM signal.
    Term,

    /// The unit is being terminated by sending a SIGHUP signal.
    Hangup,

    /// The unit is being terminated by sending a SIGQUIT signal.
    Quit,

    /// The unit is being terminated by sending a SIGKILL signal.
    Kill,
}

#[cfg(unix)]
impl fmt::Display for UnitTerminateSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitTerminateSignal::Interrupt => write!(f, "SIGINT"),
            UnitTerminateSignal::Term => write!(f, "SIGTERM"),
            UnitTerminateSignal::Hangup => write!(f, "SIGHUP"),
            UnitTerminateSignal::Quit => write!(f, "SIGQUIT"),
            UnitTerminateSignal::Kill => write!(f, "SIGKILL"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_success() {
        assert_eq!(
            RunStats::default().summarize_final(),
            FinalRunStats::NoTestsRun,
            "empty run => no tests run"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Success,
            "initial run count = final run count => success"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 41,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Cancelled {
                reason: None,
                kind: RunStatsFailureKind::Test {
                    initial_run_count: 42,
                    not_run: 1
                }
            },
            "initial run count > final run count => cancelled"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::Test {
                    initial_run_count: 42,
                    not_run: 0,
                },
            },
            "failed => failure"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                exec_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::Test {
                    initial_run_count: 42,
                    not_run: 0,
                },
            },
            "exec failed => failure"
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed_timed_out: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::Test {
                    initial_run_count: 42,
                    not_run: 0,
                },
            },
            "timed out => failure {:?} {:?}",
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                failed_timed_out: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::Test {
                    initial_run_count: 42,
                    not_run: 0,
                },
            },
        );
        assert_eq!(
            RunStats {
                initial_run_count: 42,
                finished_count: 42,
                skipped: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Success,
            "skipped => not considered a failure"
        );

        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Cancelled {
                reason: None,
                kind: RunStatsFailureKind::SetupScript,
            },
            "setup script failed => failure"
        );

        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::SetupScript,
            },
            "setup script failed => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_exec_failed: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::SetupScript,
            },
            "setup script exec failed => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_timed_out: 1,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::Failed {
                kind: RunStatsFailureKind::SetupScript,
            },
            "setup script timed out => failure"
        );
        assert_eq!(
            RunStats {
                setup_scripts_initial_count: 2,
                setup_scripts_finished_count: 2,
                setup_scripts_passed: 2,
                ..RunStats::default()
            }
            .summarize_final(),
            FinalRunStats::NoTestsRun,
            "setup scripts passed => success, but no tests run"
        );
    }

    #[test]
    fn abort_description_serialization() {
        // Unix signal with name.
        let unix_with_name = AbortDescription::UnixSignal {
            signal: 15,
            name: Some("TERM".into()),
        };
        let json = serde_json::to_string_pretty(&unix_with_name).unwrap();
        insta::assert_snapshot!("abort_unix_signal_with_name", json);
        let roundtrip: AbortDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(unix_with_name, roundtrip);

        // Unix signal without name.
        let unix_no_name = AbortDescription::UnixSignal {
            signal: 42,
            name: None,
        };
        let json = serde_json::to_string_pretty(&unix_no_name).unwrap();
        insta::assert_snapshot!("abort_unix_signal_no_name", json);
        let roundtrip: AbortDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(unix_no_name, roundtrip);

        // Windows NT status (0xC000013A is STATUS_CONTROL_C_EXIT).
        let windows_nt = AbortDescription::WindowsNtStatus {
            code: -1073741510_i32,
            message: Some("The application terminated as a result of a CTRL+C.".into()),
        };
        let json = serde_json::to_string_pretty(&windows_nt).unwrap();
        insta::assert_snapshot!("abort_windows_nt_status", json);
        let roundtrip: AbortDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(windows_nt, roundtrip);

        // Windows NT status without message.
        let windows_nt_no_msg = AbortDescription::WindowsNtStatus {
            code: -1073741819_i32,
            message: None,
        };
        let json = serde_json::to_string_pretty(&windows_nt_no_msg).unwrap();
        insta::assert_snapshot!("abort_windows_nt_status_no_message", json);
        let roundtrip: AbortDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(windows_nt_no_msg, roundtrip);

        // Windows job object.
        let job = AbortDescription::WindowsJobObject;
        let json = serde_json::to_string_pretty(&job).unwrap();
        insta::assert_snapshot!("abort_windows_job_object", json);
        let roundtrip: AbortDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(job, roundtrip);
    }

    #[test]
    fn abort_description_cross_platform_deserialization() {
        // Cross-platform deserialization: these JSON strings could come from any
        // platform. Verify they deserialize correctly regardless of current platform.
        let unix_json = r#"{"kind":"unix-signal","signal":11,"name":"SEGV"}"#;
        let unix_desc: AbortDescription = serde_json::from_str(unix_json).unwrap();
        assert_eq!(
            unix_desc,
            AbortDescription::UnixSignal {
                signal: 11,
                name: Some("SEGV".into()),
            }
        );

        let windows_json = r#"{"kind":"windows-nt-status","code":-1073741510,"message":"CTRL+C"}"#;
        let windows_desc: AbortDescription = serde_json::from_str(windows_json).unwrap();
        assert_eq!(
            windows_desc,
            AbortDescription::WindowsNtStatus {
                code: -1073741510,
                message: Some("CTRL+C".into()),
            }
        );

        let job_json = r#"{"kind":"windows-job-object"}"#;
        let job_desc: AbortDescription = serde_json::from_str(job_json).unwrap();
        assert_eq!(job_desc, AbortDescription::WindowsJobObject);
    }

    #[test]
    fn abort_description_display() {
        // Unix signal with name.
        let unix = AbortDescription::UnixSignal {
            signal: 15,
            name: Some("TERM".into()),
        };
        assert_eq!(unix.to_string(), "aborted with signal 15 (SIGTERM)");

        // Unix signal without a name.
        let unix_no_name = AbortDescription::UnixSignal {
            signal: 42,
            name: None,
        };
        assert_eq!(unix_no_name.to_string(), "aborted with signal 42");

        // Windows NT status with message.
        let windows = AbortDescription::WindowsNtStatus {
            code: -1073741510,
            message: Some("CTRL+C exit".into()),
        };
        assert_eq!(
            windows.to_string(),
            "aborted with code 0xc000013a: CTRL+C exit"
        );

        // Windows NT status without message.
        let windows_no_msg = AbortDescription::WindowsNtStatus {
            code: -1073741510,
            message: None,
        };
        assert_eq!(windows_no_msg.to_string(), "aborted with code 0xc000013a");

        // Windows job object.
        let job = AbortDescription::WindowsJobObject;
        assert_eq!(job.to_string(), "terminated via job object");
    }

    #[cfg(unix)]
    #[test]
    fn abort_description_from_abort_status() {
        // Test conversion from AbortStatus to AbortDescription on Unix.
        let status = AbortStatus::UnixSignal(15);
        let description = AbortDescription::from(status);

        assert_eq!(
            description,
            AbortDescription::UnixSignal {
                signal: 15,
                name: Some("TERM".into()),
            }
        );

        // Unknown signal.
        let unknown_status = AbortStatus::UnixSignal(42);
        let unknown_description = AbortDescription::from(unknown_status);
        assert_eq!(
            unknown_description,
            AbortDescription::UnixSignal {
                signal: 42,
                name: None,
            }
        );
    }

    #[test]
    fn execution_result_description_serialization() {
        // Test all variants of ExecutionResultDescription for serialization roundtrips.

        // Pass.
        let pass = ExecutionResultDescription::Pass;
        let json = serde_json::to_string_pretty(&pass).unwrap();
        insta::assert_snapshot!("pass", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(pass, roundtrip);

        // Leak with pass result.
        let leak_pass = ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        };
        let json = serde_json::to_string_pretty(&leak_pass).unwrap();
        insta::assert_snapshot!("leak_pass", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(leak_pass, roundtrip);

        // Leak with fail result.
        let leak_fail = ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Fail,
        };
        let json = serde_json::to_string_pretty(&leak_fail).unwrap();
        insta::assert_snapshot!("leak_fail", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(leak_fail, roundtrip);

        // Fail with exit code, no leak.
        let fail_exit_code = ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code: 101 },
            leaked: false,
        };
        let json = serde_json::to_string_pretty(&fail_exit_code).unwrap();
        insta::assert_snapshot!("fail_exit_code", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_exit_code, roundtrip);

        // Fail with exit code and leak.
        let fail_exit_code_leaked = ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code: 1 },
            leaked: true,
        };
        let json = serde_json::to_string_pretty(&fail_exit_code_leaked).unwrap();
        insta::assert_snapshot!("fail_exit_code_leaked", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_exit_code_leaked, roundtrip);

        // Fail with Unix signal abort.
        let fail_unix_signal = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::UnixSignal {
                    signal: 11,
                    name: Some("SEGV".into()),
                },
            },
            leaked: false,
        };
        let json = serde_json::to_string_pretty(&fail_unix_signal).unwrap();
        insta::assert_snapshot!("fail_unix_signal", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_unix_signal, roundtrip);

        // Fail with Unix signal abort (no name) and leak.
        let fail_unix_signal_unknown = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::UnixSignal {
                    signal: 42,
                    name: None,
                },
            },
            leaked: true,
        };
        let json = serde_json::to_string_pretty(&fail_unix_signal_unknown).unwrap();
        insta::assert_snapshot!("fail_unix_signal_unknown_leaked", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_unix_signal_unknown, roundtrip);

        // Fail with Windows NT status abort.
        let fail_windows_nt = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::WindowsNtStatus {
                    code: -1073741510,
                    message: Some("The application terminated as a result of a CTRL+C.".into()),
                },
            },
            leaked: false,
        };
        let json = serde_json::to_string_pretty(&fail_windows_nt).unwrap();
        insta::assert_snapshot!("fail_windows_nt_status", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_windows_nt, roundtrip);

        // Fail with Windows NT status abort (no message).
        let fail_windows_nt_no_msg = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::WindowsNtStatus {
                    code: -1073741819,
                    message: None,
                },
            },
            leaked: false,
        };
        let json = serde_json::to_string_pretty(&fail_windows_nt_no_msg).unwrap();
        insta::assert_snapshot!("fail_windows_nt_status_no_message", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_windows_nt_no_msg, roundtrip);

        // Fail with Windows job object abort.
        let fail_job_object = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::WindowsJobObject,
            },
            leaked: false,
        };
        let json = serde_json::to_string_pretty(&fail_job_object).unwrap();
        insta::assert_snapshot!("fail_windows_job_object", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(fail_job_object, roundtrip);

        // ExecFail.
        let exec_fail = ExecutionResultDescription::ExecFail;
        let json = serde_json::to_string_pretty(&exec_fail).unwrap();
        insta::assert_snapshot!("exec_fail", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(exec_fail, roundtrip);

        // Timeout with pass result.
        let timeout_pass = ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Pass,
        };
        let json = serde_json::to_string_pretty(&timeout_pass).unwrap();
        insta::assert_snapshot!("timeout_pass", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(timeout_pass, roundtrip);

        // Timeout with fail result.
        let timeout_fail = ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Fail,
        };
        let json = serde_json::to_string_pretty(&timeout_fail).unwrap();
        insta::assert_snapshot!("timeout_fail", json);
        let roundtrip: ExecutionResultDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(timeout_fail, roundtrip);
    }
}
