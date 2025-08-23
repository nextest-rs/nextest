// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{
    ChildOutputSpec, FinalStatusLevel, OutputStoreFinal, StatusLevel, StatusLevels,
    UnitOutputReporter,
    formatters::{
        DisplayBracketedDuration, DisplayDurationBy, DisplaySlowDuration, write_final_warnings,
        write_skip_counts,
    },
    progress::{ProgressBarState, progress_bar_msg, progress_str, write_summary_str},
    unit_output::TestOutputDisplay,
};
use crate::{
    cargo_config::CargoConfigs,
    config::{elements::LeakTimeoutResult, overrides::CompiledDefaultFilter, scripts::ScriptId},
    errors::WriteEventError,
    helpers::{DisplayScriptInstance, DisplayTestInstance, plural},
    list::{TestInstance, TestInstanceId},
    reporter::{
        displayer::{formatters::DisplayHhMmSs, progress::TerminalProgress},
        events::*,
        helpers::Styles,
        imp::ReporterStderr,
    },
    runner::StressCount,
};
use debug_ignore::DebugIgnore;
use indent_write::io::IndentWriter;
use nextest_metadata::MismatchReason;
use owo_colors::OwoColorize;
use std::{
    borrow::Cow,
    cmp::Reverse,
    io::{self, BufWriter, Write},
    time::Duration,
};

pub(crate) struct DisplayReporterBuilder {
    pub(crate) default_filter: CompiledDefaultFilter,
    pub(crate) status_levels: StatusLevels,
    pub(crate) test_count: usize,
    pub(crate) success_output: Option<TestOutputDisplay>,
    pub(crate) failure_output: Option<TestOutputDisplay>,
    pub(crate) should_colorize: bool,
    pub(crate) no_capture: bool,
    pub(crate) hide_progress_bar: bool,
    pub(crate) no_output_indent: bool,
}

impl DisplayReporterBuilder {
    pub(crate) fn build<'a>(
        self,
        configs: &CargoConfigs,
        output: ReporterStderr<'a>,
    ) -> DisplayReporter<'a> {
        let mut styles: Box<Styles> = Box::default();
        if self.should_colorize {
            styles.colorize();
        }

        let status_level = match self.no_capture {
            // In no-capture mode, the status level is treated as at least pass.
            true => self.status_levels.status_level.max(StatusLevel::Pass),
            false => self.status_levels.status_level,
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
            ReporterStderr::Terminal => {
                let progress_bar = if self.no_capture {
                    // Do not use a progress bar if --no-capture is passed in.
                    // This is required since we pass down stderr to the child
                    // process.
                    //
                    // In the future, we could potentially switch to using a
                    // pty, in which case we could still potentially use the
                    // progress bar as a status bar. However, that brings about
                    // its own complications: what if a test's output doesn't
                    // include a newline? We might have to use a curses-like UI
                    // which would be a lot of work for not much gain.
                    None
                } else if is_ci::uncached() {
                    // Some CI environments appear to pretend to be a terminal.
                    // Disable the progress bar in these environments.
                    None
                } else if self.hide_progress_bar {
                    None
                } else {
                    let state =
                        ProgressBarState::new(self.test_count, theme_characters.progress_chars);
                    // Note: even if we create a progress bar here, if stderr is
                    // piped, indicatif will not show it.
                    Some(state)
                };
                let term_progress = TerminalProgress::new(configs, &io::stderr());
                ReporterStderrImpl::Terminal {
                    progress_bar,
                    term_progress,
                }
            }
            ReporterStderr::Buffer(buf) => ReporterStderrImpl::Buffer(buf),
        };

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

        DisplayReporter {
            inner: DisplayReporterImpl {
                default_filter: self.default_filter,
                status_levels: StatusLevels {
                    status_level,
                    final_status_level: self.status_levels.final_status_level,
                },
                no_capture: self.no_capture,
                no_output_indent: self.no_output_indent,
                styles,
                theme_characters,
                cancel_status: None,
                unit_output: UnitOutputReporter::new(force_success_output, force_failure_output),
                final_outputs: DebugIgnore(Vec::new()),
            },
            stderr,
        }
    }
}

/// Functionality to report test results to stderr, JUnit, and/or structured,
/// machine-readable results to stdout
pub(crate) struct DisplayReporter<'a> {
    inner: DisplayReporterImpl<'a>,
    stderr: ReporterStderrImpl<'a>,
}

impl<'a> DisplayReporter<'a> {
    pub(crate) fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        match &mut self.stderr {
            ReporterStderrImpl::Terminal {
                progress_bar,
                term_progress,
            } => {
                if let Some(state) = progress_bar {
                    // Write to a string that will be printed as a log line.
                    let mut buf: Vec<u8> = Vec::new();
                    self.inner
                        .write_event_impl(event, &mut buf)
                        .map_err(WriteEventError::Io)?;

                    state.update_progress_bar(event, &self.inner.styles);
                    if let Some(term_progress) = term_progress {
                        term_progress
                            .update_progress(event, &mut buf)
                            .map_err(WriteEventError::Io)?;
                    }
                    state.write_buf(&buf).map_err(WriteEventError::Io)
                } else {
                    // Write to a buffered stderr.
                    let mut writer = BufWriter::new(std::io::stderr());
                    self.inner
                        .write_event_impl(event, &mut writer)
                        .map_err(WriteEventError::Io)?;
                    if let Some(state) = term_progress {
                        state
                            .update_progress(event, &mut writer)
                            .map_err(WriteEventError::Io)?;
                    }
                    writer.flush().map_err(WriteEventError::Io)
                }
            }
            ReporterStderrImpl::Buffer(buf) => self
                .inner
                .write_event_impl(event, *buf)
                .map_err(WriteEventError::Io),
        }
    }

    pub(crate) fn finish(&mut self) {
        self.stderr.finish_and_clear_bar();
    }
}

enum ReporterStderrImpl<'a> {
    Terminal {
        // Reporter-specific progress bar state. None if the progress bar is not
        // enabled.
        progress_bar: Option<ProgressBarState>,
        // OSC 9 code progress reporting.
        term_progress: Option<TerminalProgress>,
    },
    Buffer(&'a mut Vec<u8>),
}

impl ReporterStderrImpl<'_> {
    fn finish_and_clear_bar(&self) {
        match self {
            ReporterStderrImpl::Terminal {
                progress_bar: Some(state),
                ..
            } => {
                state.finish_and_clear();
            }
            ReporterStderrImpl::Terminal { .. } | ReporterStderrImpl::Buffer(_) => {}
        }
    }

    #[cfg(test)]
    fn buf_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            Self::Buffer(buf) => Some(buf),
            _ => None,
        }
    }
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

struct DisplayReporterImpl<'a> {
    default_filter: CompiledDefaultFilter,
    status_levels: StatusLevels,
    no_capture: bool,
    no_output_indent: bool,
    styles: Box<Styles>,
    theme_characters: ThemeCharacters,
    cancel_status: Option<CancelReason>,
    unit_output: UnitOutputReporter,
    final_outputs: DebugIgnore<Vec<(Option<StressIndex>, TestInstance<'a>, FinalOutput)>>,
}

impl<'a> DisplayReporterImpl<'a> {
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
                stress_condition: _,
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
            TestEventKind::StressSubRunStarted { progress } => {
                write!(
                    writer,
                    "{}\n{:>12} ",
                    self.theme_characters.hbar(12),
                    "Stress test".style(self.styles.pass)
                )?;

                match progress {
                    StressProgress::Count {
                        total: StressCount::Count(total),
                        elapsed,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {}/{} ({} elapsed so far",
                            (completed + 1).style(self.styles.count),
                            total.style(self.styles.count),
                            DisplayHhMmSs {
                                duration: *elapsed,
                                floor: true,
                            }
                            .style(self.styles.count),
                        )?;
                    }
                    StressProgress::Count {
                        total: StressCount::Infinite,
                        elapsed,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {} ({} elapsed so far",
                            (completed + 1).style(self.styles.count),
                            DisplayHhMmSs {
                                duration: *elapsed,
                                floor: true,
                            }
                            .style(self.styles.count),
                        )?;
                    }
                    StressProgress::Time {
                        total,
                        elapsed,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {} ({}/{} elapsed so far",
                            (completed + 1).style(self.styles.count),
                            DisplayHhMmSs {
                                duration: *elapsed,
                                floor: true,
                            }
                            .style(self.styles.count),
                            DisplayHhMmSs {
                                duration: *total,
                                floor: true,
                            }
                            .style(self.styles.count),
                        )?;
                    }
                }

                if let Some(remaining) = progress.remaining() {
                    match remaining {
                        StressRemaining::Count(n) => {
                            write!(
                                writer,
                                ", {} iterations remaining",
                                n.style(self.styles.count)
                            )?;
                        }
                        StressRemaining::Infinite => {
                            // There isn't anything to display here.
                        }
                        StressRemaining::Time(t) => {
                            write!(
                                writer,
                                ", {} remaining",
                                DisplayHhMmSs {
                                    duration: t,
                                    // Display the remaining time as a ceiling
                                    // so that we show something like:
                                    //
                                    // 00:02:05/00:30:00 elapsed so far, 00:27:55 remaining
                                    //
                                    // rather than
                                    //
                                    // 00:02:05/00:30:00 elapsed so far, 00:27:54 remaining
                                    floor: false,
                                }
                                .style(self.styles.count)
                            )?;
                        }
                    }
                }

                writeln!(writer, ")")?;
            }
            TestEventKind::SetupScriptStarted {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                ..
            } => {
                writeln!(
                    writer,
                    "{:>12} [{:>9}] {}",
                    "SETUP".style(self.styles.pass),
                    // index + 1 so that it displays as e.g. "1/2" and "2/2".
                    format!("{}/{}", index + 1, total),
                    self.display_script_instance(*stress_index, script_id.clone(), program, args)
                )?;
            }
            TestEventKind::SetupScriptSlow {
                stress_index,
                script_id,
                program,
                args,
                elapsed,
                will_terminate,
            } => {
                if !*will_terminate && self.status_levels.status_level >= StatusLevel::Slow {
                    write!(writer, "{:>12} ", "SETUP SLOW".style(self.styles.skip))?;
                } else if *will_terminate {
                    write!(writer, "{:>12} ", "TERMINATING".style(self.styles.fail))?;
                }

                writeln!(
                    writer,
                    "{}{}",
                    DisplaySlowDuration(*elapsed),
                    self.display_script_instance(*stress_index, script_id.clone(), program, args)
                )?;
            }
            TestEventKind::SetupScriptFinished {
                stress_index,
                script_id,
                program,
                args,
                run_status,
                ..
            } => {
                self.write_setup_script_status_line(
                    *stress_index,
                    script_id,
                    program,
                    args,
                    run_status,
                    writer,
                )?;
                // Always display failing setup script output if it exists. We
                // may change this in the future.
                if !run_status.result.is_success() {
                    self.write_setup_script_execute_status(run_status, writer)?;
                }
            }
            TestEventKind::TestStarted {
                stress_index,
                test_instance,
                ..
            } => {
                // In no-capture mode, print out a test start event.
                if self.no_capture {
                    // The spacing is to align test instances.
                    writeln!(
                        writer,
                        "{:>12}             {}",
                        "START".style(self.styles.pass),
                        self.display_test_instance(*stress_index, test_instance.id()),
                    )?;
                }
            }
            TestEventKind::TestSlow {
                stress_index,
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

                writeln!(
                    writer,
                    "{}{}",
                    DisplaySlowDuration(*elapsed),
                    self.display_test_instance(*stress_index, test_instance.id())
                )?;
            }

            TestEventKind::TestAttemptFailedWillRetry {
                stress_index,
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

                    // Print the try status and time taken.
                    write!(
                        writer,
                        "{:>12} {}",
                        try_status_string.style(self.styles.retry),
                        DisplayBracketedDuration(run_status.time_taken),
                    )?;

                    // Print the name of the test.
                    writeln!(
                        writer,
                        "{}",
                        self.display_test_instance(*stress_index, test_instance.id())
                    )?;

                    // This test is guaranteed to have failed.
                    assert!(
                        !run_status.result.is_success(),
                        "only failing tests are retried"
                    );
                    if self
                        .unit_output
                        .failure_output(*failure_output)
                        .is_immediate()
                    {
                        self.write_test_execute_status(run_status, true, writer)?;
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
                        write!(
                            writer,
                            "{:>12} {}",
                            delay_string.style(self.styles.retry),
                            DisplayDurationBy(*delay_before_next_attempt)
                        )?;

                        // Print the name of the test.
                        writeln!(
                            writer,
                            "{}",
                            self.display_test_instance(*stress_index, test_instance.id())
                        )?;
                    }
                }
            }
            TestEventKind::TestRetryStarted {
                stress_index,
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
                    self.display_test_instance(*stress_index, test_instance.id())
                )?;
            }
            TestEventKind::TestFinished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                run_statuses,
                ..
            } => {
                let describe = run_statuses.describe();
                let last_status = run_statuses.last_status();
                let test_output_display = match last_status.result.is_success() {
                    true => self.unit_output.success_output(*success_output),
                    false => self.unit_output.failure_output(*failure_output),
                };

                let output_on_test_finished = self.status_levels.compute_output_on_test_finished(
                    test_output_display,
                    self.cancel_status,
                    describe.status_level(),
                    describe.final_status_level(),
                );

                if output_on_test_finished.write_status_line {
                    self.write_status_line(*stress_index, *test_instance, describe, writer)?;
                }
                if output_on_test_finished.show_immediate {
                    self.write_test_execute_status(last_status, false, writer)?;
                }
                if let OutputStoreFinal::Yes { display_output } =
                    output_on_test_finished.store_final
                {
                    self.final_outputs.push((
                        *stress_index,
                        *test_instance,
                        FinalOutput::Executed {
                            run_statuses: run_statuses.clone(),
                            display_output,
                        },
                    ));
                }
            }
            TestEventKind::TestSkipped {
                stress_index,
                test_instance,
                reason,
            } => {
                if self.status_levels.status_level >= StatusLevel::Skip {
                    self.write_skip_line(*stress_index, test_instance.id(), writer)?;
                }
                if self.status_levels.final_status_level >= FinalStatusLevel::Skip {
                    self.final_outputs.push((
                        *stress_index,
                        *test_instance,
                        FinalOutput::Skipped(*reason),
                    ));
                }
            }
            TestEventKind::RunBeginCancel {
                setup_scripts_running,
                current_stats: _,
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
            TestEventKind::RunBeginKill {
                setup_scripts_running,
                current_stats: _,
                running,
                reason,
            } => {
                self.cancel_status = self.cancel_status.max(Some(*reason));

                write!(
                    writer,
                    "{:>12} due to {}",
                    "Killing".style(self.styles.fail),
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
            TestEventKind::InputEnter {
                current_stats,
                running,
                cancel_reason,
            } => {
                // Print everything that would be shown in the progress bar,
                // except for the bar itself.
                writeln!(
                    writer,
                    "{}",
                    progress_str(
                        event.elapsed,
                        current_stats,
                        *running,
                        *cancel_reason,
                        &self.styles,
                    )
                )?;
            }
            TestEventKind::StressSubRunFinished {
                progress,
                sub_elapsed,
                sub_stats,
            } => {
                let stats_summary = sub_stats.summarize_final();
                let summary_style = match stats_summary {
                    FinalRunStats::Success => self.styles.pass,
                    FinalRunStats::NoTestsRun => self.styles.skip,
                    FinalRunStats::Failed(_) | FinalRunStats::Cancelled(_) => self.styles.fail,
                };

                write!(
                    writer,
                    "{:>12} {}",
                    "Stress test".style(summary_style),
                    DisplayBracketedDuration(*sub_elapsed),
                )?;
                match progress {
                    StressProgress::Count {
                        total: StressCount::Count(total),
                        elapsed: _,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {}/{}: ",
                            // We do not add +1 to completed here because it
                            // represents the number of stress runs actually
                            // completed.
                            completed.style(self.styles.count),
                            total.style(self.styles.count),
                        )?;
                    }
                    StressProgress::Count {
                        total: StressCount::Infinite,
                        elapsed: _,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {}: ",
                            // We do not add +1 to completed here because it
                            // represents the number of stress runs actually
                            // completed.
                            completed.style(self.styles.count),
                        )?;
                    }
                    StressProgress::Time {
                        total: _,
                        elapsed: _,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {}: ",
                            // We do not add +1 to completed here because it
                            // represents the number of stress runs actually
                            // completed.
                            completed.style(self.styles.count),
                        )?;
                    }
                }

                write!(
                    writer,
                    "{}",
                    sub_stats.finished_count.style(self.styles.count)
                )?;
                if sub_stats.finished_count != sub_stats.initial_run_count {
                    write!(
                        writer,
                        "/{}",
                        sub_stats.initial_run_count.style(self.styles.count)
                    )?;
                }

                // Both initial and finished counts must be 1 for the singular form.
                let tests_str = plural::tests_plural_if(
                    sub_stats.initial_run_count != 1 || sub_stats.finished_count != 1,
                );

                let mut summary_str = String::new();
                write_summary_str(sub_stats, &self.styles, &mut summary_str);
                writeln!(writer, " {tests_str} run: {summary_str}")?;
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
                    self.final_outputs.sort_by_key(
                        |(stress_index, test_instance, final_output)| {
                            // Use the final status level, reversed (i.e.
                            // failing tests are printed at the very end).
                            (
                                Reverse(final_output.final_status_level()),
                                *stress_index,
                                test_instance.id(),
                            )
                        },
                    );

                    for (stress_index, test_instance, final_output) in &*self.final_outputs {
                        match final_output {
                            FinalOutput::Skipped(_) => {
                                self.write_skip_line(*stress_index, test_instance.id(), writer)?;
                            }
                            FinalOutput::Executed {
                                run_statuses,
                                display_output,
                            } => {
                                let last_status = run_statuses.last_status();

                                self.write_final_status_line(
                                    *stress_index,
                                    test_instance.id(),
                                    run_statuses.describe(),
                                    writer,
                                )?;
                                if *display_output {
                                    self.write_test_execute_status(last_status, false, writer)?;
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
        stress_index: Option<StressIndex>,
        test_instance: TestInstanceId<'a>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
        // same spacing   [   0.034s]
        writeln!(
            writer,
            "[         ] {}",
            self.display_test_instance(stress_index, test_instance)
        )?;

        Ok(())
    }

    fn write_setup_script_status_line(
        &self,
        stress_index: Option<StressIndex>,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        status: &SetupScriptExecuteStatus,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        match status.result {
            ExecutionResult::Pass => {
                write!(writer, "{:>12} ", "SETUP PASS".style(self.styles.pass))?;
            }
            ExecutionResult::Leak { result } => match result {
                LeakTimeoutResult::Pass => {
                    write!(writer, "{:>12} ", "SETUP LEAK".style(self.styles.skip))?;
                }
                LeakTimeoutResult::Fail => {
                    write!(writer, "{:>12} ", "SETUP LKFAIL".style(self.styles.fail))?;
                }
            },
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
            "{}{}",
            DisplayBracketedDuration(status.time_taken),
            self.display_script_instance(stress_index, script_id.clone(), command, args)
        )?;

        Ok(())
    }

    fn write_status_line(
        &self,
        stress_index: Option<StressIndex>,
        test_instance: TestInstance<'a>,
        describe: ExecutionDescription<'_>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let last_status = describe.last_status();
        match describe {
            ExecutionDescription::Success { .. } => {
                if last_status.result
                    == (ExecutionResult::Leak {
                        result: LeakTimeoutResult::Pass,
                    })
                {
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

        // Print the time taken and the name of the test.
        writeln!(
            writer,
            "{}{}",
            DisplayBracketedDuration(last_status.time_taken),
            self.display_test_instance(stress_index, test_instance.id())
        )?;

        // On Windows, also print out the exception if available.
        #[cfg(windows)]
        if let ExecutionResult::Fail {
            failure_status: FailureStatus::Abort(abort_status),
            leaked: _,
        } = last_status.result
        {
            write_windows_message_line(abort_status, &self.styles, writer)?;
        }

        Ok(())
    }

    fn write_final_status_line(
        &self,
        stress_index: Option<StressIndex>,
        test_instance: TestInstanceId<'a>,
        describe: ExecutionDescription<'_>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let last_status = describe.last_status();
        match describe {
            ExecutionDescription::Success { .. } => {
                match (last_status.is_slow, last_status.result) {
                    (
                        true,
                        ExecutionResult::Leak {
                            result: LeakTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "SLOW + LEAK".style(self.styles.skip))?;
                    }
                    (true, ExecutionResult::Pass) => {
                        write!(writer, "{:>12} ", "SLOW".style(self.styles.skip))?;
                    }
                    (
                        false,
                        ExecutionResult::Leak {
                            result: LeakTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "LEAK".style(self.styles.skip))?;
                    }
                    (false, ExecutionResult::Pass) => {
                        write!(writer, "{:>12} ", "PASS".style(self.styles.pass))?;
                    }
                    (_, other) => {
                        unreachable!("success is limited to pass and leak, found {other:?}")
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

        // Next, print the time taken and the name of the test.
        writeln!(
            writer,
            "{}{}",
            DisplayBracketedDuration(last_status.time_taken),
            self.display_test_instance(stress_index, test_instance),
        )?;

        // On Windows, also print out the exception if available.
        #[cfg(windows)]
        if let ExecutionResult::Fail {
            failure_status: FailureStatus::Abort(abort_status),
            leaked: _,
        } = last_status.result
        {
            write_windows_message_line(abort_status, &self.styles, writer)?;
        }

        Ok(())
    }

    fn display_test_instance(
        &self,
        stress_index: Option<StressIndex>,
        instance: TestInstanceId<'a>,
    ) -> DisplayTestInstance<'_> {
        DisplayTestInstance::new(stress_index, instance, &self.styles.list_styles)
    }

    fn display_script_instance(
        &self,
        stress_index: Option<StressIndex>,
        script_id: ScriptId,
        command: &str,
        args: &[String],
    ) -> DisplayScriptInstance {
        DisplayScriptInstance::new(
            stress_index,
            script_id,
            command,
            args,
            self.styles.script_id,
            self.styles.count,
        )
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
                stress_index,
                script_id,
                program,
                args,
                state,
                output,
            }) => {
                // Write the setup script name.
                writeln!(
                    writer,
                    "{}",
                    self.display_script_instance(*stress_index, script_id.clone(), program, args)
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
                    self.unit_output.write_child_execution_output(
                        &self.styles,
                        &self.output_spec_for_info(UnitKind::Script),
                        output,
                        &mut writer,
                    )?;
                }
            }
            InfoResponse::Test(TestInfoResponse {
                stress_index,
                test_instance,
                retry_data,
                state,
                output,
            }) => {
                // Write the test name.
                writeln!(
                    writer,
                    "{}",
                    self.display_test_instance(*stress_index, *test_instance)
                )?;

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
                    self.unit_output.write_child_execution_output(
                        &self.styles,
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
            #[cfg(windows)]
            UnitTerminateMethod::Wait => {
                writeln!(
                    writer,
                    "{}:   waiting for {} to exit on its own; spent {:.3?}s, will terminate \
                     job object after another {:.3?}s",
                    "note".style(self.styles.count),
                    kind,
                    waiting_duration.as_secs_f64(),
                    remaining.as_secs_f64(),
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
            Some(ExecutionResult::Leak {
                result: LeakTimeoutResult::Pass,
            }) => write!(
                writer,
                "{}",
                "passed with leaked handles".style(self.styles.skip)
            ),
            Some(ExecutionResult::Leak {
                result: LeakTimeoutResult::Fail,
            }) => write!(
                writer,
                "{}: exited with code 0, but leaked handles",
                "failed".style(self.styles.fail),
            ),
            Some(ExecutionResult::Timeout) => {
                write!(writer, "{}", "timed out".style(self.styles.fail))
            }
            Some(ExecutionResult::Fail {
                failure_status: FailureStatus::Abort(abort_status),
                // TODO: show leaked info here like in FailureStatus::ExitCode
                // below?
                leaked: _,
            }) => {
                // The errors are shown in the output.
                write!(writer, "{}", "aborted".style(self.styles.fail))?;
                #[cfg(unix)]
                {
                    let AbortStatus::UnixSignal(sig) = abort_status;
                    write!(writer, " with signal {}", sig.style(self.styles.count))?;
                    if let Some(s) = crate::helpers::signal_str(sig) {
                        write!(writer, ": SIG{s}")?;
                    }
                }
                #[cfg(windows)]
                {
                    _ = abort_status;
                }
                Ok(())
            }
            Some(ExecutionResult::Fail {
                failure_status: FailureStatus::ExitCode(code),
                leaked,
            }) => {
                if leaked {
                    write!(
                        writer,
                        "{} with exit code {}, and leaked handles",
                        "failed".style(self.styles.fail),
                        code.style(self.styles.count),
                    )
                } else {
                    write!(
                        writer,
                        "{} with exit code {}",
                        "failed".style(self.styles.fail),
                        code.style(self.styles.count),
                    )
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

    fn write_setup_script_execute_status(
        &self,
        run_status: &SetupScriptExecuteStatus,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_finished(run_status.result, false);
        self.unit_output.write_child_execution_output(
            &self.styles,
            &spec,
            &run_status.output,
            writer,
        )?;

        if show_finished_status_info_line(run_status.result) {
            write!(
                writer,
                // Align with output.
                "    (script ",
            )?;
            self.write_info_execution_result(Some(run_status.result), run_status.is_slow, writer)?;
            writeln!(writer, ")\n")?;
        }

        Ok(())
    }

    fn write_test_execute_status(
        &self,
        run_status: &ExecuteStatus,
        is_retry: bool,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_finished(run_status.result, is_retry);
        self.unit_output.write_child_execution_output(
            &self.styles,
            &spec,
            &run_status.output,
            writer,
        )?;

        if show_finished_status_info_line(run_status.result) {
            write!(
                writer,
                // Align with output.
                "    (test ",
            )?;
            self.write_info_execution_result(Some(run_status.result), run_status.is_slow, writer)?;
            writeln!(writer, ")\n")?;
        }

        Ok(())
    }

    fn output_spec_for_finished(&self, result: ExecutionResult, is_retry: bool) -> ChildOutputSpec {
        let header_style = if is_retry {
            self.styles.retry
        } else if result.is_success() {
            match result {
                ExecutionResult::Leak {
                    result: LeakTimeoutResult::Pass,
                } => self.styles.skip,
                ExecutionResult::Pass => self.styles.pass,
                other => panic!("success means leak-pass or pass, found {other:?}"),
            }
        } else {
            self.styles.fail
        };

        // Adding an hbar at the end gives the text a bit of visual weight that
        // makes it look more balanced. Align it with the end of the header to
        // provide a visual transition from status lines (PASS/FAIL etc) to
        // indented output.
        //
        // With indentation, the output looks like:
        //
        //         FAIL [ .... ]
        //   stdout ───
        //     <test stdout>
        //   stderr ───
        //     <test stderr>
        //
        // Without indentation:
        //
        //         FAIL [ .... ]
        // ── stdout ──
        // <test stdout>
        // ── stderr ──
        // <test stderr>
        let (six_char_start, six_char_end, eight_char_start, eight_char_end, output_indent) =
            if self.no_output_indent {
                (
                    self.theme_characters.hbar(2),
                    self.theme_characters.hbar(2),
                    self.theme_characters.hbar(1),
                    self.theme_characters.hbar(1),
                    "",
                )
            } else {
                (
                    " ".to_owned(),
                    self.theme_characters.hbar(3),
                    " ".to_owned(),
                    self.theme_characters.hbar(1),
                    "    ",
                )
            };

        let stdout_header = format!(
            "{} {} {}",
            six_char_start.style(header_style),
            "stdout".style(header_style),
            six_char_end.style(header_style),
        );
        let stderr_header = format!(
            "{} {} {}",
            six_char_start.style(header_style),
            "stderr".style(header_style),
            six_char_end.style(header_style),
        );
        let combined_header = format!(
            "{} {} {}",
            six_char_start.style(header_style),
            "output".style(header_style),
            six_char_end.style(header_style),
        );
        let exec_fail_header = format!(
            "{} {} {}",
            eight_char_start.style(header_style),
            "execfail".style(header_style),
            eight_char_end.style(header_style),
        );

        ChildOutputSpec {
            kind: UnitKind::Test,
            stdout_header,
            stderr_header,
            combined_header,
            exec_fail_header,
            output_indent,
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
}

const LIBTEST_PANIC_EXIT_CODE: i32 = 101;

// Whether to show a status line for finished units (after STDOUT:/STDERR:).
// This does not apply to info responses which have their own logic.
fn show_finished_status_info_line(result: ExecutionResult) -> bool {
    // Don't show the status line if the exit code is the default from cargo test panicking.
    match result {
        ExecutionResult::Pass => false,
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Pass,
        } => {
            // Show the leaked-handles message
            true
        }
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Fail,
        } => {
            // This is a confusing state without the message at the end.
            true
        }
        ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(code),
            leaked,
        } => {
            // Don't show the status line if the exit code is the default from
            // cargo test panicking, and if there were no leaked handles.
            code != LIBTEST_PANIC_EXIT_CODE && !leaked
        }
        ExecutionResult::Fail {
            failure_status: FailureStatus::Abort(_),
            leaked: _,
        } => {
            // Showing a line at the end aids in clarity.
            true
        }
        ExecutionResult::ExecFail => {
            // This is already shown as an error so there's no reason to show it
            // again.
            false
        }
        ExecutionResult::Timeout => {
            // Show this to be clear what happened.
            true
        }
    }
}

fn status_str(result: ExecutionResult) -> Cow<'static, str> {
    // Max 12 characters here.
    match result {
        #[cfg(unix)]
        ExecutionResult::Fail {
            failure_status: FailureStatus::Abort(AbortStatus::UnixSignal(sig)),
            leaked: _,
        } => match crate::helpers::signal_str(sig) {
            Some(s) => format!("SIG{s}").into(),
            None => format!("ABORT SIG {sig}").into(),
        },
        #[cfg(windows)]
        ExecutionResult::Fail {
            failure_status:
                FailureStatus::Abort(AbortStatus::WindowsNtStatus(_))
                | FailureStatus::Abort(AbortStatus::JobObject),
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(_),
            leaked: true,
        } => "FAIL + LEAK".into(),
        ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(_),
            leaked: false,
        } => "FAIL".into(),
        ExecutionResult::ExecFail => "XFAIL".into(),
        ExecutionResult::Pass => "PASS".into(),
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Pass,
        } => "LEAK".into(),
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Fail,
        } => "LEAK-FAIL".into(),
        ExecutionResult::Timeout => "TIMEOUT".into(),
    }
}

fn short_status_str(result: ExecutionResult) -> Cow<'static, str> {
    // Use shorter strings for this (max 6 characters).
    match result {
        #[cfg(unix)]
        ExecutionResult::Fail {
            failure_status: FailureStatus::Abort(AbortStatus::UnixSignal(sig)),
            leaked: _,
        } => match crate::helpers::signal_str(sig) {
            Some(s) => s.into(),
            None => format!("SIG {sig}").into(),
        },
        #[cfg(windows)]
        ExecutionResult::Fail {
            failure_status:
                FailureStatus::Abort(AbortStatus::WindowsNtStatus(_))
                | FailureStatus::Abort(AbortStatus::JobObject),
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(_),
            leaked: _,
        } => "FAIL".into(),
        ExecutionResult::ExecFail => "XFAIL".into(),
        ExecutionResult::Pass => "PASS".into(),
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Pass,
        } => "LEAK".into(),
        ExecutionResult::Leak {
            result: LeakTimeoutResult::Fail,
        } => "LKFAIL".into(),
        ExecutionResult::Timeout => "TMT".into(),
    }
}

#[cfg(windows)]
fn write_windows_message_line(
    status: AbortStatus,
    styles: &Styles,
    writer: &mut dyn Write,
) -> io::Result<()> {
    match status {
        AbortStatus::WindowsNtStatus(nt_status) => {
            // For subsequent lines, use an indented displayer with {:>12}
            // (ensuring that message lines are aligned).
            const INDENT: &str = "           - ";
            let mut indented = IndentWriter::new_skip_initial(INDENT, writer);
            writeln!(
                indented,
                "{:>12} {} {}",
                "-",
                "with code".style(styles.fail),
                crate::helpers::display_nt_status(nt_status, styles.count)
            )?;
            indented.flush()
        }
        AbortStatus::JobObject => {
            writeln!(
                writer,
                "{:>12} {} via {}",
                "-",
                "terminated".style(styles.fail),
                "job object".style(styles.count),
            )
        }
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
        std::iter::repeat_n(self.hbar, width).collect()
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
        errors::{ChildError, ChildFdError, ChildStartError, ErrorList},
        reporter::events::UnitTerminateReason,
        test_output::{ChildExecutionOutput, ChildOutput, ChildSplitOutput},
    };
    use bytes::Bytes;
    use camino::Utf8PathBuf;
    use chrono::Local;
    use nextest_metadata::RustBinaryId;
    use smol_str::SmolStr;
    use std::{num::NonZero, sync::Arc};

    /// Creates a test reporter with default settings and calls the given function with it.
    ///
    /// Returns the output written to the reporter.
    fn with_reporter<'a, F>(f: F, out: &'a mut Vec<u8>)
    where
        F: FnOnce(DisplayReporter<'a>),
    {
        // Start and end the search at the cwd -- we expect this to not match
        // any results since it'll be the nextest-runner directory.
        let current_dir = Utf8PathBuf::try_from(std::env::current_dir().expect("obtained cwd"))
            .expect("cwd is valid UTF_8");
        let configs = CargoConfigs::new_with_isolation(
            Vec::<String>::new(),
            &current_dir,
            &current_dir,
            Vec::new(),
        )
        .unwrap();

        let builder = DisplayReporterBuilder {
            default_filter: CompiledDefaultFilter::for_default_config(),
            status_levels: StatusLevels {
                status_level: StatusLevel::Fail,
                final_status_level: FinalStatusLevel::Fail,
            },
            test_count: 0,
            success_output: Some(TestOutputDisplay::Immediate),
            failure_output: Some(TestOutputDisplay::Immediate),
            should_colorize: false,
            no_capture: true,
            hide_progress_bar: false,
            no_output_indent: false,
        };
        let output = ReporterStderr::Buffer(out);
        let reporter = builder.build(&configs, output);
        f(reporter);
    }

    #[test]
    fn final_status_line() {
        let binary_id = RustBinaryId::new("my-binary-id");
        let test_instance = TestInstanceId {
            binary_id: &binary_id,
            test_name: "test1",
        };

        let fail_result = ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(1),
            leaked: false,
        };

        let fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 2,
            },
            // output is not relevant here.
            output: make_split_output(Some(fail_result), "", ""),
            result: fail_result,
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
        };
        let fail_describe = ExecutionDescription::Failure {
            first_status: &fail_status,
            last_status: &fail_status,
            retries: &[],
        };

        let flaky_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            // output is not relevant here.
            output: make_split_output(Some(fail_result), "", ""),
            result: ExecutionResult::Pass,
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(2),
            is_slow: false,
            delay_before_start: Duration::ZERO,
        };

        // Make an `ExecutionStatuses` with a failure and a success, indicating flakiness.
        let statuses = ExecutionStatuses::new(vec![fail_status.clone(), flaky_status]);
        let flaky_describe = statuses.describe();

        let mut out = Vec::new();

        with_reporter(
            |mut reporter| {
                // TODO: write a bunch more outputs here.
                reporter
                    .inner
                    .write_final_status_line(
                        None,
                        test_instance,
                        fail_describe,
                        reporter.stderr.buf_mut().unwrap(),
                    )
                    .unwrap();

                reporter
                    .inner
                    .write_final_status_line(
                        Some(StressIndex {
                            current: 1,
                            total: None,
                        }),
                        test_instance,
                        flaky_describe,
                        reporter.stderr.buf_mut().unwrap(),
                    )
                    .unwrap();

                reporter
                    .inner
                    .write_final_status_line(
                        Some(StressIndex {
                            current: 2,
                            total: Some(NonZero::new(3).unwrap()),
                        }),
                        test_instance,
                        flaky_describe,
                        reporter.stderr.buf_mut().unwrap(),
                    )
                    .unwrap();
            },
            &mut out,
        );

        insta::assert_snapshot!(
            "final_status_output",
            String::from_utf8(out).expect("output only consists of UTF-8"),
        );
    }

    // ---

    /// Send an information response to the reporter and return the output.
    #[test]
    fn test_info_response() {
        let args = vec!["arg1".to_string(), "arg2".to_string()];
        let binary_id = RustBinaryId::new("my-binary-id");

        let mut out = Vec::new();

        with_reporter(
            |mut reporter| {
                // Info started event.
                reporter
                    .write_event(&TestEvent {
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
                                leaky_failed: 2,
                                exec_failed: 1,
                                skipped: 5,
                            },
                        },
                    })
                    .unwrap();

                // A basic setup script.
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 0,
                            total: 20,
                            // Technically, you won't get setup script and test responses in the
                            // same response, but it's easiest to test in this manner.
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                stress_index: None,
                                script_id: ScriptId::new(SmolStr::new("setup")).unwrap(),
                                program: "setup".to_owned(),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 1,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                stress_index: None,
                                script_id: ScriptId::new(SmolStr::new("setup-slow")).unwrap(),
                                program: "setup-slow".to_owned(),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 2,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                stress_index: None,
                                script_id: ScriptId::new(SmolStr::new("setup-terminating"))
                                    .unwrap(),
                                program: "setup-terminating".to_owned(),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 3,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                stress_index: Some(StressIndex {
                                    current: 0,
                                    total: None,
                                }),
                                script_id: ScriptId::new(SmolStr::new("setup-exiting")).unwrap(),
                                program: "setup-exiting".to_owned(),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 4,
                            total: 20,
                            response: InfoResponse::SetupScript(SetupScriptInfoResponse {
                                stress_index: Some(StressIndex {
                                    current: 1,
                                    total: Some(NonZero::new(3).unwrap()),
                                }),
                                script_id: ScriptId::new(SmolStr::new("setup-exited")).unwrap(),
                                program: "setup-exited".to_owned(),
                                args: &args,
                                state: UnitState::Exited {
                                    result: ExecutionResult::Fail {
                                        failure_status: FailureStatus::ExitCode(1),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 5,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                stress_index: None,
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 6,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                stress_index: Some(StressIndex {
                                    current: 0,
                                    total: None,
                                }),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 7,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                stress_index: None,
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 8,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                stress_index: Some(StressIndex {
                                    current: 1,
                                    total: Some(NonZero::new(3).unwrap()),
                                }),
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
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::InfoResponse {
                            index: 9,
                            total: 20,
                            response: InfoResponse::Test(TestInfoResponse {
                                stress_index: None,
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
                    .write_event(&TestEvent {
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

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut out = Vec::new();

        with_reporter(
            |reporter| {
                assert!(reporter.inner.no_capture, "no_capture is true");
                assert_eq!(
                    reporter.inner.unit_output.force_failure_output(),
                    Some(TestOutputDisplay::Never),
                    "failure output is never, overriding other settings"
                );
                assert_eq!(
                    reporter.inner.unit_output.force_success_output(),
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
}

#[cfg(all(windows, test))]
mod windows_tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{STATUS_CONTROL_C_EXIT, STATUS_CONTROL_STACK_VIOLATION},
        Globalization::SetThreadUILanguage,
    };

    #[test]
    fn test_write_windows_message_line() {
        unsafe {
            // Set the thread UI language to US English for consistent output.
            SetThreadUILanguage(0x0409);
        }

        insta::assert_snapshot!(
            "ctrl_c_code",
            to_message_line(AbortStatus::WindowsNtStatus(STATUS_CONTROL_C_EXIT))
        );
        insta::assert_snapshot!(
            "stack_violation_code",
            to_message_line(AbortStatus::WindowsNtStatus(STATUS_CONTROL_STACK_VIOLATION)),
        );
        insta::assert_snapshot!("job_object", to_message_line(AbortStatus::JobObject));
    }

    #[track_caller]
    fn to_message_line(status: AbortStatus) -> String {
        let mut buf = Vec::new();
        write_windows_message_line(status, &Styles::default(), &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }
}
