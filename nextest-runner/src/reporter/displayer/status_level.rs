// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Status levels: filters for which test statuses are displayed.
//!
//! Status levels play a role that's similar to log levels in typical loggers.

use super::TestOutputDisplay;
use crate::reporter::events::{CancelReason, ExecutionResultDescription};
use serde::Deserialize;

/// Status level to show in the reporter output.
///
/// Status levels are incremental: each level causes all the statuses listed above it to be output. For example,
/// [`Slow`](Self::Slow) implies [`Retry`](Self::Retry) and [`Fail`](Self::Fail).
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum StatusLevel {
    /// No output.
    None,

    /// Only output test failures.
    Fail,

    /// Output retries and failures.
    Retry,

    /// Output information about slow tests, and all variants above.
    Slow,

    /// Output information about leaky tests, and all variants above.
    Leak,

    /// Output passing tests in addition to all variants above.
    Pass,

    /// Output skipped tests in addition to all variants above.
    Skip,

    /// Currently has the same meaning as [`Skip`](Self::Skip).
    All,
}

/// Status level to show at the end of test runs in the reporter output.
///
/// Status levels are incremental.
///
/// This differs from [`StatusLevel`] in two ways:
/// * It has a "flaky" test indicator that's different from "retry" (though "retry" works as an alias.)
/// * It has a different ordering: skipped tests are prioritized over passing ones.
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FinalStatusLevel {
    /// No output.
    None,

    /// Only output test failures.
    Fail,

    /// Output flaky tests.
    #[serde(alias = "retry")]
    Flaky,

    /// Output information about slow tests, and all variants above.
    Slow,

    /// Output skipped tests in addition to all variants above.
    Skip,

    /// Output leaky tests in addition to all variants above.
    Leak,

    /// Output passing tests in addition to all variants above.
    Pass,

    /// Currently has the same meaning as [`Pass`](Self::Pass).
    All,
}

pub(crate) struct StatusLevels {
    pub(crate) status_level: StatusLevel,
    pub(crate) final_status_level: FinalStatusLevel,
}

impl StatusLevels {
    pub(super) fn compute_output_on_test_finished(
        &self,
        display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
        execution_result: &ExecutionResultDescription,
    ) -> OutputOnTestFinished {
        let write_status_line = self.status_level >= test_status_level;

        let is_immediate = display.is_immediate();
        // We store entries in the final output map if either the final status level is high enough or
        // if `display` says we show the output at the end.
        let is_final = display.is_final() || self.final_status_level >= test_final_status_level;

        // Check if this test was terminated by nextest during immediate termination mode.
        // This is a heuristic: we check if the test failed with SIGTERM (Unix) or JobObject (Windows)
        // during TestFailureImmediate cancellation. This suppresses output spam from tests we killed.
        let terminated_by_nextest = cancel_status == Some(CancelReason::TestFailureImmediate)
            && execution_result.is_termination_failure();

        // This table is tested below. The basic invariant is that we generally follow what
        // is_immediate and is_final suggests, except:
        //
        // - if the run is cancelled due to a non-interrupt signal, we display test output at most
        //   once.
        // - if the run is cancelled due to an interrupt, we hide the output because dumping a bunch
        //   of output at the end is likely to not be helpful (though in the future we may want to
        //   at least dump outputs into files and write their names out, or whenever nextest gains
        //   the ability to replay test runs to be able to display it then.)
        // - if the run is cancelled due to immediate test failure termination, we hide output for
        //   tests that were terminated by nextest (via SIGTERM/job object), but still show output
        //   for tests that failed naturally (e.g. due to assertion failures or other exit codes).
        //
        // is_immediate  is_final      cancel_status     terminated_by_nextest  |  show_immediate  store_final
        //
        //     false      false          <= Signal                *             |      false          false
        //     false       true          <= Signal                *             |      false           true  [1]
        //      true      false          <= Signal                *             |       true          false  [1]
        //      true       true           < Signal                *             |       true           true
        //      true       true             Signal                *             |       true          false  [2]
        //       *          *            Interrupt                *             |      false          false  [3]
        //       *          *       TestFailureImmediate         true           |      false          false  [4]
        //       *          *       TestFailureImmediate        false           |  (use rules above)  [5]
        //
        // [1] In non-interrupt cases, we want to display output if specified once.
        //
        // [2] If there's a signal, we shouldn't display output twice at the end since it's
        //     redundant -- instead, just show the output as part of the immediate display.
        //
        // [3] For interrupts, hide all output to avoid spam.
        //
        // [4] For tests terminated by nextest during immediate mode, hide output to avoid spam.
        //
        // [5] For tests that failed naturally during immediate mode (race condition), show output
        //     normally since these are real failures.
        let show_immediate =
            is_immediate && cancel_status <= Some(CancelReason::Signal) && !terminated_by_nextest;

        let store_final = if cancel_status == Some(CancelReason::Interrupt) || terminated_by_nextest
        {
            // Hide output completely for interrupt and nextest-initiated termination.
            OutputStoreFinal::No
        } else if is_final && cancel_status < Some(CancelReason::Signal)
            || !is_immediate && is_final && cancel_status == Some(CancelReason::Signal)
        {
            OutputStoreFinal::Yes {
                display_output: display.is_final(),
            }
        } else if is_immediate && is_final && cancel_status == Some(CancelReason::Signal) {
            // In this special case, we already display the output once as the test is being
            // cancelled, so don't display it again at the end since that's redundant.
            OutputStoreFinal::Yes {
                display_output: false,
            }
        } else {
            OutputStoreFinal::No
        };

        OutputOnTestFinished {
            write_status_line,
            show_immediate,
            store_final,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OutputOnTestFinished {
    pub(super) write_status_line: bool,
    pub(super) show_immediate: bool,
    pub(super) store_final: OutputStoreFinal,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum OutputStoreFinal {
    /// Do not store the output.
    No,

    /// Store the output. display_output controls whether stdout and stderr should actually be
    /// displayed at the end.
    Yes { display_output: bool },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        output_spec::RecordingSpec,
        record::{LoadOutput, OutputEventKind},
        reporter::{
            displayer::{OutputLoadDecider, unit_output::OutputDisplayOverrides},
            events::ExecutionStatuses,
        },
    };
    use test_strategy::{Arbitrary, proptest};

    // ---
    // The proptests here are probabilistically exhaustive, and it's just easier to express them
    // as property-based tests. We could also potentially use a model checker like Kani here.
    // ---

    #[proptest(cases = 64)]
    fn on_test_finished_dont_write_status_line(
        display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        #[filter(StatusLevel::Pass < #test_status_level)] test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );

        assert!(!actual.write_status_line);
    }

    #[proptest(cases = 64)]
    fn on_test_finished_write_status_line(
        display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        #[filter(StatusLevel::Pass >= #test_status_level)] test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert!(actual.write_status_line);
    }

    #[proptest(cases = 64)]
    fn on_test_finished_with_interrupt(
        // We always hide output on interrupt.
        display: TestOutputDisplay,
        // cancel_status is fixed to Interrupt.

        // In this case, the status levels are not relevant for is_immediate and is_final.
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            Some(CancelReason::Interrupt),
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert!(!actual.show_immediate);
        assert_eq!(actual.store_final, OutputStoreFinal::No);
    }

    #[proptest(cases = 64)]
    fn on_test_finished_dont_show_immediate(
        #[filter(!#display.is_immediate())] display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        // The status levels are not relevant for show_immediate.
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert!(!actual.show_immediate);
    }

    #[proptest(cases = 64)]
    fn on_test_finished_show_immediate(
        #[filter(#display.is_immediate())] display: TestOutputDisplay,
        #[filter(#cancel_status <= Some(CancelReason::Signal))] cancel_status: Option<CancelReason>,
        // The status levels are not relevant for show_immediate.
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert!(actual.show_immediate);
    }

    // Where we don't store final output: if display.is_final() is false, and if the test final
    // status level is too high.
    #[proptest(cases = 64)]
    fn on_test_finished_dont_store_final(
        #[filter(!#display.is_final())] display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        // The status level is not relevant for store_final.
        test_status_level: StatusLevel,
        // But the final status level is.
        #[filter(FinalStatusLevel::Fail < #test_final_status_level)]
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert_eq!(actual.store_final, OutputStoreFinal::No);
    }

    // Case 1 where we store final output: if display is exactly TestOutputDisplay::Final, and if
    // the cancel status is not Interrupt.
    #[proptest(cases = 64)]
    fn on_test_finished_store_final_1(
        #[filter(#cancel_status <= Some(CancelReason::Signal))] cancel_status: Option<CancelReason>,
        // In this case, it isn't relevant what test_status_level and test_final_status_level are.
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            TestOutputDisplay::Final,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: true
            }
        );
    }

    // Case 2 where we store final output: if display is TestOutputDisplay::ImmediateFinal and the
    // cancel status is not Signal or Interrupt
    #[proptest(cases = 64)]
    fn on_test_finished_store_final_2(
        #[filter(#cancel_status < Some(CancelReason::Signal))] cancel_status: Option<CancelReason>,
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            TestOutputDisplay::ImmediateFinal,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: true
            }
        );
    }

    // Case 3 where we store final output: if display is TestOutputDisplay::ImmediateFinal and the
    // cancel status is exactly Signal. In this special case, we don't display the output.
    #[proptest(cases = 64)]
    fn on_test_finished_store_final_3(
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            TestOutputDisplay::ImmediateFinal,
            Some(CancelReason::Signal),
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: false,
            }
        );
    }

    // Case 4: if display.is_final() is *false* but the test_final_status_level is low enough.
    #[proptest(cases = 64)]
    fn on_test_finished_store_final_4(
        #[filter(!#display.is_final())] display: TestOutputDisplay,
        #[filter(#cancel_status <= Some(CancelReason::Signal))] cancel_status: Option<CancelReason>,
        // The status level is not relevant for store_final.
        test_status_level: StatusLevel,
        // But the final status level is.
        #[filter(FinalStatusLevel::Fail >= #test_final_status_level)]
        test_final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        let actual = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &ExecutionResultDescription::Pass,
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: false,
            }
        );
    }

    #[test]
    fn on_test_finished_terminated_by_nextest() {
        use crate::reporter::events::{AbortDescription, FailureDescription, SIGTERM};

        let status_levels = StatusLevels {
            status_level: StatusLevel::Pass,
            final_status_level: FinalStatusLevel::Fail,
        };

        // Test 1: Terminated by nextest (SIGTERM) during TestFailureImmediate - should hide
        {
            let execution_result = ExecutionResultDescription::Fail {
                failure: FailureDescription::Abort {
                    abort: AbortDescription::UnixSignal {
                        signal: SIGTERM,
                        name: Some("TERM".into()),
                    },
                },
                leaked: false,
            };

            let actual = status_levels.compute_output_on_test_finished(
                TestOutputDisplay::ImmediateFinal,
                Some(CancelReason::TestFailureImmediate),
                StatusLevel::Fail,
                FinalStatusLevel::Fail,
                &execution_result,
            );

            assert!(
                !actual.show_immediate,
                "should not show immediate for SIGTERM during TestFailureImmediate"
            );
            assert_eq!(
                actual.store_final,
                OutputStoreFinal::No,
                "should not store final for SIGTERM during TestFailureImmediate"
            );
        }

        // Test 2: Terminated by nextest (JobObject) during TestFailureImmediate - should hide
        {
            let execution_result = ExecutionResultDescription::Fail {
                failure: FailureDescription::Abort {
                    abort: AbortDescription::WindowsJobObject,
                },
                leaked: false,
            };

            let actual = status_levels.compute_output_on_test_finished(
                TestOutputDisplay::ImmediateFinal,
                Some(CancelReason::TestFailureImmediate),
                StatusLevel::Fail,
                FinalStatusLevel::Fail,
                &execution_result,
            );

            assert!(
                !actual.show_immediate,
                "should not show immediate for JobObject during TestFailureImmediate"
            );
            assert_eq!(
                actual.store_final,
                OutputStoreFinal::No,
                "should not store final for JobObject during TestFailureImmediate"
            );
        }

        // Test 3: Natural failure (exit code) during TestFailureImmediate - should show
        let execution_result = ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code: 1 },
            leaked: false,
        };

        let actual = status_levels.compute_output_on_test_finished(
            TestOutputDisplay::ImmediateFinal,
            Some(CancelReason::TestFailureImmediate),
            StatusLevel::Fail,
            FinalStatusLevel::Fail,
            &execution_result,
        );

        assert!(
            actual.show_immediate,
            "should show immediate for natural failure during TestFailureImmediate"
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: true
            },
            "should store final for natural failure"
        );

        // Test 4: SIGTERM but not during TestFailureImmediate (user sent signal) - should show
        {
            let execution_result = ExecutionResultDescription::Fail {
                failure: FailureDescription::Abort {
                    abort: AbortDescription::UnixSignal {
                        signal: SIGTERM,
                        name: Some("TERM".into()),
                    },
                },
                leaked: false,
            };

            let actual = status_levels.compute_output_on_test_finished(
                TestOutputDisplay::ImmediateFinal,
                Some(CancelReason::Signal), // Regular signal, not TestFailureImmediate
                StatusLevel::Fail,
                FinalStatusLevel::Fail,
                &execution_result,
            );

            assert!(
                actual.show_immediate,
                "should show immediate for user-initiated SIGTERM"
            );
            assert_eq!(
                actual.store_final,
                OutputStoreFinal::Yes {
                    display_output: false
                },
                "should store but not display final"
            );
        }
    }

    // --- OutputLoadDecider safety invariant tests ---
    //
    // If OutputLoadDecider returns Skip, we ensure that the reporter's display
    // logic will never show output. (This is a one-directional invariant -- the
    // decider errs towards loading more than strictly necessary.)
    //
    // The invariants established below are:
    //
    // 1. OutputLoadDecider conservatively returns Load whenever output
    //    might be shown.
    // 2. The cancellation_only_hides_output test verifies that
    //    cancellation never causes output to appear that wouldn't appear
    //    without cancellation. This justifies the decider ignoring
    //    cancel_status.
    // 3. The test-finished tests verify that if the decider says Skip,
    //    compute_output_on_test_finished (the displayer's oracle) with
    //    cancel_status=None produces no output.
    //
    // Together, they imply that if we skip loading, then there's no output.

    /// Cancellation can only hide output, never show more than the baseline
    /// (cancel_status = None).
    ///
    /// The `OutputLoadDecider` relies on this property.
    #[proptest(cases = 512)]
    fn cancellation_only_hides_output(
        display: TestOutputDisplay,
        cancel_status: Option<CancelReason>,
        test_status_level: StatusLevel,
        test_final_status_level: FinalStatusLevel,
        execution_result: ExecutionResultDescription,
        status_level: StatusLevel,
        final_status_level: FinalStatusLevel,
    ) {
        let status_levels = StatusLevels {
            status_level,
            final_status_level,
        };

        let baseline = status_levels.compute_output_on_test_finished(
            display,
            None,
            test_status_level,
            test_final_status_level,
            &execution_result,
        );

        let with_cancel = status_levels.compute_output_on_test_finished(
            display,
            cancel_status,
            test_status_level,
            test_final_status_level,
            &execution_result,
        );

        // Cancellation must never show MORE output than the baseline.
        if !baseline.show_immediate {
            assert!(
                !with_cancel.show_immediate,
                "cancel_status={cancel_status:?} caused immediate output that \
                 wouldn't appear without cancellation"
            );
        }

        // For store_final, monotonicity has two dimensions:
        // 1. An entry stored (No -> Yes is an escalation).
        // 2. Output bytes displayed (display_output: false -> true is an
        //    escalation).
        //
        // All 9 combinations are enumerated so that adding a new
        // OutputStoreFinal variant forces an update here.
        match (&baseline.store_final, &with_cancel.store_final) {
            // Cancellation caused storage that wouldn't happen without it.
            (OutputStoreFinal::No, OutputStoreFinal::Yes { display_output }) => {
                panic!(
                    "cancel_status={cancel_status:?} caused final output storage \
                     (display_output={display_output}) that wouldn't happen \
                     without cancellation"
                );
            }
            // Cancellation caused output bytes to be displayed when they
            // wouldn't be without it.
            (
                OutputStoreFinal::Yes {
                    display_output: false,
                },
                OutputStoreFinal::Yes {
                    display_output: true,
                },
            ) => {
                panic!(
                    "cancel_status={cancel_status:?} caused final output display \
                     that wouldn't happen without cancellation"
                );
            }

            // Same or reduced visibility is all right.
            (OutputStoreFinal::No, OutputStoreFinal::No)
            | (
                OutputStoreFinal::Yes {
                    display_output: false,
                },
                OutputStoreFinal::No,
            )
            | (
                OutputStoreFinal::Yes {
                    display_output: false,
                },
                OutputStoreFinal::Yes {
                    display_output: false,
                },
            )
            | (
                OutputStoreFinal::Yes {
                    display_output: true,
                },
                _,
            ) => {}
        }
    }

    // --- should_load_for_test_finished with real ExecutionStatuses ---
    //
    // These tests use ExecutionStatuses<RecordingSpec> which naturally
    // covers flaky runs (multi-attempt with last passing), is_slow
    // interactions (is_slow changes final_status_level), and multi-attempt
    // scenarios.

    #[derive(Debug, Arbitrary)]
    struct TestFinishedLoadDeciderInput {
        status_level: StatusLevel,
        final_status_level: FinalStatusLevel,
        success_output: TestOutputDisplay,
        failure_output: TestOutputDisplay,
        force_success_output: Option<TestOutputDisplay>,
        force_failure_output: Option<TestOutputDisplay>,
        force_exec_fail_output: Option<TestOutputDisplay>,
        run_statuses: ExecutionStatuses<RecordingSpec>,
    }

    /// If the decider returns Skip for a TestFinished event, the displayer's
    /// `compute_output_on_test_finished` must never access output bytes. The
    /// cancellation_only_hides test above ensures this extends to all
    /// cancel_status values.
    ///
    /// The invariant is one-directional: Skip implies no output byte access.
    /// The displayer may still store a final entry for the status line, which
    /// is fine if display_output is false.
    ///
    /// This test exercises the full `should_load_for_test_finished` path
    /// with real `ExecutionStatuses`.
    #[proptest(cases = 512)]
    fn load_decider_test_finished_skip_implies_no_output(input: TestFinishedLoadDeciderInput) {
        let TestFinishedLoadDeciderInput {
            status_level,
            final_status_level,
            success_output,
            failure_output,
            force_success_output,
            force_failure_output,
            force_exec_fail_output,
            run_statuses,
        } = input;

        let decider = OutputLoadDecider {
            status_level,
            overrides: OutputDisplayOverrides {
                force_success_output,
                force_failure_output,
                force_exec_fail_output,
            },
        };

        let load_decision =
            decider.should_load_for_test_finished(success_output, failure_output, &run_statuses);

        if load_decision == LoadOutput::Skip {
            // Derive the same inputs the displayer would compute.
            let describe = run_statuses.describe();
            let last_status = describe.last_status();

            let display = decider.overrides.resolve_test_output_display(
                success_output,
                failure_output,
                &last_status.result,
            );

            let test_status_level = describe.status_level();
            let test_final_status_level = describe.final_status_level();

            let status_levels = StatusLevels {
                status_level,
                final_status_level,
            };

            let output = status_levels.compute_output_on_test_finished(
                display,
                None, // cancel status
                test_status_level,
                test_final_status_level,
                &last_status.result,
            );

            assert!(
                !output.show_immediate,
                "load decider returned Skip but displayer would show immediate output \
                 (display={display:?}, test_status_level={test_status_level:?}, \
                 test_final_status_level={test_final_status_level:?})"
            );
            // The displayer may still store an entry for the status line,
            // but it must not display output bytes (display_output: false).
            if let OutputStoreFinal::Yes {
                display_output: true,
            } = output.store_final
            {
                panic!(
                    "load decider returned Skip but displayer would display final output \
                     (display={display:?}, test_status_level={test_status_level:?}, \
                     test_final_status_level={test_final_status_level:?})"
                );
            }
        }
    }

    /// For TestAttemptFailedWillRetry, the decider's Load/Skip decision
    /// must exactly match whether the displayer would show retry output.
    ///
    /// The displayer shows retry output iff both conditions hold:
    ///
    /// 1. `status_level >= Retry` (the retry line is printed at all)
    /// 2. `resolved_failure_output.is_immediate()` (output is shown inline)
    ///
    /// The decider must return Load for exactly these cases and Skip
    /// otherwise.
    ///
    /// ```text
    /// status_level >= Retry   resolved.is_immediate()   displayer shows   decider
    ///       false                    false                   no             Skip
    ///       false                    true                    no             Skip
    ///       true                     false                   no             Skip
    ///       true                     true                    yes            Load
    /// ```
    #[proptest(cases = 64)]
    fn load_decider_matches_retry_output(
        status_level: StatusLevel,
        failure_output: TestOutputDisplay,
        force_failure_output: Option<TestOutputDisplay>,
    ) {
        let decider = OutputLoadDecider {
            status_level,
            overrides: OutputDisplayOverrides {
                force_success_output: None,
                force_failure_output,
                force_exec_fail_output: None,
            },
        };

        let resolved = decider.overrides.failure_output(failure_output);
        let displayer_would_show = resolved.is_immediate() && status_level >= StatusLevel::Retry;

        let expected = if displayer_would_show {
            LoadOutput::Load
        } else {
            LoadOutput::Skip
        };

        let actual = OutputLoadDecider::should_load_for_retry(resolved, status_level);
        assert_eq!(actual, expected);
    }

    /// For SetupScriptFinished: the decider returns Load iff the result
    /// is not a success (the displayer always shows output for failures).
    #[proptest(cases = 64)]
    fn load_decider_matches_setup_script_output(execution_result: ExecutionResultDescription) {
        let expected = if execution_result.is_success() {
            LoadOutput::Skip
        } else {
            LoadOutput::Load
        };
        let actual = OutputLoadDecider::should_load_for_setup_script(&execution_result);
        assert_eq!(actual, expected);
    }

    // --- Wiring test for should_load_output ---
    //
    // The public entry point should_load_output dispatches to the
    // individual helper methods. This test verifies the dispatch is
    // correct: a wiring error (e.g. passing success_output where
    // failure_output is intended) would be caught.

    /// `should_load_output` must produce the same result as calling the
    /// corresponding helper method for each `OutputEventKind` variant.
    #[proptest(cases = 256)]
    fn should_load_output_consistent_with_helpers(
        status_level: StatusLevel,
        force_success_output: Option<TestOutputDisplay>,
        force_failure_output: Option<TestOutputDisplay>,
        force_exec_fail_output: Option<TestOutputDisplay>,
        event_kind: OutputEventKind<RecordingSpec>,
    ) {
        let decider = OutputLoadDecider {
            status_level,
            overrides: OutputDisplayOverrides {
                force_success_output,
                force_failure_output,
                force_exec_fail_output,
            },
        };

        let actual = decider.should_load_output(&event_kind);

        let expected = match &event_kind {
            OutputEventKind::SetupScriptFinished { run_status, .. } => {
                OutputLoadDecider::should_load_for_setup_script(&run_status.result)
            }
            OutputEventKind::TestAttemptFailedWillRetry { failure_output, .. } => {
                let display = decider.overrides.failure_output(*failure_output);
                OutputLoadDecider::should_load_for_retry(display, status_level)
            }
            OutputEventKind::TestFinished {
                success_output,
                failure_output,
                run_statuses,
                ..
            } => decider.should_load_for_test_finished(
                *success_output,
                *failure_output,
                run_statuses,
            ),
        };

        assert_eq!(
            actual, expected,
            "should_load_output disagrees with individual helper for event kind"
        );
    }
}
