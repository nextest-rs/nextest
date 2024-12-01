// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::TestOutputDisplay;
use crate::{
    config::ScriptId,
    list::{TestInstance, TestList},
    runner::{ExecuteStatus, ExecutionStatuses, RetryData, RunStats, SetupScriptExecuteStatus},
};
use chrono::{DateTime, FixedOffset};
use nextest_metadata::MismatchReason;
use quick_junit::ReportUuid;
use std::time::Duration;

/// A test event.
///
/// Events are produced by a [`TestRunner`](crate::runner::TestRunner) and consumed by a
/// [`TestReporter`](crate::reporter::TestReporter).
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
    },

    /// A setup script started.
    SetupScriptStarted {
        /// The setup script index.
        index: usize,

        /// The total number of setup scripts.
        total: usize,

        /// The script ID.
        script_id: ScriptId,

        /// The command to run.
        command: &'a str,

        /// The arguments to the command.
        args: &'a [String],

        /// True if some output from the setup script is being passed through.
        no_capture: bool,
    },

    /// A setup script was slow.
    SetupScriptSlow {
        /// The script ID.
        script_id: ScriptId,

        /// The command to run.
        command: &'a str,

        /// The arguments to the command.
        args: &'a [String],

        /// The amount of time elapsed since the start of execution.
        elapsed: Duration,

        /// True if the script has hit its timeout and is about to be terminated.
        will_terminate: bool,
    },

    /// A setup script completed execution.
    SetupScriptFinished {
        /// The setup script index.
        index: usize,

        /// The total number of setup scripts.
        total: usize,

        /// The script ID.
        script_id: ScriptId,

        /// The command to run.
        command: &'a str,

        /// The arguments to the command.
        args: &'a [String],

        /// True if some output from the setup script was passed through.
        no_capture: bool,

        /// The execution status of the setup script.
        run_status: SetupScriptExecuteStatus,
    },

    // TODO: add events for BinaryStarted and BinaryFinished? May want a slightly different way to
    // do things, maybe a couple of reporter traits (one for the run as a whole and one for each
    // binary).
    /// A test started running.
    TestStarted {
        /// The test instance that was started.
        test_instance: TestInstance<'a>,

        /// Current run statistics so far.
        current_stats: RunStats,

        /// The number of tests currently running, including this one.
        running: usize,

        /// The cancel status of the run. This is None if the run is still ongoing.
        cancel_state: Option<CancelReason>,
    },

    /// A test was slower than a configured soft timeout.
    TestSlow {
        /// The test instance that was slow.
        test_instance: TestInstance<'a>,

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
        /// The test instance that is being retried.
        test_instance: TestInstance<'a>,

        /// The status of this attempt to run the test. Will never be success.
        run_status: ExecuteStatus,

        /// The delay before the next attempt to run the test.
        delay_before_next_attempt: Duration,

        /// Whether failure outputs are printed out.
        failure_output: TestOutputDisplay,
    },

    /// A retry has started.
    TestRetryStarted {
        /// The test instance that is being retried.
        test_instance: TestInstance<'a>,

        /// Data related to retries.
        retry_data: RetryData,
    },

    /// A test finished running.
    TestFinished {
        /// The test instance that finished running.
        test_instance: TestInstance<'a>,

        /// Test setting for success output.
        success_output: TestOutputDisplay,

        /// Test setting for failure output.
        failure_output: TestOutputDisplay,

        /// Whether the JUnit report should store success output for this test.
        junit_store_success_output: bool,

        /// Whether the JUnit report should store failure output for this test.
        junit_store_failure_output: bool,

        /// Information about all the runs for this test.
        run_statuses: ExecutionStatuses,

        /// Current statistics for number of tests so far.
        current_stats: RunStats,

        /// The number of tests that are currently running, excluding this one.
        running: usize,

        /// The cancel status of the run. This is None if the run is still ongoing.
        cancel_state: Option<CancelReason>,
    },

    /// A test was skipped.
    TestSkipped {
        /// The test instance that was skipped.
        test_instance: TestInstance<'a>,

        /// The reason this test was skipped.
        reason: MismatchReason,
    },

    /// A cancellation notice was received.
    RunBeginCancel {
        /// The number of setup scripts still running.
        setup_scripts_running: usize,

        /// The number of tests still running.
        running: usize,

        /// The reason this run was cancelled.
        reason: CancelReason,
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

    /// The test run finished.
    RunFinished {
        /// The unique ID for this run.
        run_id: ReportUuid,

        /// The time at which the run was started.
        start_time: DateTime<FixedOffset>,

        /// The amount of time it took for the tests to run.
        elapsed: Duration,

        /// Statistics for the run.
        run_stats: RunStats,
    },
}

// Note: the order here matters -- it indicates severity of cancellation
/// The reason why a test run is being cancelled.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CancelReason {
    /// A setup script failed.
    SetupScriptFailure,

    /// A test failed and --no-fail-fast wasn't specified.
    TestFailure,

    /// An error occurred while reporting results.
    ReportError,

    /// A termination signal (on Unix, SIGTERM or SIGHUP) was received.
    Signal,

    /// An interrupt (on Unix, Ctrl-C) was received.
    Interrupt,
}

impl CancelReason {
    pub(crate) fn to_static_str(self) -> &'static str {
        match self {
            CancelReason::SetupScriptFailure => "setup script failure",
            CancelReason::TestFailure => "test failure",
            CancelReason::ReportError => "reporting error",
            CancelReason::Signal => "signal",
            CancelReason::Interrupt => "interrupt",
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

    #[expect(dead_code)]
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
