// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{
    FinalStatusLevel, MaxProgressRunning, StatusLevel, TestOutputDisplay,
    displayer::{DisplayReporter, DisplayReporterBuilder, StatusLevels},
};
use crate::{
    cargo_config::CargoConfigs,
    config::core::EvaluatableProfile,
    errors::WriteEventError,
    list::TestList,
    reporter::{
        aggregator::EventAggregator, displayer::ShowProgress, events::*,
        structured::StructuredReporter,
    },
};

/// Standard error destination for the reporter.
///
/// This is usually a terminal, but can be an in-memory buffer for tests.
pub enum ReporterStderr<'a> {
    /// Produce output on the (possibly piped) terminal.
    ///
    /// If the terminal isn't piped, produce output to a progress bar.
    Terminal,

    /// Write output to a buffer.
    Buffer(&'a mut Vec<u8>),
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
        cargo_configs: &CargoConfigs,
        output: ReporterStderr<'a>,
        structured_reporter: StructuredReporter<'a>,
    ) -> Reporter<'a> {
        let aggregator = EventAggregator::new(profile);

        let status_level = self.status_level.unwrap_or_else(|| profile.status_level());
        let final_status_level = self
            .final_status_level
            .unwrap_or_else(|| profile.final_status_level());

        let display_reporter = DisplayReporterBuilder {
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
            show_progress: self.show_progress,
            no_output_indent: self.no_output_indent,
            max_progress_running: self.max_progress_running,
        }
        .build(cargo_configs, output);

        Reporter {
            display_reporter,
            structured_reporter,
            metadata_reporter: aggregator,
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
    pub fn finish(&mut self) {
        self.display_reporter.finish();
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
        // TODO: write to all of these even if one of them fails?
        self.display_reporter.write_event(&event)?;
        self.structured_reporter.write_event(&event)?;
        self.metadata_reporter.write_event(event)?;
        Ok(())
    }
}
