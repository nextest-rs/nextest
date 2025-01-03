// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Status levels: filters for which test statuses are displayed.
//!
//! Status levels play a role that's similar to log levels in typical loggers.

use super::TestOutputDisplay;
use crate::reporter::events::CancelReason;
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
    ) -> OutputOnTestFinished {
        let write_status_line = self.status_level >= test_status_level;

        let is_immediate = display.is_immediate();
        // We store entries in the final output map if either the final status level is high enough or
        // if `display` says we show the output at the end.
        let is_final = display.is_final() || self.final_status_level >= test_final_status_level;

        // This table is tested below. The basic invariant is that we generally follow what
        // is_immediate and is_final suggests, except:
        //
        // - if the run is cancelled due to a non-interrupt signal, we display test output at most
        //   once.
        // - if the run is cancelled due to an interrupt, we hide the output because dumping a bunch
        //   of output at the end is likely to not be helpful (though in the future we may want to
        //   at least dump outputs into files and write their names out, or whenever nextest gains
        //   the ability to replay test runs to be able to display it then.)
        //
        // is_immediate  is_final  cancel_status  |  show_immediate  store_final
        //
        //     false      false      <= Signal    |     false          false
        //     false       true      <= Signal    |     false           true  [1]
        //      true      false      <= Signal    |      true          false  [1]
        //      true       true       < Signal    |      true           true
        //      true       true         Signal    |      true          false  [2]
        //       *           *       Interrupt    |     false          false
        //
        // [1] In non-interrupt cases, we want to display output if specified once.
        //
        // [2] If there's a signal, we shouldn't display output twice at the end since it's
        // redundant -- instead, just show the output as part of the immediate display.
        let show_immediate = is_immediate && cancel_status <= Some(CancelReason::Signal);

        let store_final = if is_final && cancel_status < Some(CancelReason::Signal)
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
    use test_strategy::proptest;

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
        );
        assert_eq!(
            actual.store_final,
            OutputStoreFinal::Yes {
                display_output: false,
            }
        );
    }
}
