// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

mod aggregator;
pub use aggregator::heuristic_extract_description;
use uuid::Uuid;

use crate::{
    config::NextestProfile,
    errors::WriteEventError,
    helpers::write_test_name,
    list::{TestInstance, TestList},
    reporter::aggregator::EventAggregator,
    runner::{
        AbortStatus, ExecuteStatus, ExecutionDescription, ExecutionResult, ExecutionStatuses,
        RetryData, RunStats,
    },
};
use debug_ignore::DebugIgnore;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use nextest_metadata::MismatchReason;
use owo_colors::{OwoColorize, Style};
use serde::Deserialize;
use std::{
    borrow::Cow,
    cmp::Reverse,
    fmt::{self, Write as _},
    io,
    io::{BufWriter, Write},
    time::{Duration, SystemTime},
};

/// When to display test output in the reporter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestOutputDisplay {
    /// Show output immediately on execution completion.
    ///
    /// This is the default for failing tests.
    Immediate,

    /// Show output immediately, and at the end of a test run.
    ImmediateFinal,

    /// Show output at the end of execution.
    Final,

    /// Never show output.
    Never,
}

impl TestOutputDisplay {
    /// Returns true if test output is shown immediately.
    pub fn is_immediate(self) -> bool {
        match self {
            TestOutputDisplay::Immediate | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Final | TestOutputDisplay::Never => false,
        }
    }

    /// Returns true if test output is shown at the end of the run.
    pub fn is_final(self) -> bool {
        match self {
            TestOutputDisplay::Final | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Immediate | TestOutputDisplay::Never => false,
        }
    }
}

/// Status level to show in the reporter output.
///
/// Status levels are incremental: each level causes all the statuses listed above it to be output. For example,
/// [`Slow`](Self::Slow) implies [`Retry`](Self::Retry) and [`Fail`](Self::Fail).
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize)]
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
pub struct TestReporterBuilder {
    no_capture: bool,
    failure_output: Option<TestOutputDisplay>,
    success_output: Option<TestOutputDisplay>,
    status_level: Option<StatusLevel>,
    final_status_level: Option<FinalStatusLevel>,
    verbose: bool,
    hide_progress_bar: bool,
}

impl TestReporterBuilder {
    /// Sets no-capture mode.
    ///
    /// In this mode, `failure_output` and `success_output` will be ignored, and `status_level`
    /// will be at least [`StatusLevel::Pass`].
    pub fn set_no_capture(&mut self, no_capture: bool) -> &mut Self {
        self.no_capture = no_capture;
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

    /// Sets visibility of the progress bar.
    /// The progress bar is also hidden if `no_capture` is set.
    pub fn set_hide_progress_bar(&mut self, hide_progress_bar: bool) -> &mut Self {
        self.hide_progress_bar = hide_progress_bar;
        self
    }
}

impl TestReporterBuilder {
    /// Creates a new test reporter.
    pub fn build<'a>(
        &self,
        test_list: &TestList,
        profile: &NextestProfile<'a>,
        output: ReporterStderr<'a>,
    ) -> TestReporter<'a> {
        let styles = Box::default();
        let binary_id_width = test_list
            .iter()
            .filter_map(|(_, info)| (info.status.test_count() > 0).then_some(info.binary_id.len()))
            .max()
            .unwrap_or_default();
        let aggregator = EventAggregator::new(profile);

        let status_level = self.status_level.unwrap_or_else(|| profile.status_level());
        let status_level = match self.no_capture {
            // In no-capture mode, the status level is treated as at least pass.
            true => status_level.max(StatusLevel::Pass),
            false => status_level,
        };
        let final_status_level = self
            .final_status_level
            .unwrap_or_else(|| profile.final_status_level());

        // failure_output and success_output are meaningless if the runner isn't capturing any
        // output.
        let failure_output = match self.no_capture {
            true => TestOutputDisplay::Never,
            false => self
                .failure_output
                .unwrap_or_else(|| profile.failure_output()),
        };
        let success_output = match self.no_capture {
            true => TestOutputDisplay::Never,
            false => self
                .success_output
                .unwrap_or_else(|| profile.success_output()),
        };

        let stderr = match output {
            ReporterStderr::Terminal if self.no_capture => {
                // Do not use a progress bar if --no-capture is passed in. This is required since we
                // pass down stderr to the child process.
                //
                // In the future, we could potentially switch to using a pty, in which case we could
                // still potentially use the progress bar as a status bar. However, that brings
                // about its own complications: what if a test's output doesn't include a newline?
                // We might have to use a curses-like UI which would be a lot of work for not much
                // gain.
                ReporterStderrImpl::TerminalWithoutBar
            }
            ReporterStderr::Terminal if is_ci::uncached() => {
                // Some CI environments appear to pretend to be a terminal. Disable the progress bar
                // in these environments.
                ReporterStderrImpl::TerminalWithoutBar
            }
            ReporterStderr::Terminal if self.hide_progress_bar => {
                ReporterStderrImpl::TerminalWithoutBar
            }

            ReporterStderr::Terminal => {
                let progress_bar = ProgressBar::new(test_list.test_count() as u64);
                // Emulate Cargo's style.
                let test_count_width = format!("{}", test_list.test_count()).len();
                // Create the template using the width as input. This is a little confusing -- {{foo}}
                // is what's passed into the ProgressBar, while {bar} is inserted by the format!() statement.
                //
                // Note: ideally we'd use the same format as our other duration displays for the elapsed time,
                // but that isn't possible due to https://github.com/console-rs/indicatif/issues/440. Use
                // {{elapsed_precise}} as an OK tradeoff here.
                let template = format!(
                    "{{prefix:>12}} [{{elapsed_precise:>9}}] [{{wide_bar}}] \
                    {{pos:>{test_count_width}}}/{{len:{test_count_width}}}: {{msg}}     "
                );
                progress_bar.set_style(
                    ProgressStyle::default_bar()
                        .progress_chars("=> ")
                        .template(&template)
                        .expect("template is known to be valid"),
                );
                // NOTE: set_draw_target must be called before enable_steady_tick to avoid a
                // spurious extra line from being printed as the draw target changes.
                //
                // This used to be unbuffered, but that option went away from indicatif 0.17.0. The
                // refresh rate is now 20hz so that it's double the steady tick rate.
                progress_bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
                // Enable a steady tick 10 times a second.
                progress_bar.enable_steady_tick(Duration::from_millis(100));
                ReporterStderrImpl::TerminalWithBar(progress_bar)
            }
            ReporterStderr::Buffer(buf) => ReporterStderrImpl::Buffer(buf),
        };

        TestReporter {
            inner: TestReporterImpl {
                status_level,
                final_status_level,
                failure_output,
                success_output,
                no_capture: self.no_capture,
                binary_id_width,
                styles,
                cancel_status: None,
                final_outputs: DebugIgnore(vec![]),
            },
            stderr,
            metadata_reporter: aggregator,
        }
    }
}

enum ReporterStderrImpl<'a> {
    TerminalWithBar(ProgressBar),
    TerminalWithoutBar,
    Buffer(&'a mut Vec<u8>),
}

/// Functionality to report test results to stderr and JUnit
pub struct TestReporter<'a> {
    inner: TestReporterImpl<'a>,
    stderr: ReporterStderrImpl<'a>,
    metadata_reporter: EventAggregator<'a>,
}

impl<'a> TestReporter<'a> {
    /// Colorizes output.
    pub fn colorize(&mut self) {
        self.inner.styles.colorize();
    }

    /// Report a test event.
    pub fn report_event(&mut self, event: TestEvent<'a>) -> Result<(), WriteEventError> {
        self.write_event(event)
    }

    // ---
    // Helper methods
    // ---

    /// Report this test event to the given writer.
    fn write_event(&mut self, event: TestEvent<'a>) -> Result<(), WriteEventError> {
        match &mut self.stderr {
            ReporterStderrImpl::TerminalWithBar(progress_bar) => {
                // Write to a string that will be printed as a log line.
                let mut buf: Vec<u8> = Vec::new();
                self.inner
                    .write_event_impl(&event, &mut buf)
                    .map_err(WriteEventError::Io)?;
                // ProgressBar::println doesn't print status lines if the bar is hidden. The suspend
                // method prints it in both cases.
                progress_bar.suspend(|| {
                    _ = std::io::stderr().write_all(&buf);
                });

                update_progress_bar(&event, &self.inner.styles, progress_bar);
            }
            ReporterStderrImpl::TerminalWithoutBar => {
                // Write to a buffered stderr.
                let mut writer = BufWriter::new(std::io::stderr());
                self.inner
                    .write_event_impl(&event, &mut writer)
                    .map_err(WriteEventError::Io)?;
                writer.flush().map_err(WriteEventError::Io)?;
            }
            ReporterStderrImpl::Buffer(buf) => {
                self.inner
                    .write_event_impl(&event, buf)
                    .map_err(WriteEventError::Io)?;
            }
        }
        self.metadata_reporter.write_event(event)?;
        Ok(())
    }
}

fn update_progress_bar<'a>(event: &TestEvent<'a>, styles: &Styles, progress_bar: &mut ProgressBar) {
    match event {
        TestEvent::TestStarted {
            current_stats,
            running,
            cancel_state,
            ..
        }
        | TestEvent::TestFinished {
            current_stats,
            running,
            cancel_state,
            ..
        } => {
            let running_state = RunningState::new(*cancel_state, current_stats);
            progress_bar.set_prefix(running_state.progress_bar_prefix(styles));
            progress_bar.set_message(progress_bar_msg(current_stats, *running, styles));
            // If there are skipped tests, the initial run count will be lower than when constructed
            // in ProgressBar::new.
            progress_bar.set_length(current_stats.initial_run_count as u64);
            progress_bar.set_position(current_stats.finished_count as u64);
        }
        TestEvent::RunBeginCancel { reason, .. } => {
            let running_state = RunningState::Canceling(*reason);
            progress_bar.set_prefix(running_state.progress_bar_prefix(styles));
        }
        _ => {}
    }
}

#[derive(Copy, Clone, Debug)]
enum RunningState<'a> {
    Running(&'a RunStats),
    Canceling(CancelReason),
}

impl<'a> RunningState<'a> {
    fn new(cancel_state: Option<CancelReason>, current_stats: &'a RunStats) -> Self {
        match cancel_state {
            Some(cancel_state) => Self::Canceling(cancel_state),
            None => Self::Running(current_stats),
        }
    }

    fn progress_bar_prefix(self, styles: &Styles) -> String {
        let (prefix_str, prefix_style) = match self {
            Self::Running(current_stats) => {
                let prefix_style = if current_stats.any_failed() {
                    styles.fail
                } else {
                    styles.pass
                };
                ("Running", prefix_style)
            }
            Self::Canceling(_) => ("Canceling", styles.fail),
        };

        format!("{:>12}", prefix_str.style(prefix_style))
    }
}

fn progress_bar_msg(current_stats: &RunStats, running: usize, styles: &Styles) -> String {
    let mut s = format!("{} running, ", running.style(styles.count));
    // Writing to strings is infallible.
    let _ = write_summary_str(current_stats, styles, &mut s);
    s
}

fn write_summary_str(run_stats: &RunStats, styles: &Styles, out: &mut String) -> fmt::Result {
    write!(
        out,
        "{} {}",
        run_stats.passed.style(styles.count),
        "passed".style(styles.pass)
    )?;

    if run_stats.passed_slow > 0 || run_stats.flaky > 0 || run_stats.leaky > 0 {
        let mut text = Vec::with_capacity(3);
        if run_stats.passed_slow > 0 {
            text.push(format!(
                "{} {}",
                run_stats.passed_slow.style(styles.count),
                "slow".style(styles.skip),
            ));
        }
        if run_stats.flaky > 0 {
            text.push(format!(
                "{} {}",
                run_stats.flaky.style(styles.count),
                "flaky".style(styles.skip),
            ));
        }
        if run_stats.leaky > 0 {
            text.push(format!(
                "{} {}",
                run_stats.leaky.style(styles.count),
                "leaky".style(styles.skip),
            ));
        }
        write!(out, " ({})", text.join(", "))?;
    }
    write!(out, ", ")?;

    if run_stats.failed > 0 {
        write!(
            out,
            "{} {}, ",
            run_stats.failed.style(styles.count),
            "failed".style(styles.fail),
        )?;
    }

    if run_stats.exec_failed > 0 {
        write!(
            out,
            "{} {}, ",
            run_stats.exec_failed.style(styles.count),
            "exec failed".style(styles.fail),
        )?;
    }

    if run_stats.timed_out > 0 {
        write!(
            out,
            "{} {}, ",
            run_stats.timed_out.style(styles.count),
            "timed out".style(styles.fail),
        )?;
    }

    write!(
        out,
        "{} {}",
        run_stats.skipped.style(styles.count),
        "skipped".style(styles.skip),
    )?;

    Ok(())
}

#[derive(Debug)]
enum FinalOutput {
    Skipped(MismatchReason),
    Executed(ExecutionStatuses),
}

impl FinalOutput {
    fn final_status_level(&self) -> FinalStatusLevel {
        match self {
            Self::Skipped(_) => FinalStatusLevel::Skip,
            Self::Executed(run_statuses) => run_statuses.describe().final_status_level(),
        }
    }
}

struct TestReporterImpl<'a> {
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    no_capture: bool,
    binary_id_width: usize,
    styles: Box<Styles>,
    cancel_status: Option<CancelReason>,
    final_outputs: DebugIgnore<Vec<(TestInstance<'a>, FinalOutput)>>,
}

impl<'a> TestReporterImpl<'a> {
    fn write_event_impl(
        &mut self,
        event: &TestEvent<'a>,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        match event {
            TestEvent::RunStarted { test_list, .. } => {
                write!(writer, "{:>12} ", "Starting".style(self.styles.pass))?;

                let count_style = self.styles.count;

                write!(
                    writer,
                    "{} tests across {} binaries",
                    test_list.run_count().style(count_style),
                    test_list.binary_count().style(count_style),
                )?;

                let skip_count = test_list.skip_count();
                if skip_count > 0 {
                    write!(writer, " ({} skipped)", skip_count.style(count_style))?;
                }

                writeln!(writer)?;
            }
            TestEvent::TestStarted { test_instance, .. } => {
                // In no-capture mode, print out a test start event.
                if self.no_capture {
                    // The spacing is to align test instances.
                    write!(
                        writer,
                        "{:>12}             ",
                        "START".style(self.styles.pass),
                    )?;
                    self.write_instance(*test_instance, writer)?;
                    writeln!(writer)?;
                }
            }
            TestEvent::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => {
                if !*will_terminate && self.status_level >= StatusLevel::Slow {
                    if retry_data.total_attempts > 1 {
                        write!(
                            writer,
                            "{:>12} ",
                            format!("TRY {} SLOW", retry_data.attempt).style(self.styles.skip)
                        )?;
                    } else {
                        write!(writer, "{:>12} ", "SLOW".style(self.styles.skip))?;
                    }
                } else if *will_terminate {
                    let (required_status_level, style) = if retry_data.is_last_attempt() {
                        (StatusLevel::Fail, self.styles.fail)
                    } else {
                        (StatusLevel::Retry, self.styles.retry)
                    };
                    if retry_data.total_attempts > 1 && self.status_level > required_status_level {
                        write!(
                            writer,
                            "{:>12} ",
                            format!("TRY {} TRMNTG", retry_data.attempt).style(style)
                        )?;
                    } else {
                        write!(writer, "{:>12} ", "TERMINATING".style(style))?;
                    };
                }

                self.write_slow_duration(*elapsed, writer)?;
                self.write_instance(*test_instance, writer)?;
                writeln!(writer)?;
            }

            TestEvent::TestAttemptFailedWillRetry {
                test_instance,
                run_status,
                delay_before_next_attempt,
            } => {
                if self.status_level >= StatusLevel::Retry {
                    let try_status_string = format!(
                        "TRY {} {}",
                        run_status.retry_data.attempt,
                        short_status_str(run_status.result),
                    );
                    write!(
                        writer,
                        "{:>12} ",
                        try_status_string.style(self.styles.retry)
                    )?;

                    // Next, print the time taken.
                    self.write_duration(run_status.time_taken, writer)?;

                    // Print the name of the test.
                    self.write_instance(*test_instance, writer)?;
                    writeln!(writer)?;

                    // This test is guaranteed to have failed.
                    assert!(
                        !run_status.result.is_success(),
                        "only failing tests are retried"
                    );
                    if self.failure_output.is_immediate() {
                        self.write_stdout_stderr(test_instance, run_status, true, writer)?;
                    }

                    // The final output doesn't show retries, so don't store this result in
                    // final_outputs.

                    if !delay_before_next_attempt.is_zero() {
                        // Print a "DELAY {}/{}" line.
                        let delay_string = format!(
                            "DELAY {}/{}",
                            run_status.retry_data.attempt + 1,
                            run_status.retry_data.total_attempts,
                        );
                        write!(writer, "{:>12} ", delay_string.style(self.styles.retry))?;

                        self.write_duration_by(*delay_before_next_attempt, writer)?;

                        // Print the name of the test.
                        self.write_instance(*test_instance, writer)?;
                        writeln!(writer)?;
                    }
                }
            }
            TestEvent::TestRetryStarted {
                test_instance,
                retry_data:
                    RetryData {
                        attempt,
                        total_attempts,
                    },
            } => {
                let retry_string = format!("RETRY {attempt}/{total_attempts}");
                write!(writer, "{:>12} ", retry_string.style(self.styles.retry))?;

                // Add spacing to align test instances.
                write!(writer, "[{:<9}] ", "")?;

                // Print the name of the test.
                self.write_instance(*test_instance, writer)?;
                writeln!(writer)?;
            }
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => {
                let describe = run_statuses.describe();
                let last_status = run_statuses.last_status();
                let test_output_display = match last_status.result.is_success() {
                    true => self.success_output,
                    false => self.failure_output,
                };

                if self.status_level >= describe.status_level() {
                    self.write_status_line(*test_instance, describe, writer)?;

                    // If the test failed to execute, print its output and error status.
                    // (don't print out test failures after Ctrl-C)
                    if self.cancel_status < Some(CancelReason::Signal)
                        && test_output_display.is_immediate()
                    {
                        self.write_stdout_stderr(test_instance, last_status, false, writer)?;
                    }
                }

                // Store the output in final_outputs if test output display is requested, or if
                // we have to print a one-line summary at the end.
                if test_output_display.is_final()
                    || self.final_status_level >= describe.final_status_level()
                {
                    self.final_outputs
                        .push((*test_instance, FinalOutput::Executed(run_statuses.clone())));
                }
            }
            TestEvent::TestSkipped {
                test_instance,
                reason,
            } => {
                if self.status_level >= StatusLevel::Skip {
                    self.write_skip_line(*test_instance, writer)?;
                }
                if self.final_status_level >= FinalStatusLevel::Skip {
                    self.final_outputs
                        .push((*test_instance, FinalOutput::Skipped(*reason)));
                }
            }
            TestEvent::RunBeginCancel { running, reason } => {
                self.cancel_status = self.cancel_status.max(Some(*reason));

                write!(writer, "{:>12} ", "Canceling".style(self.styles.fail))?;
                let reason_str = match reason {
                    CancelReason::TestFailure => "test failure",
                    CancelReason::ReportError => "error",
                    CancelReason::Signal => "signal",
                    CancelReason::Interrupt => "interrupt",
                };

                writeln!(
                    writer,
                    "due to {}: {} tests still running",
                    reason_str.style(self.styles.fail),
                    running.style(self.styles.count)
                )?;
            }
            TestEvent::RunPaused { running } => {
                writeln!(
                    writer,
                    "{:>12} {} running tests due to {}",
                    "Pausing".style(self.styles.pass),
                    running.style(self.styles.count),
                    "signal".style(self.styles.count),
                )?;
            }
            TestEvent::RunContinued { running } => {
                writeln!(
                    writer,
                    "{:>12} {} running tests due to {}",
                    "Continuing".style(self.styles.pass),
                    running.style(self.styles.count),
                    "signal".style(self.styles.count),
                )?;
            }
            TestEvent::RunFinished {
                start_time: _start_time,
                elapsed,
                run_stats,
                ..
            } => {
                let summary_style = if run_stats.any_failed() {
                    self.styles.fail
                } else {
                    self.styles.pass
                };
                write!(
                    writer,
                    "------------\n{:>12} ",
                    "Summary".style(summary_style)
                )?;

                // Next, print the total time taken.
                // * > means right-align.
                // * 8 is the number of characters to pad to.
                // * .3 means print two digits after the decimal point.
                // TODO: better time printing mechanism than this
                write!(writer, "[{:>8.3?}s] ", elapsed.as_secs_f64())?;

                write!(
                    writer,
                    "{}",
                    run_stats.finished_count.style(self.styles.count)
                )?;
                if run_stats.finished_count != run_stats.initial_run_count {
                    write!(
                        writer,
                        "/{}",
                        run_stats.initial_run_count.style(self.styles.count)
                    )?;
                }

                let mut summary_str = String::new();
                // Writing to a string is infallible.
                let _ = write_summary_str(run_stats, &self.styles, &mut summary_str);
                writeln!(writer, " tests run: {summary_str}")?;

                // Don't print out final outputs if canceled due to Ctrl-C.
                if self.cancel_status < Some(CancelReason::Signal) {
                    // Sort the final outputs for a friendlier experience.
                    self.final_outputs
                        .sort_by_key(|(test_instance, final_output)| {
                            // Use the final status level, reversed (i.e. failing tests are printed at the very end).
                            (
                                Reverse(final_output.final_status_level()),
                                test_instance.sort_key(),
                            )
                        });

                    for (test_instance, final_output) in &*self.final_outputs {
                        let final_status_level = final_output.final_status_level();
                        match final_output {
                            FinalOutput::Skipped(_) => {
                                self.write_skip_line(*test_instance, writer)?;
                            }
                            FinalOutput::Executed(run_statuses) => {
                                let last_status = run_statuses.last_status();
                                let test_output_display = match last_status.result.is_success() {
                                    true => self.success_output,
                                    false => self.failure_output,
                                };

                                // Print out the final status line so that status lines are shown
                                // for tests that e.g. failed due to signals.
                                if self.final_status_level >= final_status_level
                                    || test_output_display.is_final()
                                {
                                    self.write_final_status_line(
                                        *test_instance,
                                        run_statuses.describe(),
                                        writer,
                                    )?;
                                }
                                if test_output_display.is_final() {
                                    self.write_stdout_stderr(
                                        test_instance,
                                        last_status,
                                        false,
                                        writer,
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn write_skip_line(
        &self,
        test_instance: TestInstance<'a>,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
        // same spacing [   0.034s]
        write!(writer, "[         ] ")?;

        self.write_instance(test_instance, writer)?;
        writeln!(writer)?;

        Ok(())
    }

    fn write_status_line(
        &self,
        test_instance: TestInstance<'a>,
        describe: ExecutionDescription<'_>,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let last_status = describe.last_status();
        match describe {
            ExecutionDescription::Success { .. } => {
                if last_status.result == ExecutionResult::Leak {
                    write!(writer, "{:>12} ", "LEAK".style(self.styles.skip))?;
                } else {
                    write!(writer, "{:>12} ", "PASS".style(self.styles.pass))?;
                }
            }
            ExecutionDescription::Flaky { .. } => {
                // Use the skip color to also represent a flaky test.
                write!(
                    writer,
                    "{:>12} ",
                    format!("TRY {} PASS", last_status.retry_data.attempt).style(self.styles.skip)
                )?;
            }
            ExecutionDescription::Failure { .. } => {
                if last_status.retry_data.attempt == 1 {
                    write!(
                        writer,
                        "{:>12} ",
                        status_str(last_status.result).style(self.styles.fail)
                    )?;
                } else {
                    let status_str = short_status_str(last_status.result);
                    write!(
                        writer,
                        "{:>12} ",
                        format!("TRY {} {}", last_status.retry_data.attempt, status_str)
                            .style(self.styles.fail)
                    )?;
                }
            }
        };

        // Next, print the time taken.
        self.write_duration(last_status.time_taken, writer)?;

        // Print the name of the test.
        self.write_instance(test_instance, writer)?;
        writeln!(writer)?;

        // On Windows, also print out the exception if available.
        #[cfg(windows)]
        if let ExecutionResult::Fail {
            abort_status: Some(AbortStatus::WindowsNtStatus(nt_status)),
            leaked: _,
        } = last_status.result
        {
            self.write_windows_message_line(nt_status, writer)?;
        }

        Ok(())
    }

    fn write_final_status_line(
        &self,
        test_instance: TestInstance<'a>,
        describe: ExecutionDescription<'_>,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let last_status = describe.last_status();
        match describe {
            ExecutionDescription::Success { .. } => {
                match (last_status.is_slow, last_status.result) {
                    (true, ExecutionResult::Leak) => {
                        write!(writer, "{:>12} ", "SLOW + LEAK".style(self.styles.skip))?;
                    }
                    (true, _) => {
                        write!(writer, "{:>12} ", "SLOW".style(self.styles.skip))?;
                    }
                    (false, ExecutionResult::Leak) => {
                        write!(writer, "{:>12} ", "LEAK".style(self.styles.skip))?;
                    }
                    (false, _) => {
                        write!(writer, "{:>12} ", "PASS".style(self.styles.pass))?;
                    }
                }
            }
            ExecutionDescription::Flaky { .. } => {
                // Use the skip color to also represent a flaky test.
                write!(
                    writer,
                    "{:>12} ",
                    format!(
                        "FLAKY {}/{}",
                        last_status.retry_data.attempt, last_status.retry_data.total_attempts
                    )
                    .style(self.styles.skip)
                )?;
            }
            ExecutionDescription::Failure { .. } => {
                if last_status.retry_data.attempt == 1 {
                    write!(
                        writer,
                        "{:>12} ",
                        status_str(last_status.result).style(self.styles.fail)
                    )?;
                } else {
                    let status_str = short_status_str(last_status.result);
                    write!(
                        writer,
                        "{:>12} ",
                        format!("TRY {} {}", last_status.retry_data.attempt, status_str)
                            .style(self.styles.fail)
                    )?;
                }
            }
        };

        // Next, print the time taken.
        self.write_duration(last_status.time_taken, writer)?;

        // Print the name of the test.
        self.write_instance(test_instance, writer)?;
        writeln!(writer)?;

        // On Windows, also print out the exception if available.
        #[cfg(windows)]
        if let ExecutionResult::Fail {
            abort_status: Some(AbortStatus::WindowsNtStatus(nt_status)),
            leaked: _,
        } = last_status.result
        {
            self.write_windows_message_line(nt_status, writer)?;
        }

        Ok(())
    }

    fn write_instance(
        &self,
        instance: TestInstance<'a>,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        write!(
            writer,
            "{:>width$} ",
            instance
                .suite_info
                .binary_id
                .style(self.styles.list_styles.binary_id),
            width = self.binary_id_width
        )?;

        write_test_name(instance.name, &self.styles.list_styles, writer)
    }

    fn write_duration(&self, duration: Duration, writer: &mut impl Write) -> io::Result<()> {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "[{:>8.3?}s] ", duration.as_secs_f64())
    }

    fn write_duration_by(&self, duration: Duration, writer: &mut impl Write) -> io::Result<()> {
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "by {:>7.3?}s ", duration.as_secs_f64())
    }

    fn write_slow_duration(&self, duration: Duration, writer: &mut impl Write) -> io::Result<()> {
        // Inside the curly braces:
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "[>{:>7.3?}s] ", duration.as_secs_f64())
    }

    #[cfg(windows)]
    fn write_windows_message_line(
        &self,
        nt_status: windows::Win32::Foundation::NTSTATUS,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "Message".style(self.styles.fail))?;
        write!(writer, "[         ] ")?;
        writeln!(
            writer,
            "code {}",
            crate::helpers::display_nt_status(nt_status)
        )?;

        Ok(())
    }

    fn write_stdout_stderr(
        &self,
        test_instance: &TestInstance<'a>,
        run_status: &ExecuteStatus,
        is_retry: bool,
        writer: &mut impl Write,
    ) -> io::Result<()> {
        let (header_style, _output_style) = if is_retry {
            (self.styles.retry, self.styles.retry_output)
        } else if run_status.result.is_success() {
            (self.styles.pass, self.styles.pass_output)
        } else {
            (self.styles.fail, self.styles.fail_output)
        };

        if !run_status.stdout.is_empty() {
            write!(writer, "\n{}", "--- ".style(header_style))?;
            let out_len = self.write_attempt(run_status, header_style, writer)?;
            // The width is to align test instances.
            write!(
                writer,
                "{:width$}",
                "STDOUT:".style(header_style),
                width = (21 - out_len)
            )?;
            self.write_instance(*test_instance, writer)?;
            writeln!(writer, "{}", " ---".style(header_style))?;

            self.write_test_output(&run_status.stdout, writer)?;
        }

        if !run_status.stderr.is_empty() {
            write!(writer, "\n{}", "--- ".style(header_style))?;
            let out_len = self.write_attempt(run_status, header_style, writer)?;
            // The width is to align test instances.
            write!(
                writer,
                "{:width$}",
                "STDERR:".style(header_style),
                width = (21 - out_len)
            )?;
            self.write_instance(*test_instance, writer)?;
            writeln!(writer, "{}", " ---".style(header_style))?;

            self.write_test_output(&run_status.stderr, writer)?;
        }

        writeln!(writer)
    }

    fn write_test_output(&self, output: &[u8], writer: &mut impl Write) -> io::Result<()> {
        if self.styles.is_colorized {
            const RESET_COLOR: &[u8] = b"\x1b[0m";
            // Output the text without stripping ANSI escapes, then reset the color afterwards in case
            // the output is malformed.
            writer.write_all(output)?;
            writer.write_all(RESET_COLOR)?;
        } else {
            // Strip ANSI escapes from the output if nextest itself isn't colorized.
            let mut no_color = strip_ansi_escapes::Writer::new(writer);
            no_color.write_all(output)?;
        }

        Ok(())
    }

    // Returns the number of characters written out to the screen.
    fn write_attempt(
        &self,
        run_status: &ExecuteStatus,
        style: Style,
        writer: &mut impl Write,
    ) -> io::Result<usize> {
        if run_status.retry_data.total_attempts > 1 {
            // 3 for 'TRY' + 1 for ' ' + length of the current attempt + 1 for following space.
            let attempt_str = format!("{}", run_status.retry_data.attempt);
            let out_len = 3 + 1 + attempt_str.len() + 1;
            write!(
                writer,
                "{} {} ",
                "TRY".style(style),
                attempt_str.style(style)
            )?;
            Ok(out_len)
        } else {
            Ok(0)
        }
    }
}

impl<'a> fmt::Debug for TestReporter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TestReporter")
            .field("stdout", &"BufferWriter { .. }")
            .field("stderr", &"BufferWriter { .. }")
            .finish()
    }
}

fn status_str(result: ExecutionResult) -> Cow<'static, str> {
    // Max 12 characters here.
    match result {
        #[cfg(unix)]
        ExecutionResult::Fail {
            abort_status: Some(AbortStatus::UnixSignal(sig)),
            leaked: _,
        } => match crate::helpers::signal_str(sig) {
            Some(s) => format!("SIG{s}").into(),
            None => format!("ABORT SIG {sig}").into(),
        },
        #[cfg(windows)]
        ExecutionResult::Fail {
            abort_status: Some(AbortStatus::WindowsNtStatus(_)),
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResult::Fail {
            abort_status: None,
            leaked: true,
        } => "FAIL + LEAK".into(),
        ExecutionResult::Fail {
            abort_status: None,
            leaked: false,
        } => "FAIL".into(),
        ExecutionResult::ExecFail => "XFAIL".into(),
        ExecutionResult::Pass => "PASS".into(),
        ExecutionResult::Leak => "LEAK".into(),
        ExecutionResult::Timeout => "TIMEOUT".into(),
    }
}

fn short_status_str(result: ExecutionResult) -> Cow<'static, str> {
    // Use shorter strings for this (max 6 characters).
    match result {
        #[cfg(unix)]
        ExecutionResult::Fail {
            abort_status: Some(AbortStatus::UnixSignal(sig)),
            leaked: _,
        } => match crate::helpers::signal_str(sig) {
            Some(s) => s.into(),
            None => format!("SIG {sig}").into(),
        },
        #[cfg(windows)]
        ExecutionResult::Fail {
            abort_status: Some(AbortStatus::WindowsNtStatus(_)),
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResult::Fail {
            abort_status: None,
            leaked: _,
        } => "FAIL".into(),
        ExecutionResult::ExecFail => "XFAIL".into(),
        ExecutionResult::Pass => "PASS".into(),
        ExecutionResult::Leak => "LEAK".into(),
        ExecutionResult::Timeout => "TMT".into(),
    }
}

/// A test event.
///
/// Events are produced by a [`TestRunner`](crate::runner::TestRunner) and consumed by a [`TestReporter`].
#[derive(Clone, Debug)]
pub enum TestEvent<'a> {
    /// The test run started.
    RunStarted {
        /// The list of tests that will be run.
        ///
        /// The methods on the test list indicate the number of tests that will be run.
        test_list: &'a TestList<'a>,

        /// The UUID for this run.
        run_id: Uuid,
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
        /// The number of tests still running.
        running: usize,

        /// The reason this run was canceled.
        reason: CancelReason,
    },

    /// A SIGTSTP event was received and the run was paused.
    RunPaused {
        /// The number of tests currently running.
        running: usize,
    },

    /// A SIGCONT event was received and the run is being continued.
    RunContinued {
        /// The number of tests that will be started up again.
        running: usize,
    },

    /// The test run finished.
    RunFinished {
        /// The unique ID for this run.
        run_id: Uuid,

        /// The time at which the run was started.
        start_time: SystemTime,

        /// The amount of time it took for the tests to run.
        elapsed: Duration,

        /// Statistics for the run.
        run_stats: RunStats,
    },
}

// Note: the order here matters -- it indicates severity of cancellation
/// The reason why a test run is being cancelled.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CancelReason {
    /// A test failed and --no-fail-fast wasn't specified.
    TestFailure,

    /// An error occurred while reporting results.
    ReportError,

    /// A termination signal (on Unix, SIGTERM or SIGHUP) was received.
    Signal,

    /// An interrupt (on Unix, Ctrl-C) was received.
    Interrupt,
}

#[derive(Debug, Default)]
struct Styles {
    is_colorized: bool,
    count: Style,
    pass: Style,
    retry: Style,
    fail: Style,
    pass_output: Style,
    retry_output: Style,
    fail_output: Style,
    skip: Style,
    list_styles: crate::list::Styles,
}

impl Styles {
    fn colorize(&mut self) {
        self.is_colorized = true;
        self.count = Style::new().bold();
        self.pass = Style::new().green().bold();
        self.retry = Style::new().magenta().bold();
        self.fail = Style::new().red().bold();
        self.pass_output = Style::new().green();
        self.retry_output = Style::new().magenta();
        self.fail_output = Style::new().magenta();
        self.skip = Style::new().yellow().bold();
        self.list_styles.colorize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::NextestConfig, platform::BuildPlatforms};

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut builder = TestReporterBuilder::default();
        builder
            .set_no_capture(true)
            .set_failure_output(TestOutputDisplay::Immediate)
            .set_success_output(TestOutputDisplay::Immediate)
            .set_status_level(StatusLevel::Fail);
        let test_list = TestList::empty();
        let config = NextestConfig::default_config("/fake/dir");
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let build_platforms = BuildPlatforms::new(None).unwrap();

        let mut buf: Vec<u8> = Vec::new();
        let output = ReporterStderr::Buffer(&mut buf);
        let reporter = builder.build(
            &test_list,
            &profile.apply_build_platforms(&build_platforms),
            output,
        );
        assert!(reporter.inner.no_capture, "no_capture is true");
        assert_eq!(
            reporter.inner.failure_output,
            TestOutputDisplay::Never,
            "failure output is never, overriding other settings"
        );
        assert_eq!(
            reporter.inner.success_output,
            TestOutputDisplay::Never,
            "success output is never, overriding other settings"
        );
        assert_eq!(
            reporter.inner.status_level,
            StatusLevel::Pass,
            "status level is pass, overriding other settings"
        );
    }
}
