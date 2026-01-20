// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{
    FinalStatusLevel, MaxProgressRunning, StatusLevel, TestOutputDisplay,
    displayer::{DisplayReporter, DisplayReporterBuilder, ShowTerminalProgress, StatusLevels},
};
use crate::{
    config::core::EvaluatableProfile,
    errors::WriteEventError,
    list::TestList,
    record::{ShortestRunIdPrefix, StoreSizes},
    reporter::{
        aggregator::EventAggregator, displayer::ShowProgress, events::*,
        structured::StructuredReporter,
    },
    write_str::WriteStr,
};
use std::time::Duration;

/// Statistics returned by the reporter after a test run completes.
#[derive(Clone, Debug, Default)]
pub struct ReporterStats {
    /// The sizes of the recording written to disk (compressed and uncompressed), or `None` if
    /// recording was not enabled or an error occurred.
    pub recording_sizes: Option<StoreSizes>,
    /// Information captured from the `RunFinished` event.
    pub run_finished: Option<RunFinishedInfo>,
}

/// Information captured from the `RunFinished` event.
///
/// This struct groups together data that is always available together: if we
/// receive a `RunFinished` event, we have both the stats and elapsed time.
#[derive(Clone, Copy, Debug)]
pub struct RunFinishedInfo {
    /// Statistics about the run.
    pub stats: RunFinishedStats,
    /// Total elapsed time for the run.
    pub elapsed: Duration,
    /// The number of tests that were outstanding but not seen during this rerun.
    ///
    /// This is `None` if this was not a rerun. A value of `Some(0)` means all
    /// outstanding tests from the rerun chain were seen during this run (and
    /// either passed or failed).
    pub outstanding_not_seen_count: Option<usize>,
}

/// Output destination for the reporter.
///
/// This is usually a terminal, but can be a writer for paged output or an
/// in-memory buffer for tests.
pub enum ReporterOutput<'a> {
    /// Produce output on the terminal (stderr).
    ///
    /// If the terminal isn't piped, produce output to a progress bar.
    Terminal,

    /// Write output to a `WriteStr` implementation (e.g., for pager support or
    /// an in-memory buffer for tests).
    Writer {
        /// The writer to use for output.
        writer: &'a mut (dyn WriteStr + Send),
        /// Whether to use unicode characters for output.
        ///
        /// The caller should determine this based on the actual output
        /// destination (e.g., by checking `supports_unicode::on()` for the
        /// appropriate stream).
        use_unicode: bool,
    },
}

/// Test reporter builder.
#[derive(Debug, Default)]
pub struct ReporterBuilder {
    no_capture: bool,
    should_colorize: bool,
    failure_output: Option<TestOutputDisplay>,
    success_output: Option<TestOutputDisplay>,
    status_level: Option<StatusLevel>,
    final_status_level: Option<FinalStatusLevel>,

    verbose: bool,
    show_progress: ShowProgress,
    no_output_indent: bool,
    max_progress_running: MaxProgressRunning,
}

impl ReporterBuilder {
    /// Sets no-capture mode.
    ///
    /// In this mode, `failure_output` and `success_output` will be ignored, and `status_level`
    /// will be at least [`StatusLevel::Pass`].
    pub fn set_no_capture(&mut self, no_capture: bool) -> &mut Self {
        self.no_capture = no_capture;
        self
    }

    /// Set to true if the reporter should colorize output.
    pub fn set_colorize(&mut self, should_colorize: bool) -> &mut Self {
        self.should_colorize = should_colorize;
        self
    }

    /// Sets the conditions under which test failures are output.
    pub fn set_failure_output(&mut self, failure_output: TestOutputDisplay) -> &mut Self {
        self.failure_output = Some(failure_output);
        self
    }

    /// Sets the conditions under which test successes are output.
    pub fn set_success_output(&mut self, success_output: TestOutputDisplay) -> &mut Self {
        self.success_output = Some(success_output);
        self
    }

    /// Sets the kinds of statuses to output.
    pub fn set_status_level(&mut self, status_level: StatusLevel) -> &mut Self {
        self.status_level = Some(status_level);
        self
    }

    /// Sets the kinds of statuses to output at the end of the run.
    pub fn set_final_status_level(&mut self, final_status_level: FinalStatusLevel) -> &mut Self {
        self.final_status_level = Some(final_status_level);
        self
    }

    /// Sets verbose output.
    pub fn set_verbose(&mut self, verbose: bool) -> &mut Self {
        self.verbose = verbose;
        self
    }

    /// Sets the way of displaying progress.
    pub fn set_show_progress(&mut self, show_progress: ShowProgress) -> &mut Self {
        self.show_progress = show_progress;
        self
    }

    /// Set to true to disable indentation of captured test output.
    pub fn set_no_output_indent(&mut self, no_output_indent: bool) -> &mut Self {
        self.no_output_indent = no_output_indent;
        self
    }

    /// Sets the maximum number of running tests to display in the progress bar.
    ///
    /// When more tests are running than this limit, only the first N tests are shown
    /// with a summary line indicating how many more tests are running.
    pub fn set_max_progress_running(
        &mut self,
        max_progress_running: MaxProgressRunning,
    ) -> &mut Self {
        self.max_progress_running = max_progress_running;
        self
    }
}

impl ReporterBuilder {
    /// Creates a new test reporter.
    pub fn build<'a>(
        &self,
        test_list: &TestList,
        profile: &EvaluatableProfile<'a>,
        show_term_progress: ShowTerminalProgress,
        output: ReporterOutput<'a>,
        structured_reporter: StructuredReporter<'a>,
    ) -> Reporter<'a> {
        let aggregator = EventAggregator::new(test_list.mode(), profile);

        let status_level = self.status_level.unwrap_or_else(|| profile.status_level());
        let final_status_level = self
            .final_status_level
            .unwrap_or_else(|| profile.final_status_level());

        let display_reporter = DisplayReporterBuilder {
            mode: test_list.mode(),
            default_filter: profile.default_filter().clone(),
            status_levels: StatusLevels {
                status_level,
                final_status_level,
            },
            test_count: test_list.test_count(),
            success_output: self.success_output,
            failure_output: self.failure_output,
            should_colorize: self.should_colorize,
            no_capture: self.no_capture,
            verbose: self.verbose,
            show_progress: self.show_progress,
            no_output_indent: self.no_output_indent,
            max_progress_running: self.max_progress_running,
            show_term_progress,
        }
        .build(output);

        Reporter {
            display_reporter,
            structured_reporter,
            metadata_reporter: aggregator,
            run_finished: None,
        }
    }
}

/// Functionality to report test results to stderr, JUnit, and/or structured,
/// machine-readable results to stdout.
pub struct Reporter<'a> {
    /// Used to display results to standard error.
    display_reporter: DisplayReporter<'a>,
    /// Used to aggregate events for JUnit reports written to disk
    metadata_reporter: EventAggregator<'a>,
    /// Used to emit test events in machine-readable format(s) to stdout
    structured_reporter: StructuredReporter<'a>,
    /// Information captured from the RunFinished event.
    run_finished: Option<RunFinishedInfo>,
}

impl<'a> Reporter<'a> {
    /// Report a test event.
    pub fn report_event(&mut self, event: ReporterEvent<'a>) -> Result<(), WriteEventError> {
        match event {
            ReporterEvent::Tick => {
                self.tick();
                Ok(())
            }
            ReporterEvent::Test(event) => self.write_event(event),
        }
    }

    /// Mark the reporter done.
    ///
    /// Returns statistics about the test run, including the size of the
    /// recording if recording was enabled.
    pub fn finish(mut self) -> ReporterStats {
        self.display_reporter.finish();
        let recording_sizes = self.structured_reporter.finish();
        ReporterStats {
            recording_sizes,
            run_finished: self.run_finished,
        }
    }

    /// Sets the unique prefix for the run ID.
    ///
    /// This is used to highlight the unique prefix portion of the run ID
    /// in the `RunStarted` output when a recording session is active.
    pub fn set_run_id_unique_prefix(&mut self, prefix: ShortestRunIdPrefix) {
        self.display_reporter.set_run_id_unique_prefix(prefix);
    }

    // ---
    // Helper methods
    // ---

    /// Tick the reporter, updating displayed state.
    fn tick(&mut self) {
        self.display_reporter.tick();
    }

    /// Report this test event to the given writer.
    fn write_event(&mut self, event: Box<TestEvent<'a>>) -> Result<(), WriteEventError> {
        // Capture run finished info before passing to reporters.
        if let TestEventKind::RunFinished {
            run_stats,
            elapsed,
            outstanding_not_seen,
            ..
        } = &event.kind
        {
            self.run_finished = Some(RunFinishedInfo {
                stats: *run_stats,
                elapsed: *elapsed,
                outstanding_not_seen_count: outstanding_not_seen.as_ref().map(|t| t.total_not_seen),
            });
        }

        // TODO: write to all of these even if one of them fails?
        self.display_reporter.write_event(&event)?;
        self.structured_reporter.write_event(&event)?;
        self.metadata_reporter.write_event(event)?;
        Ok(())
    }
}
