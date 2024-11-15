// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{helpers::ByteSubslice, structured::StructuredReporter};
use crate::{
    config::{CompiledDefaultFilter, EvaluatableProfile, ScriptId},
    errors::WriteEventError,
    helpers::{io_write_test_name, plural},
    list::{SkipCounts, TestInstance, TestList},
    reporter::{aggregator::EventAggregator, helpers::highlight_end},
    runner::{
        AbortStatus, ExecuteStatus, ExecutionDescription, ExecutionResult, ExecutionStatuses,
        FinalRunStats, RetryData, RunStats, RunStatsFailureKind, SetupScriptExecuteStatus,
    },
    test_output::{ChildSingleOutput, TestExecutionOutput, TestOutput},
};
use bstr::ByteSlice;
use chrono::{DateTime, FixedOffset};
use debug_ignore::DebugIgnore;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use nextest_metadata::MismatchReason;
use owo_colors::{OwoColorize, Style};
use quick_junit::ReportUuid;
use serde::Deserialize;
use std::{
    borrow::Cow,
    cmp::Reverse,
    fmt, io,
    io::{BufWriter, Write},
    time::Duration,
};
use swrite::{swrite, SWrite};

/// When to display test output in the reporter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
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
        profile: &EvaluatableProfile<'a>,
        output: ReporterStderr<'a>,
        structured_reporter: StructuredReporter<'a>,
    ) -> TestReporter<'a> {
        let styles = Box::default();
        let binary_id_width = test_list
            .iter()
            .filter_map(|test_suite| {
                (test_suite.status.test_count() > 0).then_some(test_suite.binary_id.len())
            })
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
        let force_success_output = match self.no_capture {
            true => Some(TestOutputDisplay::Never),
            false => self.success_output,
        };
        let force_failure_output = match self.no_capture {
            true => Some(TestOutputDisplay::Never),
            false => self.failure_output,
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
                default_filter: profile.default_filter().clone(),
                status_levels: StatusLevels {
                    status_level,
                    final_status_level,
                },
                force_success_output,
                force_failure_output,
                no_capture: self.no_capture,
                binary_id_width,
                styles,
                cancel_status: None,
                final_outputs: DebugIgnore(vec![]),
            },
            stderr,
            structured_reporter,
            metadata_reporter: aggregator,
        }
    }
}

enum ReporterStderrImpl<'a> {
    TerminalWithBar(ProgressBar),
    TerminalWithoutBar,
    Buffer(&'a mut Vec<u8>),
}

impl ReporterStderrImpl<'_> {
    fn finish_and_clear_bar(&self) {
        match self {
            ReporterStderrImpl::TerminalWithBar(bar) => {
                bar.finish_and_clear();
            }
            ReporterStderrImpl::TerminalWithoutBar | ReporterStderrImpl::Buffer(_) => {}
        }
    }
}

/// Functionality to report test results to stderr, JUnit, and/or structured,
/// machine-readable results to stdout
pub struct TestReporter<'a> {
    inner: TestReporterImpl<'a>,
    stderr: ReporterStderrImpl<'a>,
    /// Used to aggregate events for JUnit reports written to disk
    metadata_reporter: EventAggregator<'a>,
    /// Used to emit test events in machine-readable format(s) to stdout
    structured_reporter: StructuredReporter<'a>,
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

    /// Mark the reporter done.
    pub fn finish(&mut self) {
        self.stderr.finish_and_clear_bar();
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
                    .write_event_impl(&event, *buf)
                    .map_err(WriteEventError::Io)?;
            }
        }

        self.structured_reporter.write_event(&event)?;
        self.metadata_reporter.write_event(event)?;
        Ok(())
    }
}

fn update_progress_bar(event: &TestEvent<'_>, styles: &Styles, progress_bar: &ProgressBar) {
    match &event.kind {
        TestEventKind::SetupScriptStarted { no_capture, .. } => {
            // Hide the progress bar if either stderr or stdout are being passed through.
            if *no_capture {
                progress_bar.set_draw_target(ProgressDrawTarget::hidden());
            }
        }
        TestEventKind::SetupScriptFinished { no_capture, .. } => {
            // Restore the progress bar if it was hidden.
            if *no_capture {
                progress_bar.set_draw_target(ProgressDrawTarget::stderr());
            }
        }
        TestEventKind::TestStarted {
            current_stats,
            running,
            cancel_state,
            ..
        }
        | TestEventKind::TestFinished {
            current_stats,
            running,
            cancel_state,
            ..
        } => {
            progress_bar.set_prefix(progress_bar_prefix(current_stats, *cancel_state, styles));
            progress_bar.set_message(progress_bar_msg(current_stats, *running, styles));
            // If there are skipped tests, the initial run count will be lower than when constructed
            // in ProgressBar::new.
            progress_bar.set_length(current_stats.initial_run_count as u64);
            progress_bar.set_position(current_stats.finished_count as u64);
        }
        TestEventKind::RunBeginCancel { .. } => {
            progress_bar.set_prefix(progress_bar_cancel_prefix(styles));
        }
        _ => {}
    }
}

fn progress_bar_cancel_prefix(styles: &Styles) -> String {
    format!("{:>12}", "Canceling".style(styles.fail))
}

fn progress_bar_prefix(
    run_stats: &RunStats,
    cancel_reason: Option<CancelReason>,
    styles: &Styles,
) -> String {
    if cancel_reason.is_some() {
        return progress_bar_cancel_prefix(styles);
    }

    let style = if run_stats.has_failures() {
        styles.fail
    } else {
        styles.pass
    };

    format!("{:>12}", "Running".style(style))
}

fn progress_bar_msg(current_stats: &RunStats, running: usize, styles: &Styles) -> String {
    let mut s = format!("{} running, ", running.style(styles.count));
    write_summary_str(current_stats, styles, &mut s);
    s
}

fn write_summary_str(run_stats: &RunStats, styles: &Styles, out: &mut String) {
    swrite!(
        out,
        "{} {}",
        run_stats.passed.style(styles.count),
        "passed".style(styles.pass)
    );

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
        swrite!(out, " ({})", text.join(", "));
    }
    swrite!(out, ", ");

    if run_stats.failed > 0 {
        swrite!(
            out,
            "{} {}, ",
            run_stats.failed.style(styles.count),
            "failed".style(styles.fail),
        );
    }

    if run_stats.exec_failed > 0 {
        swrite!(
            out,
            "{} {}, ",
            run_stats.exec_failed.style(styles.count),
            "exec failed".style(styles.fail),
        );
    }

    if run_stats.timed_out > 0 {
        swrite!(
            out,
            "{} {}, ",
            run_stats.timed_out.style(styles.count),
            "timed out".style(styles.fail),
        );
    }

    swrite!(
        out,
        "{} {}",
        run_stats.skipped.style(styles.count),
        "skipped".style(styles.skip),
    );
}

#[derive(Debug)]
enum FinalOutput {
    Skipped(#[allow(dead_code)] MismatchReason),
    Executed {
        run_statuses: ExecutionStatuses,
        display_output: bool,
    },
}

impl FinalOutput {
    fn final_status_level(&self) -> FinalStatusLevel {
        match self {
            Self::Skipped(_) => FinalStatusLevel::Skip,
            Self::Executed { run_statuses, .. } => run_statuses.describe().final_status_level(),
        }
    }
}

struct TestReporterImpl<'a> {
    default_filter: CompiledDefaultFilter,
    status_levels: StatusLevels,
    force_success_output: Option<TestOutputDisplay>,
    force_failure_output: Option<TestOutputDisplay>,
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
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        match &event.kind {
            TestEventKind::RunStarted {
                test_list,
                run_id,
                profile_name,
                cli_args: _,
            } => {
                writeln!(writer, "------------")?;
                write!(writer, "{:>12} ", "Nextest run".style(self.styles.pass))?;
                writeln!(
                    writer,
                    "ID {} with nextest profile: {}",
                    run_id.style(self.styles.count),
                    profile_name.style(self.styles.count),
                )?;

                write!(writer, "{:>12} ", "Starting".style(self.styles.pass))?;

                let count_style = self.styles.count;

                let tests_str = plural::tests_str(test_list.run_count());
                let binaries_str = plural::binaries_str(test_list.listed_binary_count());

                write!(
                    writer,
                    "{} {tests_str} across {} {binaries_str}",
                    test_list.run_count().style(count_style),
                    test_list.listed_binary_count().style(count_style),
                )?;

                write_skip_counts(
                    test_list.skip_counts(),
                    &self.default_filter,
                    &self.styles,
                    writer,
                )?;

                writeln!(writer)?;
            }
            TestEventKind::SetupScriptStarted {
                index,
                total,
                script_id,
                command,
                args,
                ..
            } => {
                write!(writer, "{:>12} ", "SETUP".style(self.styles.pass))?;
                // index + 1 so that it displays as e.g. "1/2" and "2/2".
                write!(writer, "[{:>9}] ", format!("{}/{}", index + 1, total))?;

                self.write_setup_script(script_id, command, args, writer)?;
                writeln!(writer)?;
            }
            TestEventKind::SetupScriptSlow {
                script_id,
                command,
                args,
                elapsed,
                will_terminate,
            } => {
                if !*will_terminate && self.status_levels.status_level >= StatusLevel::Slow {
                    write!(writer, "{:>12} ", "SETUP SLOW".style(self.styles.skip))?;
                } else if *will_terminate {
                    write!(writer, "{:>12} ", "TERMINATING".style(self.styles.fail))?;
                }

                self.write_slow_duration(*elapsed, writer)?;
                self.write_setup_script(script_id, command, args, writer)?;
                writeln!(writer)?;
            }
            TestEventKind::SetupScriptFinished {
                script_id,
                index,
                total,
                command,
                args,
                run_status,
                ..
            } => {
                self.write_setup_script_status_line(
                    script_id, *index, *total, command, args, run_status, writer,
                )?;
                // Always display failing setup script output if it exists. We may change this in
                // the future.
                if !run_status.result.is_success() {
                    self.write_setup_script_stdout_stderr(
                        script_id, command, args, run_status, writer,
                    )?;
                }
            }
            TestEventKind::TestStarted { test_instance, .. } => {
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
            TestEventKind::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => {
                if !*will_terminate && self.status_levels.status_level >= StatusLevel::Slow {
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
                    if retry_data.total_attempts > 1
                        && self.status_levels.status_level > required_status_level
                    {
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

            TestEventKind::TestAttemptFailedWillRetry {
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
            } => {
                if self.status_levels.status_level >= StatusLevel::Retry {
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
                    if self.failure_output(*failure_output).is_immediate() {
                        self.write_output(test_instance, run_status, true, writer)?;
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
            TestEventKind::TestRetryStarted {
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
            TestEventKind::TestFinished {
                test_instance,
                success_output,
                failure_output,
                run_statuses,
                ..
            } => {
                let describe = run_statuses.describe();
                let last_status = run_statuses.last_status();
                let test_output_display = match last_status.result.is_success() {
                    true => self.success_output(*success_output),
                    false => self.failure_output(*failure_output),
                };

                let output_on_test_finished = self.status_levels.compute_output_on_test_finished(
                    test_output_display,
                    self.cancel_status,
                    describe.status_level(),
                    describe.final_status_level(),
                );

                if output_on_test_finished.write_status_line {
                    self.write_status_line(*test_instance, describe, writer)?;
                }
                if output_on_test_finished.show_immediate {
                    self.write_output(test_instance, last_status, true, writer)?;
                }
                if let OutputStoreFinal::Yes { display_output } =
                    output_on_test_finished.store_final
                {
                    self.final_outputs.push((
                        *test_instance,
                        FinalOutput::Executed {
                            run_statuses: run_statuses.clone(),
                            display_output,
                        },
                    ));
                }
            }
            TestEventKind::TestSkipped {
                test_instance,
                reason,
            } => {
                if self.status_levels.status_level >= StatusLevel::Skip {
                    self.write_skip_line(*test_instance, writer)?;
                }
                if self.status_levels.final_status_level >= FinalStatusLevel::Skip {
                    self.final_outputs
                        .push((*test_instance, FinalOutput::Skipped(*reason)));
                }
            }
            TestEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } => {
                self.cancel_status = self.cancel_status.max(Some(*reason));

                write!(
                    writer,
                    "{:>12} due to {}",
                    "Canceling".style(self.styles.fail),
                    reason.to_static_str().style(self.styles.fail)
                )?;

                // At the moment, we can have either setup scripts or tests running, but not both.
                if *setup_scripts_running > 0 {
                    let s = plural::setup_scripts_str(*setup_scripts_running);
                    write!(
                        writer,
                        ": {} {s} still running",
                        setup_scripts_running.style(self.styles.count),
                    )?;
                } else if *running > 0 {
                    let tests_str = plural::tests_str(*running);
                    write!(
                        writer,
                        ": {} {tests_str} still running",
                        running.style(self.styles.count),
                    )?;
                }
                writeln!(writer)?;
            }
            TestEventKind::RunPaused {
                setup_scripts_running,
                running,
            } => {
                write!(
                    writer,
                    "{:>12} due to {}",
                    "Pausing".style(self.styles.pass),
                    "signal".style(self.styles.count)
                )?;

                // At the moment, we can have either setup scripts or tests running, but not both.
                if *setup_scripts_running > 0 {
                    let s = plural::setup_scripts_str(*setup_scripts_running);
                    write!(
                        writer,
                        ": {} {s} running",
                        setup_scripts_running.style(self.styles.count),
                    )?;
                } else if *running > 0 {
                    let tests_str = plural::tests_str(*running);
                    write!(
                        writer,
                        ": {} {tests_str} running",
                        running.style(self.styles.count),
                    )?;
                }
                writeln!(writer)?;
            }
            TestEventKind::RunContinued {
                setup_scripts_running,
                running,
            } => {
                write!(
                    writer,
                    "{:>12} due to {}",
                    "Continuing".style(self.styles.pass),
                    "signal".style(self.styles.count)
                )?;

                // At the moment, we can have either setup scripts or tests running, but not both.
                if *setup_scripts_running > 0 {
                    let s = plural::setup_scripts_str(*setup_scripts_running);
                    write!(
                        writer,
                        ": {} {s} running",
                        setup_scripts_running.style(self.styles.count),
                    )?;
                } else if *running > 0 {
                    let tests_str = plural::tests_str(*running);
                    write!(
                        writer,
                        ": {} {tests_str} running",
                        running.style(self.styles.count),
                    )?;
                }
                writeln!(writer)?;
            }
            TestEventKind::RunFinished {
                start_time: _start_time,
                elapsed,
                run_stats,
                ..
            } => {
                let stats_summary = run_stats.summarize_final();
                let summary_style = match stats_summary {
                    FinalRunStats::Success => self.styles.pass,
                    FinalRunStats::NoTestsRun => self.styles.skip,
                    FinalRunStats::Failed(_) | FinalRunStats::Cancelled(_) => self.styles.fail,
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

                // Both initial and finished counts must be 1 for the singular form.
                let tests_str = plural::tests_plural_if(
                    run_stats.initial_run_count != 1 || run_stats.finished_count != 1,
                );

                let mut summary_str = String::new();
                write_summary_str(run_stats, &self.styles, &mut summary_str);
                writeln!(writer, " {tests_str} run: {summary_str}")?;

                // Don't print out test outputs after Ctrl-C, but *do* print them after SIGTERM or
                // SIGHUP since those tend to be automated tasks performing kills.
                if self.cancel_status < Some(CancelReason::Interrupt) {
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
                        match final_output {
                            FinalOutput::Skipped(_) => {
                                self.write_skip_line(*test_instance, writer)?;
                            }
                            FinalOutput::Executed {
                                run_statuses,
                                display_output,
                            } => {
                                let last_status = run_statuses.last_status();

                                self.write_final_status_line(
                                    *test_instance,
                                    run_statuses.describe(),
                                    writer,
                                )?;
                                if *display_output {
                                    self.write_output(test_instance, last_status, false, writer)?;
                                }
                            }
                        }
                    }
                }

                // Print out warnings at the end, if any.
                write_final_warnings(stats_summary, self.cancel_status, &self.styles, writer)?;
            }
        }

        Ok(())
    }

    fn write_skip_line(
        &self,
        test_instance: TestInstance<'a>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
        // same spacing [   0.034s]
        write!(writer, "[         ] ")?;

        self.write_instance(test_instance, writer)?;
        writeln!(writer)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_setup_script_status_line(
        &self,
        script_id: &ScriptId,
        index: usize,
        total: usize,
        command: &str,
        args: &[String],
        status: &SetupScriptExecuteStatus,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        match status.result {
            ExecutionResult::Pass => {
                write!(writer, "{:>12} ", "SETUP PASS".style(self.styles.pass))?;
            }
            ExecutionResult::Leak => {
                write!(writer, "{:>12} ", "SETUP LEAK".style(self.styles.skip))?;
            }
            other => {
                let status_str = short_status_str(other);
                write!(
                    writer,
                    "{:>12} ",
                    format!("SETUP {status_str}").style(self.styles.fail),
                )?;
            }
        }

        write!(writer, "[{:>9}] ", format!("{}/{}", index + 1, total))?;

        self.write_setup_script(script_id, command, args, writer)?;
        writeln!(writer)?;

        Ok(())
    }

    fn write_status_line(
        &self,
        test_instance: TestInstance<'a>,
        describe: ExecutionDescription<'_>,
        writer: &mut dyn Write,
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
        writer: &mut dyn Write,
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

    fn write_instance(&self, instance: TestInstance<'a>, writer: &mut dyn Write) -> io::Result<()> {
        write!(
            writer,
            "{:>width$} ",
            instance
                .suite_info
                .binary_id
                .style(self.styles.list_styles.binary_id),
            width = self.binary_id_width
        )?;

        io_write_test_name(instance.name, &self.styles.list_styles, writer)
    }

    fn write_setup_script(
        &self,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let full_command =
            shell_words::join(std::iter::once(command).chain(args.iter().map(|arg| arg.as_ref())));
        write!(
            writer,
            "{}: {}",
            script_id.style(self.styles.script_id),
            full_command
        )
    }

    fn write_duration(&self, duration: Duration, writer: &mut dyn Write) -> io::Result<()> {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(writer, "[{:>8.3?}s] ", duration.as_secs_f64())
    }

    fn write_duration_by(&self, duration: Duration, writer: &mut dyn Write) -> io::Result<()> {
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(writer, "by {:>7.3?}s ", duration.as_secs_f64())
    }

    fn write_slow_duration(&self, duration: Duration, writer: &mut dyn Write) -> io::Result<()> {
        // Inside the curly braces:
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(writer, "[>{:>7.3?}s] ", duration.as_secs_f64())
    }

    #[cfg(windows)]
    fn write_windows_message_line(
        &self,
        nt_status: windows_sys::Win32::Foundation::NTSTATUS,
        writer: &mut dyn Write,
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

    fn write_setup_script_stdout_stderr(
        &self,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        run_status: &SetupScriptExecuteStatus,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let (header_style, _output_style) = if run_status.result.is_success() {
            (self.styles.pass, self.styles.pass_output)
        } else {
            (self.styles.fail, self.styles.fail_output)
        };

        if let Some(stdout) = &run_status.output.stdout {
            if !stdout.is_empty() {
                write!(writer, "\n{}", "--- ".style(header_style))?;
                write!(writer, "{:21}", "STDOUT:".style(header_style))?;
                self.write_setup_script(script_id, command, args, writer)?;
                writeln!(writer, "{}", " ---".style(header_style))?;

                self.write_test_single_output(stdout, writer)?;
            }
        }

        if let Some(stderr) = &run_status.output.stderr {
            if !stderr.is_empty() {
                write!(writer, "\n{}", "--- ".style(header_style))?;
                write!(writer, "{:21}", "STDERR:".style(header_style))?;
                self.write_setup_script(script_id, command, args, writer)?;
                writeln!(writer, "{}", " ---".style(header_style))?;

                self.write_test_single_output(stderr, writer)?;
            }
        }

        writeln!(writer)
    }

    fn write_output(
        &self,
        test_instance: &TestInstance<'a>,
        run_status: &ExecuteStatus,
        is_retry: bool,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let (header_style, _output_style) = if is_retry {
            (self.styles.retry, self.styles.retry_output)
        } else if run_status.result.is_success() {
            (self.styles.pass, self.styles.pass_output)
        } else {
            (self.styles.fail, self.styles.fail_output)
        };

        match &run_status.output {
            TestExecutionOutput::Output(output) => {
                let description = if self.styles.is_colorized {
                    output.heuristic_extract_description(run_status.result)
                } else {
                    None
                };

                match output {
                    TestOutput::Split(split) => {
                        if let Some(stdout) = &split.stdout {
                            if !stdout.is_empty() {
                                write!(writer, "\n{}", "--- ".style(header_style))?;
                                let out_len =
                                    self.write_attempt(run_status, header_style, writer)?;
                                // The width is to align test instances.
                                write!(
                                    writer,
                                    "{:width$}",
                                    "STDOUT:".style(header_style),
                                    width = (21 - out_len)
                                )?;
                                self.write_instance(*test_instance, writer)?;
                                writeln!(writer, "{}", " ---".style(header_style))?;

                                self.write_test_single_output_with_description(
                                    stdout,
                                    description.and_then(|d| d.stdout_subslice()),
                                    writer,
                                )?;
                            }
                        }

                        if let Some(stderr) = &split.stderr {
                            if !stderr.is_empty() {
                                write!(writer, "\n{}", "--- ".style(header_style))?;
                                let out_len =
                                    self.write_attempt(run_status, header_style, writer)?;
                                // The width is to align test instances.
                                write!(
                                    writer,
                                    "{:width$}",
                                    "STDERR:".style(header_style),
                                    width = (21 - out_len)
                                )?;
                                self.write_instance(*test_instance, writer)?;
                                writeln!(writer, "{}", " ---".style(header_style))?;

                                self.write_test_single_output_with_description(
                                    stderr,
                                    description.and_then(|d| d.stderr_subslice()),
                                    writer,
                                )?;
                            }
                        }
                    }
                    TestOutput::Combined { output } => {
                        if !output.is_empty() {
                            write!(writer, "\n{}", "--- ".style(header_style))?;
                            let out_len = self.write_attempt(run_status, header_style, writer)?;
                            // The width is to align test instances.
                            write!(
                                writer,
                                "{:width$}",
                                "OUTPUT:".style(header_style),
                                width = (21 - out_len)
                            )?;
                            self.write_instance(*test_instance, writer)?;
                            writeln!(writer, "{}", " ---".style(header_style))?;

                            self.write_test_single_output_with_description(
                                output,
                                description.and_then(|d| d.combined_subslice()),
                                writer,
                            )?;
                        }
                    }
                }
            }

            TestExecutionOutput::ExecFail { description, .. } => {
                write!(writer, "\n{}", "--- ".style(header_style))?;
                let out_len = self.write_attempt(run_status, header_style, writer)?;
                // The width is to align test instances.
                write!(
                    writer,
                    "{:width$}",
                    "EXECFAIL:".style(header_style),
                    width = (21 - out_len)
                )?;
                self.write_instance(*test_instance, writer)?;
                writeln!(writer, "{}", " ---".style(header_style))?;

                writeln!(writer, "{description}")?;
            }
        }

        writeln!(writer)
    }

    /// Writes a test output to the writer.
    fn write_test_single_output(
        &self,
        output: &ChildSingleOutput,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        // SAFETY: The description is not provided.
        self.write_test_single_output_with_description(output, None, writer)
    }

    /// Writes a test output to the writer, along with optionally a subslice of the output to
    /// highlight.
    ///
    /// The description must be a subslice of the output.
    fn write_test_single_output_with_description(
        &self,
        output: &ChildSingleOutput,
        description: Option<ByteSubslice<'_>>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        if self.styles.is_colorized {
            if let Some(subslice) = description {
                write_output_with_highlight(&output.buf, subslice, &self.styles.fail, writer)?;
            } else {
                // Output the text without stripping ANSI escapes, then reset the color afterwards
                // in case the output is malformed.
                writer.write_all(&output.buf)?;
                writer.write_all(RESET_COLOR)?;
            }
        } else {
            // Strip ANSI escapes from the output if nextest itself isn't colorized.
            let mut no_color = strip_ansi_escapes::Writer::new(writer);
            no_color.write_all(&output.buf)?;
        }

        Ok(())
    }

    // Returns the number of characters written out to the screen.
    fn write_attempt(
        &self,
        run_status: &ExecuteStatus,
        style: Style,
        writer: &mut dyn Write,
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

    fn success_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_success_output.unwrap_or(test_setting)
    }

    fn failure_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_failure_output.unwrap_or(test_setting)
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

const RESET_COLOR: &[u8] = b"\x1b[0m";

fn write_output_with_highlight(
    output: &[u8],
    ByteSubslice { slice, start }: ByteSubslice,
    highlight_style: &Style,
    mut writer: &mut dyn Write,
) -> io::Result<()> {
    let end = start + highlight_end(slice);

    // Output the start and end of the test without stripping ANSI escapes, then reset
    // the color afterwards in case the output is malformed.
    writer.write_all(&output[..start])?;
    writer.write_all(RESET_COLOR)?;

    // Some systems (e.g. GitHub Actions, Buildomat) don't handle multiline ANSI
    // coloring -- they reset colors after each line. To work around that,
    // we reset and re-apply colors for each line.
    for line in output[start..end].lines_with_terminator() {
        write!(writer, "{}", FmtPrefix(highlight_style))?;

        // Write everything before the newline, stripping ANSI escapes.
        let mut no_color = strip_ansi_escapes::Writer::new(writer);
        let trimmed = line.trim_end_with(|c| c == '\n' || c == '\r');
        no_color.write_all(trimmed.as_bytes())?;
        writer = no_color.into_inner()?;

        // End coloring.
        write!(writer, "{}", FmtSuffix(highlight_style))?;

        // Now write the newline, if present.
        writer.write_all(&line[trimmed.len()..])?;
    }

    // `end` is guaranteed to be within the bounds of `output.buf`. (It is actually safe
    // for it to be equal to `output.buf.len()` -- it gets treated as an empty list in
    // that case.)
    writer.write_all(&output[end..])?;
    writer.write_all(RESET_COLOR)?;

    Ok(())
}

struct FmtPrefix<'a>(&'a Style);

impl fmt::Display for FmtPrefix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt_prefix(f)
    }
}

struct FmtSuffix<'a>(&'a Style);

impl fmt::Display for FmtSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt_suffix(f)
    }
}

fn write_skip_counts(
    skip_counts: &SkipCounts,
    default_filter: &CompiledDefaultFilter,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    if skip_counts.skipped_tests > 0 || skip_counts.skipped_binaries > 0 {
        write!(writer, " (")?;
        write_skip_counts_impl(
            skip_counts.skipped_tests,
            skip_counts.skipped_binaries,
            styles,
            writer,
        )?;

        // Were all tests and binaries that were skipped, skipped due to being in the
        // default set?
        if skip_counts.skipped_tests == skip_counts.skipped_tests_default_filter
            && skip_counts.skipped_binaries == skip_counts.skipped_binaries_default_filter
        {
            write!(
                writer,
                " {} via {}",
                "skipped".style(styles.skip),
                default_filter.display_config(styles.count)
            )?;
        } else {
            write!(writer, " {}", "skipped".style(styles.skip))?;
            // Were *any* tests in the default set?
            if skip_counts.skipped_binaries_default_filter > 0
                || skip_counts.skipped_tests_default_filter > 0
            {
                write!(writer, ", including ")?;
                write_skip_counts_impl(
                    skip_counts.skipped_tests_default_filter,
                    skip_counts.skipped_binaries_default_filter,
                    styles,
                    writer,
                )?;
                write!(
                    writer,
                    " via {}",
                    default_filter.display_config(styles.count)
                )?;
            }
        }
        write!(writer, ")")?;
    }

    Ok(())
}

fn write_skip_counts_impl(
    skipped_tests: usize,
    skipped_binaries: usize,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    // X tests and Y binaries skipped, or X tests skipped, or Y binaries skipped.
    if skipped_tests > 0 && skipped_binaries > 0 {
        write!(
            writer,
            "{} {} and {} {}",
            skipped_tests.style(styles.count),
            plural::tests_str(skipped_tests),
            skipped_binaries.style(styles.count),
            plural::binaries_str(skipped_binaries),
        )?;
    } else if skipped_tests > 0 {
        write!(
            writer,
            "{} {}",
            skipped_tests.style(styles.count),
            plural::tests_str(skipped_tests),
        )?;
    } else if skipped_binaries > 0 {
        write!(
            writer,
            "{} {}",
            skipped_binaries.style(styles.count),
            plural::binaries_str(skipped_binaries),
        )?;
    }

    Ok(())
}

struct StatusLevels {
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
}

impl StatusLevels {
    fn compute_output_on_test_finished(
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
struct OutputOnTestFinished {
    write_status_line: bool,
    show_immediate: bool,
    store_final: OutputStoreFinal,
}

#[derive(Debug, PartialEq, Eq)]
enum OutputStoreFinal {
    /// Do not store the output.
    No,

    /// Store the output. display_output controls whether stdout and stderr should actually be
    /// displayed at the end.
    Yes { display_output: bool },
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

fn write_final_warnings(
    final_stats: FinalRunStats,
    cancel_status: Option<CancelReason>,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    match final_stats {
        FinalRunStats::Failed(RunStatsFailureKind::Test {
            initial_run_count,
            not_run,
        })
        | FinalRunStats::Cancelled(RunStatsFailureKind::Test {
            initial_run_count,
            not_run,
        }) if not_run > 0 => {
            if cancel_status == Some(CancelReason::TestFailure) {
                writeln!(
                    writer,
                    "{}: {}/{} {} {} not run due to {} (run with {} to run all tests)",
                    "warning".style(styles.skip),
                    not_run.style(styles.count),
                    initial_run_count.style(styles.count),
                    plural::tests_plural_if(initial_run_count != 1 || not_run != 1),
                    plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                    CancelReason::TestFailure.to_static_str().style(styles.skip),
                    "--no-fail-fast".style(styles.count),
                )?;
            } else {
                let due_to_reason = match cancel_status {
                    Some(reason) => {
                        format!(" due to {}", reason.to_static_str().style(styles.skip))
                    }
                    None => "".to_string(),
                };
                writeln!(
                    writer,
                    "{}: {}/{} {} {} not run{}",
                    "warning".style(styles.skip),
                    not_run.style(styles.count),
                    initial_run_count.style(styles.count),
                    plural::tests_plural_if(initial_run_count != 1 || not_run != 1),
                    plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                    due_to_reason,
                )?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// A test event.
///
/// Events are produced by a [`TestRunner`](crate::runner::TestRunner) and consumed by a
/// [`TestReporter`].
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
    script_id: Style,
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
        self.script_id = Style::new().blue().bold();
        self.list_styles.colorize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{CompiledDefaultFilterSection, NextestConfig},
        platform::BuildPlatforms,
        reporter::structured::StructuredReporter,
    };
    use nextest_filtering::CompiledExpr;
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

    // ---

    #[test]
    fn test_write_skip_counts() {
        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, false), @" (1 binary skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 2,
        }, true), @" (2 binaries skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 2,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 1,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 2,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 3,
        }, false), @" (2 tests and 3 binaries skipped via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (1 test and 1 binary skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 2,
            skipped_tests_default_filter: 0,
            skipped_binaries: 3,
            skipped_binaries_default_filter: 0,
        }, false), @" (2 tests and 3 binaries skipped)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 1,
            skipped_tests_default_filter: 0,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 1,
        }, true), @" (1 test and 1 binary skipped, including 1 binary via default-filter in profile.my-profile.overrides)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 3,
            skipped_tests_default_filter: 2,
            skipped_binaries: 1,
            skipped_binaries_default_filter: 0,
        }, false), @" (3 tests and 1 binary skipped, including 2 tests via profile.my-profile.default-filter)");

        insta::assert_snapshot!(skip_counts_str(&SkipCounts {
            skipped_tests: 0,
            skipped_tests_default_filter: 0,
            skipped_binaries: 0,
            skipped_binaries_default_filter: 0,
        }, false), @"");
    }

    fn skip_counts_str(skip_counts: &SkipCounts, override_section: bool) -> String {
        let mut buf = Vec::new();
        write_skip_counts(
            skip_counts,
            &CompiledDefaultFilter {
                expr: CompiledExpr::ALL,
                profile: "my-profile".to_owned(),
                section: if override_section {
                    CompiledDefaultFilterSection::Override(0)
                } else {
                    CompiledDefaultFilterSection::Profile
                },
            },
            &Styles::default(),
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_write_output_with_highlight() {
        const RESET_COLOR: &str = "\u{1b}[0m";
        const BOLD_RED: &str = "\u{1b}[31;1m";

        assert_eq!(
            write_output_with_highlight_buf("output", 0, Some(6)),
            format!("{RESET_COLOR}{BOLD_RED}output{RESET_COLOR}{RESET_COLOR}")
        );

        assert_eq!(
            write_output_with_highlight_buf("output", 1, Some(5)),
            format!("o{RESET_COLOR}{BOLD_RED}utpu{RESET_COLOR}t{RESET_COLOR}")
        );

        assert_eq!(
            write_output_with_highlight_buf("output\nhighlight 1\nhighlight 2", 7, None),
            format!(
                "output\n{RESET_COLOR}\
                {BOLD_RED}highlight 1{RESET_COLOR}\n\
                {BOLD_RED}highlight 2{RESET_COLOR}{RESET_COLOR}"
            )
        );

        assert_eq!(
            write_output_with_highlight_buf(
                "output\nhighlight 1\nhighlight 2\nnot highlighted",
                7,
                None
            ),
            format!(
                "output\n{RESET_COLOR}\
                {BOLD_RED}highlight 1{RESET_COLOR}\n\
                {BOLD_RED}highlight 2{RESET_COLOR}\n\
                not highlighted{RESET_COLOR}"
            )
        );
    }

    fn write_output_with_highlight_buf(output: &str, start: usize, end: Option<usize>) -> String {
        // We're not really testing non-UTF-8 output here, and using strings results in much more
        // readable error messages.
        let mut buf = Vec::new();
        let end = end.unwrap_or(output.len());

        let subslice = ByteSubslice {
            start,
            slice: &output.as_bytes()[start..end],
        };
        write_output_with_highlight(
            output.as_bytes(),
            subslice,
            &Style::new().red().bold(),
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

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
        let build_platforms = BuildPlatforms::new_with_no_target().unwrap();

        let mut buf: Vec<u8> = Vec::new();
        let output = ReporterStderr::Buffer(&mut buf);
        let reporter = builder.build(
            &test_list,
            &profile.apply_build_platforms(&build_platforms),
            output,
            StructuredReporter::new(),
        );
        assert!(reporter.inner.no_capture, "no_capture is true");
        assert_eq!(
            reporter.inner.force_failure_output,
            Some(TestOutputDisplay::Never),
            "failure output is never, overriding other settings"
        );
        assert_eq!(
            reporter.inner.force_success_output,
            Some(TestOutputDisplay::Never),
            "success output is never, overriding other settings"
        );
        assert_eq!(
            reporter.inner.status_levels.status_level,
            StatusLevel::Pass,
            "status level is pass, overriding other settings"
        );
    }

    #[test]
    fn test_progress_bar_prefix() {
        let mut styles = Styles::default();
        styles.colorize();

        for stats in run_stats_test_failure_examples() {
            let prefix = progress_bar_prefix(&stats, Some(CancelReason::TestFailure), &styles);
            assert_eq!(prefix, "   Canceling".style(styles.fail).to_string());
        }
        for stats in run_stats_setup_script_failure_examples() {
            let prefix =
                progress_bar_prefix(&stats, Some(CancelReason::SetupScriptFailure), &styles);
            assert_eq!(prefix, "   Canceling".style(styles.fail).to_string());
        }

        let prefix = progress_bar_prefix(&RunStats::default(), Some(CancelReason::Signal), &styles);
        assert_eq!(prefix, "   Canceling".style(styles.fail).to_string());

        let prefix = progress_bar_prefix(&RunStats::default(), None, &styles);
        assert_eq!(prefix, "     Running".style(styles.pass).to_string());

        for stats in run_stats_test_failure_examples() {
            let prefix = progress_bar_prefix(&stats, None, &styles);
            assert_eq!(prefix, "     Running".style(styles.fail).to_string());
        }
        for stats in run_stats_setup_script_failure_examples() {
            let prefix = progress_bar_prefix(&stats, None, &styles);
            assert_eq!(prefix, "     Running".style(styles.fail).to_string());
        }
    }

    fn run_stats_test_failure_examples() -> Vec<RunStats> {
        vec![
            RunStats {
                failed: 1,
                ..RunStats::default()
            },
            RunStats {
                failed: 1,
                passed: 1,
                ..RunStats::default()
            },
            RunStats {
                exec_failed: 1,
                ..RunStats::default()
            },
            RunStats {
                timed_out: 1,
                ..RunStats::default()
            },
        ]
    }

    fn run_stats_setup_script_failure_examples() -> Vec<RunStats> {
        vec![
            RunStats {
                setup_scripts_failed: 1,
                ..RunStats::default()
            },
            RunStats {
                setup_scripts_exec_failed: 1,
                ..RunStats::default()
            },
            RunStats {
                setup_scripts_timed_out: 1,
                ..RunStats::default()
            },
        ]
    }

    #[test]
    fn test_final_warnings() {
        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 3,
                not_run: 1,
            }),
            Some(CancelReason::TestFailure),
        );
        assert_eq!(
            warnings,
            "warning: 1/3 tests were not run due to test failure \
             (run with --no-fail-fast to run all tests)\n"
        );

        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::Test {
                initial_run_count: 8,
                not_run: 5,
            }),
            Some(CancelReason::Signal),
        );
        assert_eq!(warnings, "warning: 5/8 tests were not run due to signal\n");

        let warnings = final_warnings_for(
            FinalRunStats::Cancelled(RunStatsFailureKind::Test {
                initial_run_count: 1,
                not_run: 1,
            }),
            Some(CancelReason::Interrupt),
        );
        assert_eq!(warnings, "warning: 1/1 test was not run due to interrupt\n");

        // These warnings are taken care of by cargo-nextest.
        let warnings = final_warnings_for(FinalRunStats::NoTestsRun, None);
        assert_eq!(warnings, "");
        let warnings = final_warnings_for(FinalRunStats::NoTestsRun, Some(CancelReason::Signal));
        assert_eq!(warnings, "");

        // No warnings for success.
        let warnings = final_warnings_for(FinalRunStats::Success, None);
        assert_eq!(warnings, "");

        // No warnings for setup script failure.
        let warnings = final_warnings_for(
            FinalRunStats::Failed(RunStatsFailureKind::SetupScript),
            Some(CancelReason::SetupScriptFailure),
        );
        assert_eq!(warnings, "");

        // No warnings for setup script cancellation.
        let warnings = final_warnings_for(
            FinalRunStats::Cancelled(RunStatsFailureKind::SetupScript),
            Some(CancelReason::Interrupt),
        );
        assert_eq!(warnings, "");
    }

    fn final_warnings_for(stats: FinalRunStats, cancel_status: Option<CancelReason>) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let styles = Styles::default();
        write_final_warnings(stats, cancel_status, &styles, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }
}
