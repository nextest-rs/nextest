// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{
    structured::StructuredReporter, ByteSubslice, CancelReason, InfoResponse,
    SetupScriptInfoResponse, TestEvent, TestEventKind, TestInfoResponse, TestOutputErrorSlice,
    UnitKind, UnitState, UnitTerminatingState,
};
use crate::{
    config::{CompiledDefaultFilter, EvaluatableProfile, ScriptId},
    errors::{DisplayErrorChain, WriteEventError},
    helpers::{plural, DisplayScriptInstance, DisplayTestInstance},
    list::{SkipCounts, TestInstance, TestInstanceId, TestList},
    reporter::{
        aggregator::EventAggregator, helpers::highlight_end, UnitErrorDescription,
        UnitTerminateMethod,
    },
    runner::{
        AbortStatus, ExecuteStatus, ExecutionDescription, ExecutionResult, ExecutionStatuses,
        FinalRunStats, RetryData, RunStats, RunStatsFailureKind, SetupScriptExecuteStatus,
    },
    test_output::{ChildExecutionOutput, ChildOutput, ChildSingleOutput},
};
use bstr::ByteSlice;
use debug_ignore::DebugIgnore;
use indent_write::io::IndentWriter;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use nextest_metadata::MismatchReason;
use owo_colors::{OwoColorize, Style};
use serde::Deserialize;
use std::{
    borrow::Cow,
    cmp::Reverse,
    fmt,
    io::{self, BufWriter, Write},
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
    should_colorize: bool,
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
        let mut styles: Box<Styles> = Box::default();
        if self.should_colorize {
            styles.colorize();
        }

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

        let mut theme_characters = ThemeCharacters::default();
        match output {
            ReporterStderr::Terminal => {
                if supports_unicode::on(supports_unicode::Stream::Stderr) {
                    theme_characters.use_unicode();
                }
            }
            ReporterStderr::Buffer(_) => {
                // Always use Unicode for internal buffers.
                theme_characters.use_unicode();
            }
        }

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
                let bar = ProgressBar::new(test_list.test_count() as u64);
                // Emulate Cargo's style.
                let test_count_width = format!("{}", test_list.test_count()).len();
                // Create the template using the width as input. This is a little confusing -- {{foo}}
                // is what's passed into the ProgressBar, while {bar} is inserted by the format!() statement.
                //
                // Note: ideally we'd use the same format as our other duration displays for the elapsed time,
                // but that isn't possible due to https://github.com/console-rs/indicatif/issues/440. Use
                // {{elapsed_precise}} as an OK tradeoff here.
                let template = format!(
                    "{{prefix:>12}} [{{elapsed_precise:>9}}] {{wide_bar}} \
                    {{pos:>{test_count_width}}}/{{len:{test_count_width}}}: {{msg}}     "
                );
                bar.set_style(
                    ProgressStyle::default_bar()
                        .progress_chars(theme_characters.progress_chars)
                        .template(&template)
                        .expect("template is known to be valid"),
                );
                // NOTE: set_draw_target must be called before enable_steady_tick to avoid a
                // spurious extra line from being printed as the draw target changes.
                //
                // This used to be unbuffered, but that option went away from indicatif 0.17.0. The
                // refresh rate is now 20hz so that it's double the steady tick rate.
                bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
                // Enable a steady tick 10 times a second.
                bar.enable_steady_tick(Duration::from_millis(100));
                ReporterStderrImpl::TerminalWithBar {
                    bar,
                    state: ProgressBarState::new(),
                }
            }
            ReporterStderr::Buffer(buf) => ReporterStderrImpl::Buffer(buf),
        };

        // Ordinarily, empty stdout and stderr are not displayed. This
        // environment variable is set in integration tests to ensure that they
        // are.
        let display_empty_outputs =
            std::env::var_os("__NEXTEST_DISPLAY_EMPTY_OUTPUTS").map_or(false, |v| v == "1");

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
                styles,
                theme_characters,
                cancel_status: None,
                final_outputs: DebugIgnore(vec![]),
                display_empty_outputs,
            },
            stderr,
            structured_reporter,
            metadata_reporter: aggregator,
        }
    }
}

enum ReporterStderrImpl<'a> {
    TerminalWithBar {
        bar: ProgressBar,
        // Reporter-specific progress bar state.
        state: ProgressBarState,
    },
    TerminalWithoutBar,
    Buffer(&'a mut Vec<u8>),
}

impl ReporterStderrImpl<'_> {
    fn finish_and_clear_bar(&self) {
        match self {
            ReporterStderrImpl::TerminalWithBar { bar, .. } => {
                bar.finish_and_clear();
            }
            ReporterStderrImpl::TerminalWithoutBar | ReporterStderrImpl::Buffer(_) => {}
        }
    }
}

#[derive(Debug)]
struct ProgressBarState {
    // Reasons for hiding the progress bar. We show the progress bar if none of
    // these are set and hide it if any of them are set.
    //
    // indicatif cannot handle this kind of "stacked" state management so it
    // falls on us to do so.
    hidden_no_capture: bool,
    hidden_run_paused: bool,
    hidden_info_response: bool,
}

impl ProgressBarState {
    fn new() -> Self {
        Self {
            hidden_no_capture: false,
            hidden_run_paused: false,
            hidden_info_response: false,
        }
    }

    fn should_hide(&self) -> bool {
        self.hidden_no_capture || self.hidden_run_paused || self.hidden_info_response
    }

    fn update_progress_bar(
        &mut self,
        event: &TestEvent<'_>,
        styles: &Styles,
        progress_bar: &ProgressBar,
    ) {
        let before_should_hide = self.should_hide();

        match &event.kind {
            TestEventKind::SetupScriptStarted { no_capture, .. } => {
                // Hide the progress bar if either stderr or stdout are being passed through.
                if *no_capture {
                    self.hidden_no_capture = true;
                }
            }
            TestEventKind::SetupScriptFinished { no_capture, .. } => {
                // Restore the progress bar if it was hidden.
                if *no_capture {
                    self.hidden_no_capture = false;
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
            TestEventKind::InfoStarted { .. } => {
                // While info is being displayed, hide the progress bar to avoid
                // it interrupting the info display.
                self.hidden_info_response = true;
            }
            TestEventKind::InfoFinished { .. } => {
                // Restore the progress bar if it was hidden.
                self.hidden_info_response = false;
            }
            TestEventKind::RunPaused { .. } => {
                // Pausing the run should hide the progress bar since we'll exit
                // to the terminal immediately after.
                self.hidden_run_paused = true;
            }
            TestEventKind::RunContinued { .. } => {
                // Continuing the run should show the progress bar since we'll
                // continue to output to it.
                self.hidden_run_paused = false;
            }
            TestEventKind::RunBeginCancel { .. } => {
                progress_bar.set_prefix(progress_bar_cancel_prefix(styles));
            }
            _ => {}
        }

        let after_should_hide = self.should_hide();

        match (before_should_hide, after_should_hide) {
            (false, true) => progress_bar.set_draw_target(ProgressDrawTarget::hidden()),
            (true, false) => progress_bar.set_draw_target(ProgressDrawTarget::stderr()),
            _ => {}
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
            ReporterStderrImpl::TerminalWithBar { bar, state } => {
                // Write to a string that will be printed as a log line.
                let mut buf: Vec<u8> = Vec::new();
                self.inner
                    .write_event_impl(&event, &mut buf)
                    .map_err(WriteEventError::Io)?;

                state.update_progress_bar(&event, &self.inner.styles, bar);
                // ProgressBar::println doesn't print status lines if the bar is
                // hidden. The suspend method prints it in all cases.
                bar.suspend(|| {
                    _ = std::io::stderr().write_all(&buf);
                });
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

fn progress_bar_cancel_prefix(styles: &Styles) -> String {
    format!("{:>12}", "Cancelling".style(styles.fail))
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
    Skipped(#[expect(dead_code)] MismatchReason),
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
    styles: Box<Styles>,
    theme_characters: ThemeCharacters,
    cancel_status: Option<CancelReason>,
    final_outputs: DebugIgnore<Vec<(TestInstance<'a>, FinalOutput)>>,
    display_empty_outputs: bool,
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
                writeln!(writer, "{}", self.theme_characters.hbar(12))?;
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
                writeln!(
                    writer,
                    "{:>12} [{:>9}] {}",
                    "SETUP".style(self.styles.pass),
                    // index + 1 so that it displays as e.g. "1/2" and "2/2".
                    format!("{}/{}", index + 1, total),
                    self.display_script_instance(script_id.clone(), command, args)
                )?;
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
                writeln!(
                    writer,
                    "{}",
                    self.display_script_instance(script_id.clone(), command, args)
                )?;
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
                    self.write_setup_script_execute_status(
                        script_id, command, args, run_status, writer,
                    )?;
                }
            }
            TestEventKind::TestStarted { test_instance, .. } => {
                // In no-capture mode, print out a test start event.
                if self.no_capture {
                    // The spacing is to align test instances.
                    writeln!(
                        writer,
                        "{:>12}             {}",
                        "START".style(self.styles.pass),
                        self.display_test_instance(test_instance.id()),
                    )?;
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
                writeln!(writer, "{}", self.display_test_instance(test_instance.id()))?;
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
                    writeln!(writer, "{}", self.display_test_instance(test_instance.id()))?;

                    // This test is guaranteed to have failed.
                    assert!(
                        !run_status.result.is_success(),
                        "only failing tests are retried"
                    );
                    if self.failure_output(*failure_output).is_immediate() {
                        self.write_test_execute_status(test_instance, run_status, true, writer)?;
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
                        writeln!(writer, "{}", self.display_test_instance(test_instance.id()))?;
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

                // Add spacing to align test instances, then print the name of the test.
                writeln!(
                    writer,
                    "[{:<9}] {}",
                    "",
                    self.display_test_instance(test_instance.id())
                )?;
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
                    self.write_test_execute_status(test_instance, last_status, false, writer)?;
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
                    self.write_skip_line(test_instance.id(), writer)?;
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
                    "Cancelling".style(self.styles.fail),
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
            TestEventKind::InfoStarted { total, run_stats } => {
                let info_style = if run_stats.has_failures() {
                    self.styles.fail
                } else {
                    self.styles.pass
                };

                let hbar = self.theme_characters.hbar(12);

                write!(writer, "{hbar}\n{}: ", "info".style(info_style))?;

                // TODO: display setup_scripts_running as well
                writeln!(
                    writer,
                    "{} in {:.3?}s",
                    // Using "total" here for the number of running units is a
                    // slight fudge, but it prevents situations where (due to
                    // races with unit tasks exiting) the numbers don't exactly
                    // match up. It's also not dishonest -- there really are
                    // these many units currently running.
                    progress_bar_msg(run_stats, *total, &self.styles),
                    event.elapsed.as_secs_f64(),
                )?;
            }
            TestEventKind::InfoResponse {
                index,
                total,
                response,
            } => {
                self.write_info_response(*index, *total, response, writer)?;
            }
            TestEventKind::InfoFinished { missing } => {
                let hbar = self.theme_characters.hbar(12);

                if *missing > 0 {
                    // This should ordinarily not happen, but it's possible if
                    // some of the unit futures are slow to respond.
                    writeln!(
                        writer,
                        "{}: missing {} responses",
                        "info".style(self.styles.skip),
                        missing.style(self.styles.count)
                    )?;
                }

                writeln!(writer, "{hbar}")?;
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
                    "{}\n{:>12} ",
                    self.theme_characters.hbar(12),
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
                            // Use the final status level, reversed (i.e.
                            // failing tests are printed at the very end).
                            (
                                Reverse(final_output.final_status_level()),
                                test_instance.id(),
                            )
                        });

                    for (test_instance, final_output) in &*self.final_outputs {
                        match final_output {
                            FinalOutput::Skipped(_) => {
                                self.write_skip_line(test_instance.id(), writer)?;
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
                                    self.write_test_execute_status(
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

                // Print out warnings at the end, if any.
                write_final_warnings(stats_summary, self.cancel_status, &self.styles, writer)?;
            }
        }

        Ok(())
    }

    fn write_skip_line(
        &self,
        test_instance: TestInstanceId<'a>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
        // same spacing   [   0.034s]
        writeln!(
            writer,
            "[         ] {}",
            self.display_test_instance(test_instance)
        )?;

        Ok(())
    }

    #[expect(clippy::too_many_arguments)]
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

        writeln!(
            writer,
            "[{:>9}] {}",
            format!("{}/{}", index + 1, total),
            self.display_script_instance(script_id.clone(), command, args)
        )?;

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
        writeln!(writer, "{}", self.display_test_instance(test_instance.id()))?;

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
        writeln!(writer, "{}", self.display_test_instance(test_instance.id()))?;

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

    fn display_test_instance(&self, instance: TestInstanceId<'a>) -> DisplayTestInstance<'_> {
        DisplayTestInstance::new(instance, &self.styles.list_styles)
    }

    fn display_script_instance(
        &self,
        script_id: ScriptId,
        command: &str,
        args: &[String],
    ) -> DisplayScriptInstance {
        DisplayScriptInstance::new(script_id, command, args, self.styles.script_id)
    }

    fn write_info_response(
        &self,
        index: usize,
        total: usize,
        response: &InfoResponse<'_>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        if index > 0 {
            // Show a shorter hbar than the hbar surrounding the info started
            // and finished lines.
            writeln!(writer, "{}", self.theme_characters.hbar(8))?;
        }

        // "status: " is 8 characters. Pad "{}/{}:" such that it also gets to
        // the 8 characters.
        //
        // The width to be printed out is index width + total width + 1 for '/'
        // + 1 for ':' + 1 for the space after that.
        let count_width = decimal_char_width(index + 1) + decimal_char_width(total) + 3;
        let padding = usize::try_from(8_u32.saturating_sub(count_width)).unwrap();

        write!(
            writer,
            "\n* {}/{}: {:padding$}",
            // index is 0-based, so add 1 to make it 1-based.
            (index + 1).style(self.styles.count),
            total.style(self.styles.count),
            "",
        )?;

        // Indent everything a bit to make it clear that this is a
        // response.
        let mut writer = IndentWriter::new_skip_initial("  ", writer);

        match response {
            InfoResponse::SetupScript(SetupScriptInfoResponse {
                script_id,
                command,
                args,
                state,
                output,
            }) => {
                // Write the setup script name.
                writeln!(
                    writer,
                    "{}",
                    self.display_script_instance(script_id.clone(), command, args)
                )?;

                // Write the state of the script.
                self.write_unit_state(
                    UnitKind::Script,
                    "",
                    state,
                    output.has_errors(),
                    &mut writer,
                )?;

                // Write the output of the script.
                if state.has_valid_output() {
                    self.write_child_execution_output(
                        &self.output_spec_for_info(UnitKind::Script),
                        output,
                        &mut writer,
                    )?;
                }
            }
            InfoResponse::Test(TestInfoResponse {
                test_instance,
                retry_data,
                state,
                output,
            }) => {
                // Write the test name.
                writeln!(writer, "{}", self.display_test_instance(*test_instance))?;

                // We want to show an attached attempt string either if this is
                // a DelayBeforeNextAttempt message or if this is a retry. (This
                // is a bit abstraction-breaking, but what good UI isn't?)
                let show_attempt_str = (retry_data.attempt > 1 && retry_data.total_attempts > 1)
                    || matches!(state, UnitState::DelayBeforeNextAttempt { .. });
                let attempt_str = if show_attempt_str {
                    format!(
                        "(attempt {}/{}) ",
                        retry_data.attempt, retry_data.total_attempts
                    )
                } else {
                    String::new()
                };

                // Write the state of the test.
                self.write_unit_state(
                    UnitKind::Test,
                    &attempt_str,
                    state,
                    output.has_errors(),
                    &mut writer,
                )?;

                // Write the output of the test.
                if state.has_valid_output() {
                    self.write_child_execution_output(
                        &self.output_spec_for_info(UnitKind::Test),
                        output,
                        &mut writer,
                    )?;
                }
            }
        }

        writer.flush()?;
        let inner_writer = writer.into_inner();

        // Add a newline at the end to visually separate the responses.
        writeln!(inner_writer)?;

        Ok(())
    }

    fn write_unit_state(
        &self,
        kind: UnitKind,
        attempt_str: &str,
        state: &UnitState,
        output_has_errors: bool,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let status_str = "status".style(self.styles.count);
        match state {
            UnitState::Running {
                pid,
                time_taken,
                slow_after,
            } => {
                let running_style = if output_has_errors {
                    self.styles.fail
                } else if slow_after.is_some() {
                    self.styles.skip
                } else {
                    self.styles.pass
                };
                write!(
                    writer,
                    "{status_str}: {attempt_str}{kind} {} for {:.3?}s as PID {}",
                    "running".style(running_style),
                    time_taken.as_secs_f64(),
                    pid.style(self.styles.count),
                )?;
                if let Some(slow_after) = slow_after {
                    write!(
                        writer,
                        " (marked slow after {:.3?}s)",
                        slow_after.as_secs_f64()
                    )?;
                }
                writeln!(writer)?;
            }
            UnitState::Exiting {
                pid,
                time_taken,
                slow_after,
                tentative_result,
                waiting_duration,
                remaining,
            } => {
                write!(writer, "{status_str}: {attempt_str}{kind} ")?;

                self.write_info_execution_result(*tentative_result, slow_after.is_some(), writer)?;
                write!(writer, " after {:.3?}s", time_taken.as_secs_f64())?;
                if let Some(slow_after) = slow_after {
                    write!(
                        writer,
                        " (marked slow after {:.3?}s)",
                        slow_after.as_secs_f64()
                    )?;
                }
                writeln!(writer)?;

                // Don't need to print the waiting duration for leak detection
                // if it's relatively small.
                if *waiting_duration >= Duration::from_secs(1) {
                    writeln!(
                        writer,
                        "{}:   spent {:.3?}s waiting for {kind} PID {} to shut down, \
                         will mark as leaky after another {:.3?}s",
                        "note".style(self.styles.count),
                        waiting_duration.as_secs_f64(),
                        pid.style(self.styles.count),
                        remaining.as_secs_f64(),
                    )?;
                }
            }
            UnitState::Terminating(state) => {
                self.write_terminating_state(kind, attempt_str, state, writer)?;
            }
            UnitState::Exited {
                result,
                time_taken,
                slow_after,
            } => {
                write!(writer, "{status_str}: {attempt_str}{kind} ")?;
                self.write_info_execution_result(Some(*result), slow_after.is_some(), writer)?;
                write!(writer, " after {:.3?}s", time_taken.as_secs_f64())?;
                if let Some(slow_after) = slow_after {
                    write!(
                        writer,
                        " (marked slow after {:.3?}s)",
                        slow_after.as_secs_f64()
                    )?;
                }
                writeln!(writer)?;
            }
            UnitState::DelayBeforeNextAttempt {
                previous_result,
                previous_slow,
                waiting_duration,
                remaining,
            } => {
                write!(writer, "{status_str}: {attempt_str}{kind} ")?;
                self.write_info_execution_result(Some(*previous_result), *previous_slow, writer)?;
                writeln!(
                    writer,
                    ", currently {} before next attempt",
                    "waiting".style(self.styles.count)
                )?;
                writeln!(
                    writer,
                    "{}:   waited {:.3?}s so far, will wait another {:.3?}s before retrying {kind}",
                    "note".style(self.styles.count),
                    waiting_duration.as_secs_f64(),
                    remaining.as_secs_f64(),
                )?;
            }
        }

        Ok(())
    }

    fn write_terminating_state(
        &self,
        kind: UnitKind,
        attempt_str: &str,
        state: &UnitTerminatingState,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        #[cfg_attr(not(any(unix, test)), expect(unused_variables))]
        let UnitTerminatingState {
            pid,
            time_taken,
            reason,
            method,
            waiting_duration,
            remaining,
        } = state;

        writeln!(
            writer,
            "{}: {attempt_str}{} {kind} PID {} due to {} ({} ran for {:.3?}s)",
            "status".style(self.styles.count),
            "terminating".style(self.styles.fail),
            pid.style(self.styles.count),
            reason.style(self.styles.count),
            kind,
            time_taken.as_secs_f64(),
        )?;

        match method {
            #[cfg(unix)]
            UnitTerminateMethod::Signal(signal) => {
                writeln!(
                    writer,
                    "{}:   sent {} to process group; spent {:.3?}s waiting for {} to exit, \
                     will SIGKILL after another {:.3?}s",
                    "note".style(self.styles.count),
                    signal,
                    waiting_duration.as_secs_f64(),
                    kind,
                    remaining.as_secs_f64(),
                )?;
            }
            #[cfg(windows)]
            UnitTerminateMethod::JobObject => {
                writeln!(
                    writer,
                    // Job objects are like SIGKILL -- they terminate
                    // immediately. No need to show the waiting duration or
                    // remaining time.
                    "{}:   instructed job object to terminate",
                    "note".style(self.styles.count),
                )?;
            }
            #[cfg(test)]
            UnitTerminateMethod::Fake => {
                // This is only used in tests.
                writeln!(
                    writer,
                    "{}:   fake termination method; spent {:.3?}s waiting for {} to exit, \
                     will kill after another {:.3?}s",
                    "note".style(self.styles.count),
                    waiting_duration.as_secs_f64(),
                    kind,
                    remaining.as_secs_f64(),
                )?;
            }
        }

        Ok(())
    }

    // TODO: this should be unified with write_exit_status above -- we need a
    // general, short description of what's happened to both an in-progress and
    // a final unit.
    fn write_info_execution_result(
        &self,
        result: Option<ExecutionResult>,
        is_slow: bool,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        match result {
            Some(ExecutionResult::Pass) => {
                let style = if is_slow {
                    self.styles.skip
                } else {
                    self.styles.pass
                };

                write!(writer, "{}", "passed".style(style))
            }
            Some(ExecutionResult::Leak) => write!(
                writer,
                "{}",
                "passed with leaked handles".style(self.styles.skip)
            ),
            Some(ExecutionResult::Timeout) => {
                write!(writer, "{}", "timed out".style(self.styles.fail))
            }
            Some(ExecutionResult::Fail {
                abort_status,
                leaked,
            }) => {
                if abort_status.is_some() {
                    write!(writer, "{}", "aborted".style(self.styles.fail))
                    // The errors are shown in the output.
                } else if leaked {
                    write!(
                        writer,
                        "{} with leaked handles",
                        "failed".style(self.styles.fail)
                    )
                } else {
                    write!(writer, "{}", "failed".style(self.styles.fail))
                }
            }
            Some(ExecutionResult::ExecFail) => {
                write!(writer, "{}", "failed to execute".style(self.styles.fail))
            }
            None => {
                write!(
                    writer,
                    "{} with unknown status",
                    "failed".style(self.styles.fail)
                )
            }
        }
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

    fn write_setup_script_execute_status(
        &self,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        run_status: &SetupScriptExecuteStatus,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_script(script_id, command, args, run_status);
        self.write_child_execution_output(&spec, &run_status.output, writer)
    }

    fn write_test_execute_status(
        &self,
        test_instance: &TestInstance<'a>,
        run_status: &ExecuteStatus,
        is_retry: bool,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_test(test_instance.id(), run_status, is_retry);
        self.write_child_execution_output(&spec, &run_status.output, writer)
    }

    fn write_child_execution_output(
        &self,
        spec: &ChildOutputSpec,
        exec_output: &ChildExecutionOutput,
        mut writer: &mut dyn Write,
    ) -> io::Result<()> {
        match exec_output {
            ChildExecutionOutput::Output {
                output,
                // result and errors are captured by desc.
                result: _,
                errors: _,
            } => {
                let desc = UnitErrorDescription::new(spec.kind, exec_output);

                // Show execution failures first so that they show up
                // immediately after the failure notification.
                if let Some(errors) = desc.exec_fail_error_list() {
                    writeln!(writer, "{}", spec.exec_fail_header)?;

                    // Indent the displayed error chain.
                    let error_chain = DisplayErrorChain::new(errors);
                    let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                    writeln!(indent_writer, "{error_chain}")?;
                    indent_writer.flush()?;
                    writer = indent_writer.into_inner();
                }

                let highlight_slice = if self.styles.is_colorized {
                    desc.output_slice()
                } else {
                    None
                };
                self.write_child_output(spec, output, highlight_slice, writer)?;
            }

            ChildExecutionOutput::StartError(error) => {
                writeln!(writer, "{}", spec.exec_fail_header)?;

                // Indent the displayed error chain.
                let error_chain = DisplayErrorChain::new(error);
                let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                writeln!(indent_writer, "{error_chain}")?;
                indent_writer.flush()?;
                writer = indent_writer.into_inner();
            }
        }

        writeln!(writer)
    }

    fn write_child_output(
        &self,
        spec: &ChildOutputSpec,
        output: &ChildOutput,
        highlight_slice: Option<TestOutputErrorSlice<'_>>,
        mut writer: &mut dyn Write,
    ) -> io::Result<()> {
        match output {
            ChildOutput::Split(split) => {
                if let Some(stdout) = &split.stdout {
                    if self.display_empty_outputs || !stdout.is_empty() {
                        writeln!(writer, "{}", spec.stdout_header)?;

                        // If there's no output indent, this is a no-op, though
                        // it will bear the perf cost of a vtable indirection +
                        // whatever internal state IndentWriter tracks. Doubt
                        // this will be an issue in practice though!
                        let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                        self.write_test_single_output_with_description(
                            stdout,
                            highlight_slice.and_then(|d| d.stdout_subslice()),
                            &mut indent_writer,
                        )?;
                        indent_writer.flush()?;
                        writer = indent_writer.into_inner();
                    }
                }

                if let Some(stderr) = &split.stderr {
                    if self.display_empty_outputs || !stderr.is_empty() {
                        writeln!(writer, "{}", spec.stderr_header)?;

                        let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                        self.write_test_single_output_with_description(
                            stderr,
                            highlight_slice.and_then(|d| d.stderr_subslice()),
                            &mut indent_writer,
                        )?;
                        indent_writer.flush()?;
                    }
                }
            }
            ChildOutput::Combined { output } => {
                if self.display_empty_outputs || !output.is_empty() {
                    writeln!(writer, "{}", spec.combined_header)?;

                    let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                    self.write_test_single_output_with_description(
                        output,
                        highlight_slice.and_then(|d| d.combined_subslice()),
                        &mut indent_writer,
                    )?;
                    indent_writer.flush()?;
                }
            }
        }

        Ok(())
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
                write_output_with_trailing_newline(&output.buf, RESET_COLOR, writer)?;
            }
        } else {
            // Strip ANSI escapes from the output if nextest itself isn't colorized.
            let mut no_color = strip_ansi_escapes::Writer::new(writer);
            write_output_with_trailing_newline(&output.buf, b"", &mut no_color)?;
        }

        Ok(())
    }

    // Returns the number of characters written out to the screen.
    fn write_attempt(&self, run_status: &ExecuteStatus, style: Style, out: &mut String) -> usize {
        if run_status.retry_data.total_attempts > 1 {
            // 3 for 'TRY' + 1 for ' ' + length of the current attempt + 1 for following space.
            let attempt_str = format!("{}", run_status.retry_data.attempt);
            let out_len = 3 + 1 + attempt_str.len() + 1;
            swrite!(out, "{} {} ", "TRY".style(style), attempt_str.style(style));
            out_len
        } else {
            0
        }
    }

    fn success_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_success_output.unwrap_or(test_setting)
    }

    fn failure_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_failure_output.unwrap_or(test_setting)
    }

    fn output_spec_for_test(
        &self,
        test_instance: TestInstanceId<'a>,
        run_status: &ExecuteStatus,
        is_retry: bool,
    ) -> ChildOutputSpec {
        let header_style = if is_retry {
            self.styles.retry
        } else if run_status.result.is_success() {
            self.styles.pass
        } else {
            self.styles.fail
        };

        let hbar = self.theme_characters.hbar(4);

        let stdout_header = {
            let mut header = String::new();
            swrite!(header, "{} ", hbar.style(header_style));
            let out_len = self.write_attempt(run_status, header_style, &mut header);
            swrite!(
                header,
                "{:width$} {}",
                "STDOUT:".style(header_style),
                self.display_test_instance(test_instance),
                // The width is to align test instances.
                width = (19 - out_len),
            );
            header
        };

        let stderr_header = {
            let mut header = String::new();
            swrite!(header, "{} ", hbar.style(header_style));
            let out_len = self.write_attempt(run_status, header_style, &mut header);
            swrite!(
                header,
                "{:width$} {}",
                "STDERR:".style(header_style),
                self.display_test_instance(test_instance),
                // The width is to align test instances.
                width = (19 - out_len),
            );
            header
        };

        let combined_header = {
            let mut header = String::new();
            swrite!(header, "{} ", hbar.style(header_style));
            let out_len = self.write_attempt(run_status, header_style, &mut header);
            swrite!(
                header,
                "{:width$} {}",
                "OUTPUT:".style(header_style),
                self.display_test_instance(test_instance),
                // The width is to align test instances.
                width = (19 - out_len),
            );
            header
        };

        let exec_fail_header = {
            let mut header = String::new();
            swrite!(header, "{} ", hbar.style(header_style));
            let out_len = self.write_attempt(run_status, header_style, &mut header);
            swrite!(
                header,
                "{:width$} {}",
                "EXECFAIL:".style(header_style),
                self.display_test_instance(test_instance),
                // The width is to align test instances.
                width = (19 - out_len)
            );
            header
        };

        ChildOutputSpec {
            kind: UnitKind::Test,
            stdout_header,
            stderr_header,
            combined_header,
            exec_fail_header,
            // No output indent for now -- maybe this should be supported?
            // Definitely worth trying out.
            output_indent: "",
        }
    }

    // Info response queries are more compact and so have a somewhat different
    // output format. But at some point we should consider using the same format
    // for both regular test output and info responses.
    fn output_spec_for_info(&self, kind: UnitKind) -> ChildOutputSpec {
        let stdout_header = format!("{}:", "stdout".style(self.styles.count));
        let stderr_header = format!("{}:", "stderr".style(self.styles.count));
        let combined_header = format!("{}:", "output".style(self.styles.count));
        let exec_fail_header = format!("{}:", "errors".style(self.styles.count));

        ChildOutputSpec {
            kind,
            stdout_header,
            stderr_header,
            combined_header,
            exec_fail_header,
            output_indent: "  ",
        }
    }

    fn output_spec_for_script(
        &self,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        run_status: &SetupScriptExecuteStatus,
    ) -> ChildOutputSpec {
        let header_style = if run_status.result.is_success() {
            self.styles.pass
        } else {
            self.styles.fail
        };

        let hbar = self.theme_characters.hbar(4);

        let stdout_header = {
            format!(
                "{} {:19} {}",
                hbar.style(header_style),
                "STDOUT:".style(header_style),
                self.display_script_instance(script_id.clone(), command, args),
            )
        };

        let stderr_header = {
            format!(
                "{} {:19} {}",
                hbar.style(header_style),
                "STDERR:".style(header_style),
                self.display_script_instance(script_id.clone(), command, args),
            )
        };

        let combined_header = {
            format!(
                "{} {:19} {}",
                hbar.style(header_style),
                "OUTPUT:".style(header_style),
                self.display_script_instance(script_id.clone(), command, args),
            )
        };

        let exec_fail_header = {
            format!(
                "{} {:19} {}",
                hbar.style(header_style),
                "EXECFAIL:".style(header_style),
                self.display_script_instance(script_id.clone(), command, args),
            )
        };

        ChildOutputSpec {
            kind: UnitKind::Script,
            stdout_header,
            stderr_header,
            combined_header,
            exec_fail_header,
            output_indent: "",
        }
    }
}

impl fmt::Debug for TestReporter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TestReporter")
            .field("stdout", &"BufferWriter { .. }")
            .field("stderr", &"BufferWriter { .. }")
            .finish()
    }
}

const RESET_COLOR: &[u8] = b"\x1b[0m";

/// Formatting options for writing out child process output.
///
/// TODO: should these be lazily generated? Can't imagine this ever being
/// measurably slow.
#[derive(Debug)]
struct ChildOutputSpec {
    kind: UnitKind,
    stdout_header: String,
    stderr_header: String,
    combined_header: String,
    exec_fail_header: String,
    output_indent: &'static str,
}

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
    write_output_with_trailing_newline(&output[end..], RESET_COLOR, writer)?;

    Ok(())
}

/// Write output, always ensuring there's a trailing newline. (If there's no
/// newline, one will be inserted.)
///
/// `trailer` is written immediately before the trailing newline if any.
fn write_output_with_trailing_newline(
    mut output: &[u8],
    trailer: &[u8],
    writer: &mut dyn Write,
) -> io::Result<()> {
    // If there's a trailing newline in the output, insert the trailer right
    // before it.
    if output.last() == Some(&b'\n') {
        output = &output[..output.len() - 1];
    }

    writer.write_all(output)?;
    writer.write_all(trailer)?;
    writer.write_all(b"\n")
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
                    "{}: {}/{} {} {} not run due to {} (run with {} to run all tests, or run with {})",
                    "warning".style(styles.skip),
                    not_run.style(styles.count),
                    initial_run_count.style(styles.count),
                    plural::tests_plural_if(initial_run_count != 1 || not_run != 1),
                    plural::were_plural_if(initial_run_count != 1 || not_run != 1),
                    CancelReason::TestFailure.to_static_str().style(styles.skip),
                    "--no-fail-fast".style(styles.count),
                    "--max-fail".style(styles.count),
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

#[derive(Debug, Default)]
struct Styles {
    is_colorized: bool,
    count: Style,
    pass: Style,
    retry: Style,
    fail: Style,
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
        self.skip = Style::new().yellow().bold();
        self.script_id = Style::new().blue().bold();
        self.list_styles.colorize();
    }
}

#[derive(Debug)]
struct ThemeCharacters {
    hbar: char,
    progress_chars: &'static str,
}

impl Default for ThemeCharacters {
    fn default() -> Self {
        Self {
            hbar: '-',
            progress_chars: "=> ",
        }
    }
}

impl ThemeCharacters {
    fn use_unicode(&mut self) {
        self.hbar = '─';
        // https://mike42.me/blog/2018-06-make-better-cli-progress-bars-with-unicode-block-characters
        self.progress_chars = "█▉▊▋▌▍▎▏ ";
    }

    fn hbar(&self, width: usize) -> String {
        std::iter::repeat(self.hbar).take(width).collect()
    }
}

fn decimal_char_width(n: usize) -> u32 {
    // checked_ilog10 returns 0 for 1-9, 1 for 10-99, 2 for 100-999, etc. (And
    // None for 0 which we unwrap to the same as 1). Add 1 to it to get the
    // actual number of digits.
    n.checked_ilog10().unwrap_or(0) + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{CompiledDefaultFilterSection, NextestConfig},
        errors::{ChildError, ChildFdError, ChildStartError, ErrorList},
        platform::BuildPlatforms,
        reporter::{structured::StructuredReporter, UnitTerminateReason},
        test_output::ChildSplitOutput,
    };
    use bytes::Bytes;
    use chrono::Local;
    use nextest_filtering::CompiledExpr;
    use nextest_metadata::RustBinaryId;
    use smol_str::SmolStr;
    use std::sync::Arc;
    use test_strategy::proptest;

    /// Creates a test reporter with default settings and calls the given function with it.
    ///
    /// Returns the output written to the reporter.
    fn with_reporter<'a, F>(config: &'a NextestConfig, f: F, out: &'a mut Vec<u8>)
    where
        F: FnOnce(TestReporter<'a>),
    {
        let mut builder = TestReporterBuilder::default();
        builder
            .set_no_capture(true)
            .set_failure_output(TestOutputDisplay::Immediate)
            .set_success_output(TestOutputDisplay::Immediate)
            .set_status_level(StatusLevel::Fail);
        let test_list = TestList::empty();
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let build_platforms = BuildPlatforms::new_with_no_target().unwrap();

        let output = ReporterStderr::Buffer(out);
        let reporter = builder.build(
            &test_list,
            &profile.apply_build_platforms(&build_platforms),
            output,
            StructuredReporter::new(),
        );

        f(reporter);
    }

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
            format!("{RESET_COLOR}{BOLD_RED}output{RESET_COLOR}{RESET_COLOR}\n")
        );

        assert_eq!(
            write_output_with_highlight_buf("output", 1, Some(5)),
            format!("o{RESET_COLOR}{BOLD_RED}utpu{RESET_COLOR}t{RESET_COLOR}\n")
        );

        assert_eq!(
            write_output_with_highlight_buf("output\nhighlight 1\nhighlight 2\n", 7, None),
            format!(
                "output\n{RESET_COLOR}\
                {BOLD_RED}highlight 1{RESET_COLOR}\n\
                {BOLD_RED}highlight 2{RESET_COLOR}{RESET_COLOR}\n"
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
                not highlighted{RESET_COLOR}\n"
            )
        );
    }

    /// Send an information response to the reporter and return the output.
    #[test]
    fn test_info_response() {
        let args = vec!["arg1".to_string(), "arg2".to_string()];
        let binary_id = RustBinaryId::new("my-binary-id");

        let config = NextestConfig::default_config("/fake/dir");
        let mut out = Vec::new();

        with_reporter(
            &config,
            |mut reporter| {
                // Info started event.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoStarted {
                            total: 30,
                            run_stats: RunStats {
                                initial_run_count: 40,
                                finished_count: 20,
                                setup_scripts_initial_count: 1,
                                setup_scripts_finished_count: 1,
                                setup_scripts_passed: 1,
                                setup_scripts_failed: 0,
                                setup_scripts_exec_failed: 0,
                                setup_scripts_timed_out: 0,
                                passed: 17,
                                passed_slow: 4,
                                flaky: 2,
                                failed: 2,
                                failed_slow: 1,
                                timed_out: 1,
                                leaky: 1,
                                exec_failed: 1,
                                skipped: 5,
                            },
                        },
                    })
                    .unwrap();

                // A basic setup script.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 0,
                            total: 20,
                            // Technically, you won't get setup script and test responses in the
                            // same response, but it's easiest to test in this manner.
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                script_id: ScriptId::new(SmolStr::new("setup")).unwrap(),
                                command: "setup",
                                args: &args,
                                state: UnitState::Running {
                                    pid: 4567,
                                    time_taken: Duration::from_millis(1234),
                                    slow_after: None,
                                },
                                output: make_split_output(
                                    None,
                                    "script stdout 1",
                                    "script stderr 1",
                                ),
                            }),
                        },
                    })
                    .unwrap();

                // A setup script with a slow warning, combined output, and an
                // execution failure.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 1,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                script_id: ScriptId::new(SmolStr::new("setup-slow")).unwrap(),
                                command: "setup-slow",
                                args: &args,
                                state: UnitState::Running {
                                    pid: 4568,
                                    time_taken: Duration::from_millis(1234),
                                    slow_after: Some(Duration::from_millis(1000)),
                                },
                                output: make_combined_output_with_errors(
                                    None,
                                    "script output 2\n",
                                    vec![ChildError::Fd(ChildFdError::ReadStdout(Arc::new(
                                        std::io::Error::other("read stdout error"),
                                    )))],
                                ),
                            }),
                        },
                    })
                    .unwrap();

                // A setup script that's terminating and has multiple errors.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 2,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                script_id: ScriptId::new(SmolStr::new("setup-terminating"))
                                    .unwrap(),
                                command: "setup-terminating",
                                args: &args,
                                state: UnitState::Terminating(UnitTerminatingState {
                                    pid: 5094,
                                    time_taken: Duration::from_millis(1234),
                                    reason: UnitTerminateReason::Signal,
                                    method: UnitTerminateMethod::Fake,
                                    waiting_duration: Duration::from_millis(6789),
                                    remaining: Duration::from_millis(9786),
                                }),
                                output: make_split_output_with_errors(
                                    None,
                                    "script output 3\n",
                                    "script stderr 3\n",
                                    vec![
                                        ChildError::Fd(ChildFdError::ReadStdout(Arc::new(
                                            std::io::Error::other("read stdout error"),
                                        ))),
                                        ChildError::Fd(ChildFdError::ReadStderr(Arc::new(
                                            std::io::Error::other("read stderr error"),
                                        ))),
                                    ],
                                ),
                            }),
                        },
                    })
                    .unwrap();

                // A setup script that's about to exit along with a start error
                // (this is not a real situation but we're just testing out
                // various cases).
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 3,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                script_id: ScriptId::new(SmolStr::new("setup-exiting")).unwrap(),
                                command: "setup-exiting",
                                args: &args,
                                state: UnitState::Exiting {
                                    pid: 9987,
                                    time_taken: Duration::from_millis(1234),
                                    slow_after: Some(Duration::from_millis(1000)),
                                    // Even if exit_status is 0, the presence of
                                    // exec-fail errors should be considered
                                    // part of the output.
                                    tentative_result: Some(ExecutionResult::ExecFail),
                                    waiting_duration: Duration::from_millis(10467),
                                    remaining: Duration::from_millis(335),
                                },
                                output: ChildExecutionOutput::StartError(ChildStartError::Spawn(
                                    Arc::new(std::io::Error::other("exec error")),
                                )),
                            }),
                        },
                    })
                    .unwrap();

                // A setup script that has exited.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 4,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                script_id: ScriptId::new(SmolStr::new("setup-exited")).unwrap(),
                                command: "setup-exited",
                                args: &args,
                                state: UnitState::Exited {
                                    result: ExecutionResult::Fail {
                                        abort_status: None,
                                        leaked: true,
                                    },
                                    time_taken: Duration::from_millis(9999),
                                    slow_after: Some(Duration::from_millis(3000)),
                                },
                                output: ChildExecutionOutput::StartError(ChildStartError::Spawn(
                                    Arc::new(std::io::Error::other("exec error")),
                                )),
                            }),
                        },
                    })
                    .unwrap();

                // A test is running.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 5,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                test_instance: TestInstanceId {
                                    binary_id: &binary_id,
                                    test_name: "test1",
                                },
                                retry_data: RetryData {
                                    attempt: 1,
                                    total_attempts: 1,
                                },
                                state: UnitState::Running {
                                    pid: 12345,
                                    time_taken: Duration::from_millis(400),
                                    slow_after: None,
                                },
                                output: make_split_output(None, "abc", "def"),
                            }),
                        },
                    })
                    .unwrap();

                // A test is being terminated due to a timeout.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 6,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                test_instance: TestInstanceId {
                                    binary_id: &binary_id,
                                    test_name: "test2",
                                },
                                retry_data: RetryData {
                                    attempt: 2,
                                    total_attempts: 3,
                                },
                                state: UnitState::Terminating(UnitTerminatingState {
                                    pid: 12346,
                                    time_taken: Duration::from_millis(99999),
                                    reason: UnitTerminateReason::Timeout,
                                    method: UnitTerminateMethod::Fake,
                                    waiting_duration: Duration::from_millis(6789),
                                    remaining: Duration::from_millis(9786),
                                }),
                                output: make_split_output(None, "abc", "def"),
                            }),
                        },
                    })
                    .unwrap();

                // A test is exiting.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 7,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                test_instance: TestInstanceId {
                                    binary_id: &binary_id,
                                    test_name: "test3",
                                },
                                retry_data: RetryData {
                                    attempt: 2,
                                    total_attempts: 3,
                                },
                                state: UnitState::Exiting {
                                    pid: 99999,
                                    time_taken: Duration::from_millis(99999),
                                    slow_after: Some(Duration::from_millis(33333)),
                                    tentative_result: None,
                                    waiting_duration: Duration::from_millis(1),
                                    remaining: Duration::from_millis(999),
                                },
                                output: make_split_output(None, "abc", "def"),
                            }),
                        },
                    })
                    .unwrap();

                // A test has exited.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 8,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                test_instance: TestInstanceId {
                                    binary_id: &binary_id,
                                    test_name: "test4",
                                },
                                retry_data: RetryData {
                                    attempt: 1,
                                    total_attempts: 5,
                                },
                                state: UnitState::Exited {
                                    result: ExecutionResult::Pass,
                                    time_taken: Duration::from_millis(99999),
                                    slow_after: Some(Duration::from_millis(33333)),
                                },
                                output: make_combined_output_with_errors(
                                    Some(ExecutionResult::Pass),
                                    "abc\ndef\nghi\n",
                                    vec![ChildError::Fd(ChildFdError::Wait(Arc::new(
                                        std::io::Error::other("error waiting"),
                                    )))],
                                ),
                            }),
                        },
                    })
                    .unwrap();

                // Delay before next attempt.
                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 9,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                test_instance: TestInstanceId {
                                    binary_id: &binary_id,
                                    test_name: "test4",
                                },
                                retry_data: RetryData {
                                    // Note that even though attempt is 1, we
                                    // still show it in the UI in this special
                                    // case.
                                    attempt: 1,
                                    total_attempts: 5,
                                },
                                state: UnitState::DelayBeforeNextAttempt {
                                    previous_result: ExecutionResult::ExecFail,
                                    previous_slow: true,
                                    waiting_duration: Duration::from_millis(1234),
                                    remaining: Duration::from_millis(5678),
                                },
                                // In reality, the output isn't available at this point,
                                // and it shouldn't be shown.
                                output: make_combined_output_with_errors(
                                    Some(ExecutionResult::Pass),
                                    "*** THIS OUTPUT SHOULD BE IGNORED",
                                    vec![ChildError::Fd(ChildFdError::Wait(Arc::new(
                                        std::io::Error::other(
                                            "*** THIS ERROR SHOULD ALSO BE IGNORED",
                                        ),
                                    )))],
                                ),
                            }),
                        },
                    })
                    .unwrap();

                reporter
                    .write_event(TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoFinished { missing: 2 },
                    })
                    .unwrap();
            },
            &mut out,
        );

        insta::assert_snapshot!(
            "info_response_output",
            String::from_utf8(out).expect("output only consists of UTF-8"),
        );
    }

    fn make_split_output(
        result: Option<ExecutionResult>,
        stdout: &str,
        stderr: &str,
    ) -> ChildExecutionOutput {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: Some(Bytes::from(stdout.to_owned()).into()),
                stderr: Some(Bytes::from(stderr.to_owned()).into()),
            }),
            errors: None,
        }
    }

    fn make_split_output_with_errors(
        result: Option<ExecutionResult>,
        stdout: &str,
        stderr: &str,
        errors: Vec<ChildError>,
    ) -> ChildExecutionOutput {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: Some(Bytes::from(stdout.to_owned()).into()),
                stderr: Some(Bytes::from(stderr.to_owned()).into()),
            }),
            errors: ErrorList::new("testing split output", errors),
        }
    }

    fn make_combined_output_with_errors(
        result: Option<ExecutionResult>,
        output: &str,
        errors: Vec<ChildError>,
    ) -> ChildExecutionOutput {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Combined {
                output: Bytes::from(output.to_owned()).into(),
            },
            errors: ErrorList::new("testing split output", errors),
        }
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

        let config = NextestConfig::default_config("/fake/dir");
        let mut out = Vec::new();

        with_reporter(
            &config,
            |reporter| {
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
            },
            &mut out,
        );
    }

    #[test]
    fn test_progress_bar_prefix() {
        let mut styles = Styles::default();
        styles.colorize();

        for stats in run_stats_test_failure_examples() {
            let prefix = progress_bar_prefix(&stats, Some(CancelReason::TestFailure), &styles);
            assert_eq!(prefix, "  Cancelling".style(styles.fail).to_string());
        }
        for stats in run_stats_setup_script_failure_examples() {
            let prefix =
                progress_bar_prefix(&stats, Some(CancelReason::SetupScriptFailure), &styles);
            assert_eq!(prefix, "  Cancelling".style(styles.fail).to_string());
        }

        let prefix = progress_bar_prefix(&RunStats::default(), Some(CancelReason::Signal), &styles);
        assert_eq!(prefix, "  Cancelling".style(styles.fail).to_string());

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
             (run with --no-fail-fast to run all tests, or run with --max-fail)\n"
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
