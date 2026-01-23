// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints out and aggregates test execution statuses.
//!
//! The main structure in this module is [`TestReporter`].

use super::{
    ChildOutputSpec, FinalStatusLevel, OutputStoreFinal, StatusLevel, StatusLevels,
    UnitOutputReporter,
    formatters::{
        DisplayBracketedDuration, DisplayDurationBy, DisplaySlowDuration, DisplayUnitKind,
        write_final_warnings, write_skip_counts,
    },
    progress::{
        MaxProgressRunning, ProgressBarState, progress_bar_msg, progress_str, write_summary_str,
    },
    unit_output::TestOutputDisplay,
};
use crate::{
    config::{
        elements::{LeakTimeoutResult, SlowTimeoutResult},
        overrides::CompiledDefaultFilter,
        scripts::ScriptId,
    },
    errors::WriteEventError,
    helpers::{
        DisplayCounterIndex, DisplayScriptInstance, DisplayTestInstance, ThemeCharacters, plural,
        usize_decimal_char_width,
    },
    indenter::indented,
    list::TestInstanceId,
    record::{ReplayHeader, ShortestRunIdPrefix},
    reporter::{
        displayer::{
            ShowProgress,
            formatters::DisplayHhMmSs,
            progress::{ShowTerminalProgress, TerminalProgress},
        },
        events::*,
        helpers::Styles,
        imp::ReporterOutput,
    },
    run_mode::NextestRunMode,
    runner::StressCount,
    test_output::ChildSingleOutput,
    write_str::WriteStr,
};
use debug_ignore::DebugIgnore;
use nextest_metadata::MismatchReason;
use owo_colors::OwoColorize;
use std::{
    borrow::Cow,
    cmp::Reverse,
    io::{self, BufWriter, IsTerminal, Write},
    time::Duration,
};

pub(crate) struct DisplayReporterBuilder {
    pub(crate) mode: NextestRunMode,
    pub(crate) default_filter: CompiledDefaultFilter,
    pub(crate) status_levels: StatusLevels,
    pub(crate) test_count: usize,
    pub(crate) success_output: Option<TestOutputDisplay>,
    pub(crate) failure_output: Option<TestOutputDisplay>,
    pub(crate) should_colorize: bool,
    pub(crate) no_capture: bool,
    pub(crate) verbose: bool,
    pub(crate) show_progress: ShowProgress,
    pub(crate) no_output_indent: bool,
    pub(crate) max_progress_running: MaxProgressRunning,
    pub(crate) show_term_progress: ShowTerminalProgress,
}

impl DisplayReporterBuilder {
    pub(crate) fn build<'a>(self, output: ReporterOutput<'a>) -> DisplayReporter<'a> {
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
        match &output {
            ReporterOutput::Terminal => {
                if supports_unicode::on(supports_unicode::Stream::Stderr) {
                    theme_characters.use_unicode();
                }
            }
            ReporterOutput::Writer { use_unicode, .. } => {
                if *use_unicode {
                    theme_characters.use_unicode();
                }
            }
        }

        let mut show_progress_bar = false;

        let output = match output {
            ReporterOutput::Terminal => {
                let is_terminal = io::stderr().is_terminal();
                let progress_bar = self.progress_bar(
                    is_terminal,
                    theme_characters.progress_chars(),
                    self.max_progress_running,
                );
                let term_progress = TerminalProgress::new(self.show_term_progress);

                show_progress_bar = progress_bar
                    .as_ref()
                    .map(|progress_bar| !progress_bar.is_hidden())
                    .unwrap_or_default();

                ReporterOutputImpl::Terminal {
                    progress_bar: progress_bar.map(Box::new),
                    term_progress,
                }
            }
            ReporterOutput::Writer { writer, .. } => ReporterOutputImpl::Writer(writer),
        };

        // success_output is meaningless if the runner isn't capturing any
        // output. However, failure output is still meaningful for exec fail
        // events.
        let force_success_output = match self.no_capture {
            true => Some(TestOutputDisplay::Never),
            false => self.success_output,
        };
        let force_failure_output = match self.no_capture {
            true => Some(TestOutputDisplay::Never),
            false => self.failure_output,
        };
        let force_exec_fail_output = match self.no_capture {
            true => Some(TestOutputDisplay::Immediate),
            false => self.failure_output,
        };

        let show_counter = match self.show_progress {
            ShowProgress::Auto => is_ci::uncached() || !show_progress_bar,
            ShowProgress::Running | ShowProgress::None => false,
            ShowProgress::Counter => true,
        };
        let counter_width = show_counter.then_some(usize_decimal_char_width(self.test_count));

        DisplayReporter {
            inner: DisplayReporterImpl {
                mode: self.mode,
                default_filter: self.default_filter,
                status_levels: StatusLevels {
                    status_level,
                    final_status_level: self.status_levels.final_status_level,
                },
                no_capture: self.no_capture,
                verbose: self.verbose,
                no_output_indent: self.no_output_indent,
                counter_width,
                styles,
                theme_characters,
                cancel_status: None,
                unit_output: UnitOutputReporter::new(
                    force_success_output,
                    force_failure_output,
                    force_exec_fail_output,
                ),
                final_outputs: DebugIgnore(Vec::new()),
                run_id_unique_prefix: None,
            },
            output,
        }
    }

    fn progress_bar(
        &self,
        is_terminal: bool,
        progress_chars: &'static str,
        max_progress_running: MaxProgressRunning,
    ) -> Option<ProgressBarState> {
        if self.no_capture {
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
            return None;
        }

        if is_ci::uncached() {
            // Some CI environments appear to pretend to be a terminal.
            // Disable the progress bar in these environments.
            return None;
        }

        // If this is not a terminal, don't enable the progress bar. indicatif
        // also has this logic internally, but we do this check outside so we
        // know whether we're writing to an external buffer or to indicatif.
        if !is_terminal {
            return None;
        }

        match self.show_progress {
            ShowProgress::None | ShowProgress::Counter => None,
            ShowProgress::Auto | ShowProgress::Running => {
                let state = ProgressBarState::new(
                    self.mode,
                    self.test_count,
                    progress_chars,
                    max_progress_running,
                );
                Some(state)
            }
        }
    }
}

/// Functionality to report test results to stderr, JUnit, and/or structured,
/// machine-readable results to stdout.
pub(crate) struct DisplayReporter<'a> {
    inner: DisplayReporterImpl<'a>,
    output: ReporterOutputImpl<'a>,
}

impl<'a> DisplayReporter<'a> {
    pub(crate) fn tick(&mut self) {
        self.output.tick(&self.inner.styles);
    }

    pub(crate) fn write_event(&mut self, event: &TestEvent<'a>) -> Result<(), WriteEventError> {
        match &mut self.output {
            ReporterOutputImpl::Terminal {
                progress_bar,
                term_progress,
            } => {
                if let Some(term_progress) = term_progress {
                    term_progress.update_progress(event);
                }

                if let Some(state) = progress_bar {
                    // Write to a string that will be printed as a log line.
                    let mut buf = String::new();
                    self.inner
                        .write_event_impl(event, &mut buf)
                        .map_err(WriteEventError::Io)?;

                    state.update_progress_bar(event, &self.inner.styles);
                    state.write_buf(&buf);
                    Ok(())
                } else {
                    // Write to a buffered stderr.
                    let mut writer = BufWriter::new(std::io::stderr());
                    self.inner
                        .write_event_impl(event, &mut writer)
                        .map_err(WriteEventError::Io)?;
                    writer.flush().map_err(WriteEventError::Io)
                }
            }
            ReporterOutputImpl::Writer(writer) => {
                self.inner
                    .write_event_impl(event, *writer)
                    .map_err(WriteEventError::Io)?;
                writer.write_str_flush().map_err(WriteEventError::Io)
            }
        }
    }

    pub(crate) fn finish(&mut self) {
        self.output.finish_and_clear_bar();
    }

    /// Sets the unique prefix for the run ID.
    ///
    /// This is used to highlight the unique prefix portion of the run ID
    /// in the `RunStarted` output when a recording session is active.
    pub(crate) fn set_run_id_unique_prefix(&mut self, prefix: ShortestRunIdPrefix) {
        self.inner.run_id_unique_prefix = Some(prefix);
    }

    /// Writes a replay header to the output.
    ///
    /// This is used by `ReplayReporter` to display replay-specific information
    /// before processing recorded events.
    pub(crate) fn write_replay_header(
        &mut self,
        header: &ReplayHeader,
    ) -> Result<(), WriteEventError> {
        self.write_impl(|writer, styles, _theme_chars| {
            // Write "Replaying" line with unique prefix highlighting.
            write!(writer, "{:>12} ", "Replaying".style(styles.pass))?;
            let run_id_display = if let Some(prefix_info) = &header.unique_prefix {
                // Highlight the unique prefix portion of the full run ID.
                format!(
                    "{}{}",
                    prefix_info.prefix.style(styles.run_id_prefix),
                    prefix_info.rest.style(styles.run_id_rest),
                )
            } else {
                // No prefix info available, show the full ID without highlighting.
                header.run_id.to_string().style(styles.count).to_string()
            };
            writeln!(writer, "recorded run {}", run_id_display)?;

            // Write "Started" line with status.
            let status_str = header.status.short_status_str();
            write!(writer, "{:>12} ", "Started".style(styles.pass))?;
            writeln!(
                writer,
                "{}  status: {}",
                header.started_at.format("%Y-%m-%d %H:%M:%S"),
                status_str.style(styles.count)
            )?;

            Ok(())
        })
    }

    /// Internal helper for writing through the output with access to styles.
    fn write_impl<F>(&mut self, f: F) -> Result<(), WriteEventError>
    where
        F: FnOnce(&mut dyn WriteStr, &Styles, &ThemeCharacters) -> io::Result<()>,
    {
        match &mut self.output {
            ReporterOutputImpl::Terminal { progress_bar, .. } => {
                if let Some(state) = progress_bar {
                    // Write to a string that will be printed as a log line.
                    let mut buf = String::new();
                    f(&mut buf, &self.inner.styles, &self.inner.theme_characters)
                        .map_err(WriteEventError::Io)?;
                    state.write_buf(&buf);
                    Ok(())
                } else {
                    // Write to a buffered stderr.
                    let mut writer = BufWriter::new(std::io::stderr());
                    f(
                        &mut writer,
                        &self.inner.styles,
                        &self.inner.theme_characters,
                    )
                    .map_err(WriteEventError::Io)?;
                    writer.flush().map_err(WriteEventError::Io)
                }
            }
            ReporterOutputImpl::Writer(writer) => {
                f(*writer, &self.inner.styles, &self.inner.theme_characters)
                    .map_err(WriteEventError::Io)?;
                writer.write_str_flush().map_err(WriteEventError::Io)
            }
        }
    }
}

enum ReporterOutputImpl<'a> {
    Terminal {
        // Reporter-specific progress bar state. None if the progress bar is not
        // enabled (which can include the terminal not being a TTY).
        progress_bar: Option<Box<ProgressBarState>>,
        // OSC 9 code progress reporting.
        term_progress: Option<TerminalProgress>,
    },
    Writer(&'a mut (dyn WriteStr + Send)),
}

impl ReporterOutputImpl<'_> {
    fn tick(&mut self, styles: &Styles) {
        match self {
            ReporterOutputImpl::Terminal {
                progress_bar,
                term_progress,
            } => {
                if let Some(state) = progress_bar {
                    state.tick(styles);
                }
                if let Some(term_progress) = term_progress {
                    // In this case, write the last value directly to stderr.
                    // This is a very small amount of data so buffering is not
                    // required. It also doesn't have newlines or any visible
                    // text, so it can be directly written out to stderr without
                    // going through the progress bar (which screws up
                    // indicatif's calculations).
                    eprint!("{}", term_progress.last_value())
                }
            }
            ReporterOutputImpl::Writer(_) => {
                // No ticking for writers.
            }
        }
    }

    fn finish_and_clear_bar(&self) {
        match self {
            ReporterOutputImpl::Terminal {
                progress_bar,
                term_progress,
            } => {
                if let Some(state) = progress_bar {
                    state.finish_and_clear();
                }
                if let Some(term_progress) = term_progress {
                    // The last value is expected to be Remove.
                    eprint!("{}", term_progress.last_value())
                }
            }
            ReporterOutputImpl::Writer(_) => {
                // No progress bar to clear.
            }
        }
    }

    #[cfg(test)]
    fn writer_mut(&mut self) -> Option<&mut (dyn WriteStr + Send)> {
        match self {
            Self::Writer(writer) => Some(*writer),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum FinalOutput {
    Skipped(#[expect(dead_code)] MismatchReason),
    Executed {
        run_statuses: ExecutionStatuses<ChildSingleOutput>,
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

struct FinalOutputEntry<'a> {
    stress_index: Option<StressIndex>,
    counter: TestInstanceCounter,
    instance: TestInstanceId<'a>,
    output: FinalOutput,
}

impl<'a> PartialEq for FinalOutputEntry<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl<'a> Eq for FinalOutputEntry<'a> {}

impl<'a> PartialOrd for FinalOutputEntry<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for FinalOutputEntry<'a> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Use the final status level, reversed (i.e.
        // failing tests are printed at the very end).
        (
            Reverse(self.output.final_status_level()),
            self.stress_index,
            self.counter,
            self.instance,
        )
            .cmp(&(
                Reverse(other.output.final_status_level()),
                other.stress_index,
                other.counter,
                other.instance,
            ))
    }
}

struct DisplayReporterImpl<'a> {
    mode: NextestRunMode,
    default_filter: CompiledDefaultFilter,
    status_levels: StatusLevels,
    no_capture: bool,
    verbose: bool,
    no_output_indent: bool,
    // None if no counter is displayed. If a counter is displayed, this is the
    // width of the total number of tests to run.
    counter_width: Option<usize>,
    styles: Box<Styles>,
    theme_characters: ThemeCharacters,
    cancel_status: Option<CancelReason>,
    unit_output: UnitOutputReporter,
    final_outputs: DebugIgnore<Vec<FinalOutputEntry<'a>>>,
    // The unique prefix for the current run ID, if a recording session is active.
    // Used for highlighting the run ID in RunStarted output.
    run_id_unique_prefix: Option<ShortestRunIdPrefix>,
}

impl<'a> DisplayReporterImpl<'a> {
    fn write_event_impl(
        &mut self,
        event: &TestEvent<'a>,
        writer: &mut dyn WriteStr,
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

                // Display the run ID with unique prefix highlighting if a recording
                // session is active, otherwise use plain styling.
                let run_id_display = if let Some(prefix_info) = &self.run_id_unique_prefix {
                    format!(
                        "{}{}",
                        prefix_info.prefix.style(self.styles.run_id_prefix),
                        prefix_info.rest.style(self.styles.run_id_rest),
                    )
                } else {
                    run_id.style(self.styles.count).to_string()
                };

                writeln!(
                    writer,
                    "ID {} with nextest profile: {}",
                    run_id_display,
                    profile_name.style(self.styles.count),
                )?;

                write!(writer, "{:>12} ", "Starting".style(self.styles.pass))?;

                let count_style = self.styles.count;

                let tests_str = plural::tests_str(self.mode, test_list.run_count());
                let binaries_str = plural::binaries_str(test_list.listed_binary_count());

                write!(
                    writer,
                    "{} {tests_str} across {} {binaries_str}",
                    test_list.run_count().style(count_style),
                    test_list.listed_binary_count().style(count_style),
                )?;

                write_skip_counts(
                    self.mode,
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
                        total: StressCount::Count { count },
                        elapsed,
                        completed,
                    } => {
                        write!(
                            writer,
                            "iteration {}/{} ({} elapsed so far",
                            (completed + 1).style(self.styles.count),
                            count.style(self.styles.count),
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
                                ", {} {} remaining",
                                n.style(self.styles.count),
                                plural::iterations_str(n.get()),
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
                current_stats,
                command_line,
                ..
            } => {
                // In no-capture and verbose modes, print out a test start
                // event.
                if self.no_capture || self.verbose {
                    // The spacing is to align test instances.
                    writeln!(
                        writer,
                        "{:>12} [         ] {}",
                        "START".style(self.styles.pass),
                        self.display_test_instance(
                            *stress_index,
                            TestInstanceCounter::Counter {
                                // --no-capture implies tests being run
                                // serially, so the current test is the number
                                // of finished tests plus one.
                                current: current_stats.finished_count + 1,
                                total: current_stats.initial_run_count,
                            },
                            *test_instance
                        ),
                    )?;
                }

                if self.verbose {
                    self.write_command_line(command_line, writer)?;
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
                    self.display_test_instance(
                        *stress_index,
                        TestInstanceCounter::Padded,
                        *test_instance
                    )
                )?;
            }

            TestEventKind::TestAttemptFailedWillRetry {
                stress_index,
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
                running: _,
            } => {
                if self.status_levels.status_level >= StatusLevel::Retry {
                    let try_status_string = format!(
                        "TRY {} {}",
                        run_status.retry_data.attempt,
                        short_status_str(&run_status.result),
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
                        self.display_test_instance(
                            *stress_index,
                            TestInstanceCounter::Padded,
                            *test_instance
                        )
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
                            self.display_test_instance(
                                *stress_index,
                                TestInstanceCounter::Padded,
                                *test_instance
                            )
                        )?;
                    }
                }
            }
            TestEventKind::TestRetryStarted {
                stress_index,
                test_instance,
                retry_data: RetryData { attempt, .. },
                running: _,
                command_line,
            } => {
                // In no-capture and verbose modes, print out a retry start event.
                if self.no_capture || self.verbose {
                    let retry_string = format!("TRY {attempt} START");
                    writeln!(
                        writer,
                        "{:>12} [         ] {}",
                        retry_string.style(self.styles.retry),
                        self.display_test_instance(
                            *stress_index,
                            TestInstanceCounter::Padded,
                            *test_instance
                        )
                    )?;
                }

                if self.verbose {
                    self.write_command_line(command_line, writer)?;
                }
            }
            TestEventKind::TestFinished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                run_statuses,
                current_stats,
                ..
            } => {
                let describe = run_statuses.describe();
                let last_status = run_statuses.last_status();
                let test_output_display = match last_status.result {
                    ExecutionResultDescription::Pass
                    | ExecutionResultDescription::Timeout {
                        result: SlowTimeoutResult::Pass,
                    }
                    | ExecutionResultDescription::Leak {
                        result: LeakTimeoutResult::Pass,
                    } => self.unit_output.success_output(*success_output),
                    ExecutionResultDescription::Leak {
                        result: LeakTimeoutResult::Fail,
                    }
                    | ExecutionResultDescription::Timeout {
                        result: SlowTimeoutResult::Fail,
                    }
                    | ExecutionResultDescription::Fail { .. } => {
                        self.unit_output.failure_output(*failure_output)
                    }
                    ExecutionResultDescription::ExecFail => {
                        self.unit_output.exec_fail_output(*failure_output)
                    }
                };

                let output_on_test_finished = self.status_levels.compute_output_on_test_finished(
                    test_output_display,
                    self.cancel_status,
                    describe.status_level(),
                    describe.final_status_level(),
                    &last_status.result,
                );

                let counter = TestInstanceCounter::Counter {
                    current: current_stats.finished_count,
                    total: current_stats.initial_run_count,
                };

                if output_on_test_finished.write_status_line {
                    self.write_status_line(
                        *stress_index,
                        counter,
                        *test_instance,
                        describe,
                        writer,
                    )?;
                }
                if output_on_test_finished.show_immediate {
                    self.write_test_execute_status(last_status, false, writer)?;
                }
                if let OutputStoreFinal::Yes { display_output } =
                    output_on_test_finished.store_final
                {
                    self.final_outputs.push(FinalOutputEntry {
                        stress_index: *stress_index,
                        counter,
                        instance: *test_instance,
                        output: FinalOutput::Executed {
                            run_statuses: run_statuses.clone(),
                            display_output,
                        },
                    });
                }
            }
            TestEventKind::TestSkipped {
                stress_index,
                test_instance,
                reason,
            } => {
                if self.status_levels.status_level >= StatusLevel::Skip {
                    self.write_skip_line(*stress_index, *test_instance, writer)?;
                }
                if self.status_levels.final_status_level >= FinalStatusLevel::Skip {
                    self.final_outputs.push(FinalOutputEntry {
                        stress_index: *stress_index,
                        counter: TestInstanceCounter::Padded,
                        instance: *test_instance,
                        output: FinalOutput::Skipped(*reason),
                    });
                }
            }
            TestEventKind::RunBeginCancel {
                setup_scripts_running,
                current_stats,
                running,
            } => {
                self.cancel_status = self.cancel_status.max(current_stats.cancel_reason);

                write!(writer, "{:>12} ", "Cancelling".style(self.styles.fail))?;
                if let Some(reason) = current_stats.cancel_reason {
                    write!(
                        writer,
                        "due to {}: ",
                        reason.to_static_str().style(self.styles.fail)
                    )?;
                }

                let immediately_terminating_text =
                    if current_stats.cancel_reason == Some(CancelReason::TestFailureImmediate) {
                        format!("immediately {} ", "terminating".style(self.styles.fail))
                    } else {
                        String::new()
                    };

                // At the moment, we can have either setup scripts or tests running, but not both.
                if *setup_scripts_running > 0 {
                    let s = plural::setup_scripts_str(*setup_scripts_running);
                    write!(
                        writer,
                        "{immediately_terminating_text}{} {s} still running",
                        setup_scripts_running.style(self.styles.count),
                    )?;
                } else if *running > 0 {
                    let tests_str = plural::tests_str(self.mode, *running);
                    write!(
                        writer,
                        "{immediately_terminating_text}{} {tests_str} still running",
                        running.style(self.styles.count),
                    )?;
                }
                writeln!(writer)?;
            }
            TestEventKind::RunBeginKill {
                setup_scripts_running,
                current_stats,
                running,
            } => {
                self.cancel_status = self.cancel_status.max(current_stats.cancel_reason);

                write!(writer, "{:>12} ", "Killing".style(self.styles.fail),)?;
                if let Some(reason) = current_stats.cancel_reason {
                    write!(
                        writer,
                        "due to {}: ",
                        reason.to_static_str().style(self.styles.fail)
                    )?;
                }

                // At the moment, we can have either setup scripts or tests running, but not both.
                if *setup_scripts_running > 0 {
                    let s = plural::setup_scripts_str(*setup_scripts_running);
                    write!(
                        writer,
                        ": {} {s} still running",
                        setup_scripts_running.style(self.styles.count),
                    )?;
                } else if *running > 0 {
                    let tests_str = plural::tests_str(self.mode, *running);
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
                    let tests_str = plural::tests_str(self.mode, *running);
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
                    let tests_str = plural::tests_str(self.mode, *running);
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
            } => {
                // Print everything that would be shown in the progress bar,
                // except for the bar itself.
                writeln!(
                    writer,
                    "{}",
                    progress_str(event.elapsed, current_stats, *running, &self.styles)
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
                    FinalRunStats::Failed { .. } | FinalRunStats::Cancelled { .. } => {
                        self.styles.fail
                    }
                };

                write!(
                    writer,
                    "{:>12} {}",
                    "Stress test".style(summary_style),
                    DisplayBracketedDuration(*sub_elapsed),
                )?;
                match progress {
                    StressProgress::Count {
                        total: StressCount::Count { count },
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
                            count.style(self.styles.count),
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
                    self.mode,
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
                outstanding_not_seen: tests_not_seen,
                ..
            } => {
                match run_stats {
                    RunFinishedStats::Single(run_stats) => {
                        let stats_summary = run_stats.summarize_final();
                        let summary_style = match stats_summary {
                            FinalRunStats::Success => self.styles.pass,
                            FinalRunStats::NoTestsRun => self.styles.skip,
                            FinalRunStats::Failed { .. } | FinalRunStats::Cancelled { .. } => {
                                self.styles.fail
                            }
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
                            self.mode,
                            run_stats.initial_run_count != 1 || run_stats.finished_count != 1,
                        );

                        let mut summary_str = String::new();
                        write_summary_str(run_stats, &self.styles, &mut summary_str);
                        writeln!(writer, " {tests_str} run: {summary_str}")?;
                    }
                    RunFinishedStats::Stress(stats) => {
                        let stats_summary = stats.summarize_final();
                        let summary_style = match stats_summary {
                            StressFinalRunStats::Success => self.styles.pass,
                            StressFinalRunStats::NoTestsRun => self.styles.skip,
                            StressFinalRunStats::Cancelled | StressFinalRunStats::Failed => {
                                self.styles.fail
                            }
                        };

                        write!(
                            writer,
                            "{}\n{:>12} ",
                            self.theme_characters.hbar(12),
                            "Summary".style(summary_style),
                        )?;

                        // Next, print the total time taken.
                        // * > means right-align.
                        // * 8 is the number of characters to pad to.
                        // * .3 means print two digits after the decimal point.
                        write!(writer, "[{:>8.3?}s] ", elapsed.as_secs_f64())?;

                        write!(
                            writer,
                            "{}",
                            stats.completed.current.style(self.styles.count),
                        )?;
                        let iterations_str = if let Some(total) = stats.completed.total {
                            write!(writer, "/{}", total.style(self.styles.count))?;
                            plural::iterations_str(total.get())
                        } else {
                            plural::iterations_str(stats.completed.current)
                        };
                        write!(
                            writer,
                            " stress run {iterations_str}: {} {}",
                            stats.success_count.style(self.styles.count),
                            "passed".style(self.styles.pass),
                        )?;
                        if stats.failed_count > 0 {
                            write!(
                                writer,
                                ", {} {}",
                                stats.failed_count.style(self.styles.count),
                                "failed".style(self.styles.fail),
                            )?;
                        }

                        match stats.last_final_stats {
                            FinalRunStats::Cancelled { reason, kind: _ } => {
                                if let Some(reason) = reason {
                                    write!(
                                        writer,
                                        "; cancelled due to {}",
                                        reason.to_static_str().style(self.styles.fail),
                                    )?;
                                }
                            }
                            FinalRunStats::Failed { .. }
                            | FinalRunStats::Success
                            | FinalRunStats::NoTestsRun => {}
                        }

                        writeln!(writer)?;
                    }
                }

                // Don't print out test outputs after Ctrl-C, but *do* print them after SIGTERM or
                // SIGHUP since those tend to be automated tasks performing kills.
                if self.cancel_status < Some(CancelReason::Interrupt) {
                    // Sort the final outputs for a friendlier experience.
                    self.final_outputs.sort_unstable();

                    for entry in &*self.final_outputs {
                        match &entry.output {
                            FinalOutput::Skipped(_) => {
                                self.write_skip_line(entry.stress_index, entry.instance, writer)?;
                            }
                            FinalOutput::Executed {
                                run_statuses,
                                display_output,
                            } => {
                                let last_status = run_statuses.last_status();

                                self.write_final_status_line(
                                    entry.stress_index,
                                    entry.counter,
                                    entry.instance,
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

                if let Some(not_seen) = tests_not_seen
                    && not_seen.total_not_seen > 0
                {
                    writeln!(
                        writer,
                        "{:>12} {} outstanding {} not seen during this rerun:",
                        "Note".style(self.styles.skip),
                        not_seen.total_not_seen.style(self.styles.count),
                        plural::tests_str(self.mode, not_seen.total_not_seen),
                    )?;

                    for t in &not_seen.not_seen {
                        let display = DisplayTestInstance::new(
                            None,
                            None,
                            t.as_ref(),
                            &self.styles.list_styles,
                        );
                        writeln!(writer, "             {}", display)?;
                    }

                    let remaining = not_seen
                        .total_not_seen
                        .saturating_sub(not_seen.not_seen.len());
                    if remaining > 0 {
                        writeln!(
                            writer,
                            "             ... and {} more {}",
                            remaining.style(self.styles.count),
                            plural::tests_str(self.mode, remaining),
                        )?;
                    }
                }

                // Print out warnings at the end, if any.
                write_final_warnings(self.mode, run_stats.final_stats(), &self.styles, writer)?;
            }
        }

        Ok(())
    }

    fn write_skip_line(
        &self,
        stress_index: Option<StressIndex>,
        test_instance: TestInstanceId<'a>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
        // same spacing   [   0.034s]
        writeln!(
            writer,
            "[         ] {}",
            self.display_test_instance(stress_index, TestInstanceCounter::Padded, test_instance)
        )?;

        Ok(())
    }

    fn write_setup_script_status_line(
        &self,
        stress_index: Option<StressIndex>,
        script_id: &ScriptId,
        command: &str,
        args: &[String],
        status: &SetupScriptExecuteStatus<ChildSingleOutput>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        match status.result {
            ExecutionResultDescription::Pass => {
                write!(writer, "{:>12} ", "SETUP PASS".style(self.styles.pass))?;
            }
            ExecutionResultDescription::Leak { result } => match result {
                LeakTimeoutResult::Pass => {
                    write!(writer, "{:>12} ", "SETUP LEAK".style(self.styles.skip))?;
                }
                LeakTimeoutResult::Fail => {
                    write!(writer, "{:>12} ", "SETUP LKFAIL".style(self.styles.fail))?;
                }
            },
            ref other => {
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
        counter: TestInstanceCounter,
        test_instance: TestInstanceId<'a>,
        describe: ExecutionDescription<'_, ChildSingleOutput>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        self.write_status_line_impl(
            stress_index,
            counter,
            test_instance,
            describe,
            StatusLineKind::Intermediate,
            writer,
        )
    }

    fn write_final_status_line(
        &self,
        stress_index: Option<StressIndex>,
        counter: TestInstanceCounter,
        test_instance: TestInstanceId<'a>,
        describe: ExecutionDescription<'_, ChildSingleOutput>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        self.write_status_line_impl(
            stress_index,
            counter,
            test_instance,
            describe,
            StatusLineKind::Final,
            writer,
        )
    }

    fn write_status_line_impl(
        &self,
        stress_index: Option<StressIndex>,
        counter: TestInstanceCounter,
        test_instance: TestInstanceId<'a>,
        describe: ExecutionDescription<'_, ChildSingleOutput>,
        kind: StatusLineKind,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let last_status = describe.last_status();

        // Write the status prefix (e.g., "PASS", "FAIL", "FLAKY 2/3").
        self.write_status_line_prefix(describe, kind, writer)?;

        // Write the duration and test instance.
        writeln!(
            writer,
            "{}{}",
            DisplayBracketedDuration(last_status.time_taken),
            self.display_test_instance(stress_index, counter, test_instance),
        )?;

        // For Windows aborts, print out the exception code on a separate line.
        if let ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort { ref abort },
            leaked: _,
        } = last_status.result
        {
            write_windows_abort_line(abort, &self.styles, writer)?;
        }

        Ok(())
    }

    fn write_status_line_prefix(
        &self,
        describe: ExecutionDescription<'_, ChildSingleOutput>,
        kind: StatusLineKind,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let last_status = describe.last_status();
        match describe {
            ExecutionDescription::Success { .. } => {
                // Exhaustive match on (is_slow, result) to catch missing cases
                // at compile time. For intermediate status lines, is_slow is
                // ignored (shown via separate SLOW lines during execution).
                match (kind, last_status.is_slow, &last_status.result) {
                    // Final + slow variants.
                    (StatusLineKind::Final, true, ExecutionResultDescription::Pass) => {
                        write!(writer, "{:>12} ", "SLOW".style(self.styles.skip))?;
                    }
                    (
                        StatusLineKind::Final,
                        true,
                        ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "SLOW + LEAK".style(self.styles.skip))?;
                    }
                    (
                        StatusLineKind::Final,
                        true,
                        ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "SLOW+TMPASS".style(self.styles.skip))?;
                    }
                    // Non-slow variants (or intermediate where is_slow is ignored).
                    (_, _, ExecutionResultDescription::Pass) => {
                        write!(writer, "{:>12} ", "PASS".style(self.styles.pass))?;
                    }
                    (
                        _,
                        _,
                        ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "LEAK".style(self.styles.skip))?;
                    }
                    (
                        _,
                        _,
                        ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Pass,
                        },
                    ) => {
                        write!(writer, "{:>12} ", "TIMEOUT-PASS".style(self.styles.skip))?;
                    }
                    // These are failure cases and cannot appear in Success.
                    (
                        _,
                        _,
                        ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Fail,
                        },
                    )
                    | (
                        _,
                        _,
                        ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Fail,
                        },
                    )
                    | (_, _, ExecutionResultDescription::Fail { .. })
                    | (_, _, ExecutionResultDescription::ExecFail) => {
                        unreachable!(
                            "success description cannot have failure result: {:?}",
                            last_status.result
                        )
                    }
                }
            }
            ExecutionDescription::Flaky { .. } => {
                // Use the skip color to also represent a flaky test.
                let status = match kind {
                    StatusLineKind::Intermediate => {
                        format!("TRY {} PASS", last_status.retry_data.attempt)
                    }
                    StatusLineKind::Final => {
                        format!(
                            "FLAKY {}/{}",
                            last_status.retry_data.attempt, last_status.retry_data.total_attempts
                        )
                    }
                };
                write!(writer, "{:>12} ", status.style(self.styles.skip))?;
            }
            ExecutionDescription::Failure { .. } => {
                if last_status.retry_data.attempt == 1 {
                    write!(
                        writer,
                        "{:>12} ",
                        status_str(&last_status.result).style(self.styles.fail)
                    )?;
                } else {
                    let status_str = short_status_str(&last_status.result);
                    write!(
                        writer,
                        "{:>12} ",
                        format!("TRY {} {}", last_status.retry_data.attempt, status_str)
                            .style(self.styles.fail)
                    )?;
                }
            }
        }
        Ok(())
    }

    fn display_test_instance(
        &self,
        stress_index: Option<StressIndex>,
        counter: TestInstanceCounter,
        instance: TestInstanceId<'a>,
    ) -> DisplayTestInstance<'_> {
        let counter_index = match (counter, self.counter_width) {
            (TestInstanceCounter::Counter { current, total }, Some(_)) => {
                Some(DisplayCounterIndex::new_counter(current, total))
            }
            (TestInstanceCounter::Padded, Some(counter_width)) => Some(
                DisplayCounterIndex::new_padded(self.theme_characters.hbar_char(), counter_width),
            ),
            (TestInstanceCounter::None, _) | (_, None) => None,
        };

        DisplayTestInstance::new(
            stress_index,
            counter_index,
            instance,
            &self.styles.list_styles,
        )
    }

    fn write_command_line(
        &self,
        command_line: &[String],
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        // Indent under START (13 spaces + "command").
        writeln!(
            writer,
            "{:>20}: {}",
            "command".style(self.styles.count),
            shell_words::join(command_line),
        )
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
        writer: &mut dyn WriteStr,
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
        let count_width = usize_decimal_char_width(index + 1) + usize_decimal_char_width(total) + 3;
        let padding = 8usize.saturating_sub(count_width);

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
        let mut writer = indented(writer).with_str("  ").skip_initial();

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
                    self.display_test_instance(
                        *stress_index,
                        TestInstanceCounter::None,
                        *test_instance
                    )
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

        writer.write_str_flush()?;
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
        writer: &mut dyn WriteStr,
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
                    "{status_str}: {attempt_str}{} {} for {:.3?}s as PID {}",
                    DisplayUnitKind::new(self.mode, kind),
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
                write!(
                    writer,
                    "{status_str}: {attempt_str}{} ",
                    DisplayUnitKind::new(self.mode, kind)
                )?;

                let tentative_desc = tentative_result.map(ExecutionResultDescription::from);
                self.write_info_execution_result(
                    tentative_desc.as_ref(),
                    slow_after.is_some(),
                    writer,
                )?;
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
                        "{}:   spent {:.3?}s waiting for {} PID {} to shut down, \
                         will mark as leaky after another {:.3?}s",
                        "note".style(self.styles.count),
                        waiting_duration.as_secs_f64(),
                        DisplayUnitKind::new(self.mode, kind),
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
                write!(
                    writer,
                    "{status_str}: {attempt_str}{} ",
                    DisplayUnitKind::new(self.mode, kind)
                )?;
                let result_desc = ExecutionResultDescription::from(*result);
                self.write_info_execution_result(Some(&result_desc), slow_after.is_some(), writer)?;
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
                write!(
                    writer,
                    "{status_str}: {attempt_str}{} ",
                    DisplayUnitKind::new(self.mode, kind)
                )?;
                let previous_desc = ExecutionResultDescription::from(*previous_result);
                self.write_info_execution_result(Some(&previous_desc), *previous_slow, writer)?;
                writeln!(
                    writer,
                    ", currently {} before next attempt",
                    "waiting".style(self.styles.count)
                )?;
                writeln!(
                    writer,
                    "{}:   waited {:.3?}s so far, will wait another {:.3?}s before retrying {}",
                    "note".style(self.styles.count),
                    waiting_duration.as_secs_f64(),
                    remaining.as_secs_f64(),
                    DisplayUnitKind::new(self.mode, kind),
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
        writer: &mut dyn WriteStr,
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
            "{}: {attempt_str}{} {} PID {} due to {} ({} ran for {:.3?}s)",
            "status".style(self.styles.count),
            "terminating".style(self.styles.fail),
            DisplayUnitKind::new(self.mode, kind),
            pid.style(self.styles.count),
            reason.style(self.styles.count),
            DisplayUnitKind::new(self.mode, kind),
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
                    DisplayUnitKind::new(self.mode, kind),
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
                    DisplayUnitKind::new(self.mode, kind),
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
                    DisplayUnitKind::new(self.mode, kind),
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
        result: Option<&ExecutionResultDescription>,
        is_slow: bool,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        match result {
            Some(ExecutionResultDescription::Pass) => {
                let style = if is_slow {
                    self.styles.skip
                } else {
                    self.styles.pass
                };

                write!(writer, "{}", "passed".style(style))
            }
            Some(ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Pass,
            }) => write!(
                writer,
                "{}",
                "passed with leaked handles".style(self.styles.skip)
            ),
            Some(ExecutionResultDescription::Leak {
                result: LeakTimeoutResult::Fail,
            }) => write!(
                writer,
                "{}: exited with code 0, but leaked handles",
                "failed".style(self.styles.fail),
            ),
            Some(ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Pass,
            }) => {
                write!(writer, "{}", "passed with timeout".style(self.styles.skip))
            }
            Some(ExecutionResultDescription::Timeout {
                result: SlowTimeoutResult::Fail,
            }) => {
                write!(writer, "{}", "timed out".style(self.styles.fail))
            }
            Some(ExecutionResultDescription::Fail {
                failure: FailureDescription::Abort { abort },
                // TODO: show leaked info here like in FailureDescription::ExitCode
                // below?
                leaked: _,
            }) => {
                // The errors are shown in the output.
                write!(writer, "{}", "aborted".style(self.styles.fail))?;
                // AbortDescription is platform-independent and contains display
                // info. Note that Windows descriptions are handled separately,
                // in write_windows_abort_suffix.
                if let AbortDescription::UnixSignal { signal, name } = abort {
                    write!(writer, " with signal {}", signal.style(self.styles.count))?;
                    if let Some(s) = name {
                        write!(writer, ": SIG{s}")?;
                    }
                }
                Ok(())
            }
            Some(ExecutionResultDescription::Fail {
                failure: FailureDescription::ExitCode { code },
                leaked,
            }) => {
                if *leaked {
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
            Some(ExecutionResultDescription::ExecFail) => {
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
        run_status: &SetupScriptExecuteStatus<ChildSingleOutput>,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_finished(&run_status.result, false);
        self.unit_output.write_child_execution_output(
            &self.styles,
            &spec,
            &run_status.output,
            writer,
        )?;

        if show_finished_status_info_line(&run_status.result) {
            write!(
                writer,
                // Align with output.
                "    (script ",
            )?;
            self.write_info_execution_result(Some(&run_status.result), run_status.is_slow, writer)?;
            writeln!(writer, ")\n")?;
        }

        Ok(())
    }

    fn write_test_execute_status(
        &self,
        run_status: &ExecuteStatus<ChildSingleOutput>,
        is_retry: bool,
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let spec = self.output_spec_for_finished(&run_status.result, is_retry);
        self.unit_output.write_child_execution_output(
            &self.styles,
            &spec,
            &run_status.output,
            writer,
        )?;

        if show_finished_status_info_line(&run_status.result) {
            write!(
                writer,
                // Align with output.
                "    (test ",
            )?;
            self.write_info_execution_result(Some(&run_status.result), run_status.is_slow, writer)?;
            writeln!(writer, ")\n")?;
        }

        Ok(())
    }

    fn output_spec_for_finished(
        &self,
        result: &ExecutionResultDescription,
        is_retry: bool,
    ) -> ChildOutputSpec {
        let header_style = if is_retry {
            self.styles.retry
        } else {
            match result {
                ExecutionResultDescription::Pass => self.styles.pass,
                ExecutionResultDescription::Leak {
                    result: LeakTimeoutResult::Pass,
                } => self.styles.skip,
                ExecutionResultDescription::Leak {
                    result: LeakTimeoutResult::Fail,
                } => self.styles.fail,
                ExecutionResultDescription::Timeout {
                    result: SlowTimeoutResult::Pass,
                } => self.styles.skip,
                ExecutionResultDescription::Timeout {
                    result: SlowTimeoutResult::Fail,
                } => self.styles.fail,
                ExecutionResultDescription::Fail { .. } => self.styles.fail,
                ExecutionResultDescription::ExecFail => self.styles.fail,
            }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TestInstanceCounter {
    Counter { current: usize, total: usize },
    Padded,
    None,
}

/// Whether a status line is an intermediate line (during execution) or a final
/// line (in the summary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusLineKind {
    /// Intermediate status line shown during test execution.
    Intermediate,
    /// Final status line shown in the summary.
    Final,
}

const LIBTEST_PANIC_EXIT_CODE: i32 = 101;

// Whether to show a status line for finished units (after STDOUT:/STDERR:).
// This does not apply to info responses which have their own logic.
fn show_finished_status_info_line(result: &ExecutionResultDescription) -> bool {
    // Don't show the status line if the exit code is the default from cargo test panicking.
    match result {
        ExecutionResultDescription::Pass => false,
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        } => {
            // Show the leaked-handles message.
            true
        }
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Fail,
        } => {
            // This is a confusing state without the message at the end.
            true
        }
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { code },
            leaked,
        } => {
            // Don't show the status line if the exit code is the default from
            // cargo test panicking, and if there were no leaked handles.
            *code != LIBTEST_PANIC_EXIT_CODE && !leaked
        }
        ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort { .. },
            leaked: _,
        } => {
            // Showing a line at the end aids in clarity.
            true
        }
        ExecutionResultDescription::ExecFail => {
            // This is already shown as an error so there's no reason to show it
            // again.
            false
        }
        ExecutionResultDescription::Timeout { .. } => {
            // Show this to be clear what happened.
            true
        }
    }
}

fn status_str(result: &ExecutionResultDescription) -> Cow<'static, str> {
    // Max 12 characters here.
    match result {
        ExecutionResultDescription::Fail {
            failure:
                FailureDescription::Abort {
                    abort: AbortDescription::UnixSignal { signal, name },
                },
            leaked: _,
        } => match name {
            Some(s) => format!("SIG{s}").into(),
            None => format!("ABORT SIG {signal}").into(),
        },
        ExecutionResultDescription::Fail {
            failure:
                FailureDescription::Abort {
                    abort: AbortDescription::WindowsNtStatus { .. },
                }
                | FailureDescription::Abort {
                    abort: AbortDescription::WindowsJobObject,
                },
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { .. },
            leaked: true,
        } => "FAIL + LEAK".into(),
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { .. },
            leaked: false,
        } => "FAIL".into(),
        ExecutionResultDescription::ExecFail => "XFAIL".into(),
        ExecutionResultDescription::Pass => "PASS".into(),
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        } => "LEAK".into(),
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Fail,
        } => "LEAK-FAIL".into(),
        ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Pass,
        } => "TIMEOUT-PASS".into(),
        ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Fail,
        } => "TIMEOUT".into(),
    }
}

fn short_status_str(result: &ExecutionResultDescription) -> Cow<'static, str> {
    // Use shorter strings for this (max 6 characters).
    match result {
        ExecutionResultDescription::Fail {
            failure:
                FailureDescription::Abort {
                    abort: AbortDescription::UnixSignal { signal, name },
                },
            leaked: _,
        } => match name {
            Some(s) => s.to_string().into(),
            None => format!("SIG {signal}").into(),
        },
        ExecutionResultDescription::Fail {
            failure:
                FailureDescription::Abort {
                    abort: AbortDescription::WindowsNtStatus { .. },
                }
                | FailureDescription::Abort {
                    abort: AbortDescription::WindowsJobObject,
                },
            leaked: _,
        } => {
            // Going to print out the full error message on the following line -- just "ABORT" will
            // do for now.
            "ABORT".into()
        }
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { .. },
            leaked: true,
        } => "FL+LK".into(),
        ExecutionResultDescription::Fail {
            failure: FailureDescription::ExitCode { .. },
            leaked: false,
        } => "FAIL".into(),
        ExecutionResultDescription::ExecFail => "XFAIL".into(),
        ExecutionResultDescription::Pass => "PASS".into(),
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Pass,
        } => "LEAK".into(),
        ExecutionResultDescription::Leak {
            result: LeakTimeoutResult::Fail,
        } => "LKFAIL".into(),
        ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Pass,
        } => "TMPASS".into(),
        ExecutionResultDescription::Timeout {
            result: SlowTimeoutResult::Fail,
        } => "TMT".into(),
    }
}

/// Writes a supplementary line for Windows abort statuses.
///
/// For Unix signals, this is a no-op since the signal info is displayed inline.
fn write_windows_abort_line(
    status: &AbortDescription,
    styles: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    match status {
        AbortDescription::UnixSignal { .. } => {
            // Unix signal info is displayed inline, no separate line needed.
            Ok(())
        }
        AbortDescription::WindowsNtStatus { code, message } => {
            // For subsequent lines, use an indented displayer with {:>12}
            // (ensuring that message lines are aligned).
            const INDENT: &str = "           - ";
            let mut indented = indented(writer).with_str(INDENT).skip_initial();
            // Format code as 10 characters ("0x" + 8 hex digits) for uniformity.
            let code_str = format!("{:#010x}", code.style(styles.count));
            let status_str = match message {
                Some(msg) => format!("{code_str}: {msg}"),
                None => code_str,
            };
            writeln!(
                indented,
                "{:>12} {} {}",
                "-",
                "with code".style(styles.fail),
                status_str,
            )?;
            indented.write_str_flush()
        }
        AbortDescription::WindowsJobObject => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::{ChildError, ChildFdError, ChildStartError, ErrorList},
        reporter::events::{
            ChildExecutionOutputDescription, ExecutionResult, FailureStatus, UnitTerminateReason,
        },
        test_output::{ChildExecutionOutput, ChildOutput, ChildSingleOutput, ChildSplitOutput},
    };
    use bytes::Bytes;
    use chrono::Local;
    use nextest_metadata::{RustBinaryId, TestCaseName};
    use quick_junit::ReportUuid;
    use smol_str::SmolStr;
    use std::{num::NonZero, sync::Arc};

    /// Creates a test reporter with default settings and calls the given function with it.
    ///
    /// Returns the output written to the reporter.
    fn with_reporter<'a, F>(f: F, out: &'a mut String)
    where
        F: FnOnce(DisplayReporter<'a>),
    {
        with_reporter_impl(f, out, false)
    }

    /// Creates a test reporter with verbose mode enabled.
    fn with_verbose_reporter<'a, F>(f: F, out: &'a mut String)
    where
        F: FnOnce(DisplayReporter<'a>),
    {
        with_reporter_impl(f, out, true)
    }

    fn with_reporter_impl<'a, F>(f: F, out: &'a mut String, verbose: bool)
    where
        F: FnOnce(DisplayReporter<'a>),
    {
        let builder = DisplayReporterBuilder {
            mode: NextestRunMode::Test,
            default_filter: CompiledDefaultFilter::for_default_config(),
            status_levels: StatusLevels {
                status_level: StatusLevel::Fail,
                final_status_level: FinalStatusLevel::Fail,
            },
            test_count: 5000,
            success_output: Some(TestOutputDisplay::Immediate),
            failure_output: Some(TestOutputDisplay::Immediate),
            should_colorize: false,
            no_capture: true,
            verbose,
            show_progress: ShowProgress::Counter,
            no_output_indent: false,
            max_progress_running: MaxProgressRunning::default(),
            show_term_progress: ShowTerminalProgress::No,
        };

        let output = ReporterOutput::Writer {
            writer: out,
            use_unicode: true,
        };
        let reporter = builder.build(output);
        f(reporter);
    }

    #[test]
    fn final_status_line() {
        let binary_id = RustBinaryId::new("my-binary-id");
        let test_name = TestCaseName::new("test1");
        let test_instance = TestInstanceId {
            binary_id: &binary_id,
            test_name: &test_name,
        };

        let fail_result_internal = ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(1),
            leaked: false,
        };
        let fail_result = ExecutionResultDescription::from(fail_result_internal);

        let fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 2,
            },
            // output is not relevant here.
            output: make_split_output(Some(fail_result_internal), "", ""),
            result: fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
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
            output: make_split_output(Some(fail_result_internal), "", ""),
            result: ExecutionResultDescription::Pass,
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(2),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // Make an `ExecutionStatuses` with a failure and a success, indicating flakiness.
        let statuses = ExecutionStatuses::new(vec![fail_status.clone(), flaky_status]);
        let flaky_describe = statuses.describe();

        let mut out = String::new();

        with_reporter(
            |mut reporter| {
                // TODO: write a bunch more outputs here.
                reporter
                    .inner
                    .write_final_status_line(
                        None,
                        TestInstanceCounter::None,
                        test_instance,
                        fail_describe,
                        reporter.output.writer_mut().unwrap(),
                    )
                    .unwrap();

                reporter
                    .inner
                    .write_final_status_line(
                        Some(StressIndex {
                            current: 1,
                            total: None,
                        }),
                        TestInstanceCounter::Padded,
                        test_instance,
                        flaky_describe,
                        reporter.output.writer_mut().unwrap(),
                    )
                    .unwrap();

                reporter
                    .inner
                    .write_final_status_line(
                        Some(StressIndex {
                            current: 2,
                            total: Some(NonZero::new(3).unwrap()),
                        }),
                        TestInstanceCounter::Counter {
                            current: 20,
                            total: 5000,
                        },
                        test_instance,
                        flaky_describe,
                        reporter.output.writer_mut().unwrap(),
                    )
                    .unwrap();
            },
            &mut out,
        );

        insta::assert_snapshot!("final_status_output", out,);
    }

    #[test]
    fn status_line_all_variants() {
        let binary_id = RustBinaryId::new("my-binary-id");
        let test_name = TestCaseName::new("test_name");
        let test_instance = TestInstanceId {
            binary_id: &binary_id,
            test_name: &test_name,
        };

        // --- Success result types ---
        let pass_result_internal = ExecutionResult::Pass;
        let pass_result = ExecutionResultDescription::from(pass_result_internal);

        let leak_pass_result_internal = ExecutionResult::Leak {
            result: LeakTimeoutResult::Pass,
        };
        let leak_pass_result = ExecutionResultDescription::from(leak_pass_result_internal);

        let timeout_pass_result_internal = ExecutionResult::Timeout {
            result: SlowTimeoutResult::Pass,
        };
        let timeout_pass_result = ExecutionResultDescription::from(timeout_pass_result_internal);

        // --- Failure result types ---
        let fail_result_internal = ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(1),
            leaked: false,
        };
        let fail_result = ExecutionResultDescription::from(fail_result_internal);

        let fail_leak_result_internal = ExecutionResult::Fail {
            failure_status: FailureStatus::ExitCode(1),
            leaked: true,
        };
        let fail_leak_result = ExecutionResultDescription::from(fail_leak_result_internal);

        let exec_fail_result_internal = ExecutionResult::ExecFail;
        let exec_fail_result = ExecutionResultDescription::from(exec_fail_result_internal);

        let leak_fail_result_internal = ExecutionResult::Leak {
            result: LeakTimeoutResult::Fail,
        };
        let leak_fail_result = ExecutionResultDescription::from(leak_fail_result_internal);

        let timeout_fail_result_internal = ExecutionResult::Timeout {
            result: SlowTimeoutResult::Fail,
        };
        let timeout_fail_result = ExecutionResultDescription::from(timeout_fail_result_internal);

        // Construct abort results directly as ExecutionResultDescription (platform-independent).
        let abort_unix_result = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::UnixSignal {
                    signal: 11,
                    name: Some("SEGV".into()),
                },
            },
            leaked: false,
        };
        let abort_windows_result = ExecutionResultDescription::Fail {
            failure: FailureDescription::Abort {
                abort: AbortDescription::WindowsNtStatus {
                    // STATUS_ACCESS_VIOLATION = 0xC0000005
                    code: 0xC0000005_u32 as i32,
                    message: Some("Access violation".into()),
                },
            },
            leaked: false,
        };

        // --- Success statuses (is_slow = false) ---
        let pass_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(pass_result_internal), "", ""),
            result: pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let leak_pass_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(leak_pass_result_internal), "", ""),
            result: leak_pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(2),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let timeout_pass_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(timeout_pass_result_internal), "", ""),
            result: timeout_pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(240),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // --- Success statuses (is_slow = true) ---
        let pass_slow_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(pass_result_internal), "", ""),
            result: pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(30),
            is_slow: true,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let leak_pass_slow_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(leak_pass_result_internal), "", ""),
            result: leak_pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(30),
            is_slow: true,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let timeout_pass_slow_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(timeout_pass_result_internal), "", ""),
            result: timeout_pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(300),
            is_slow: true,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // --- Flaky statuses ---
        let flaky_first_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 2,
            },
            output: make_split_output(Some(fail_result_internal), "", ""),
            result: fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };
        let flaky_last_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            output: make_split_output(Some(pass_result_internal), "", ""),
            result: pass_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // --- First-attempt failure statuses ---
        let fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(fail_result_internal), "", ""),
            result: fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let fail_leak_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(fail_leak_result_internal), "", ""),
            result: fail_leak_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let exec_fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(exec_fail_result_internal), "", ""),
            result: exec_fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let leak_fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(leak_fail_result_internal), "", ""),
            result: leak_fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let timeout_fail_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(Some(timeout_fail_result_internal), "", ""),
            result: timeout_fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(60),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let abort_unix_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(None, "", ""),
            result: abort_unix_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let abort_windows_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 1,
                total_attempts: 1,
            },
            output: make_split_output(None, "", ""),
            result: abort_windows_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // --- Retry failure statuses ---
        let fail_retry_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            output: make_split_output(Some(fail_result_internal), "", ""),
            result: fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let fail_leak_retry_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            output: make_split_output(Some(fail_leak_result_internal), "", ""),
            result: fail_leak_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let leak_fail_retry_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            output: make_split_output(Some(leak_fail_result_internal), "", ""),
            result: leak_fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(1),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        let timeout_fail_retry_status = ExecuteStatus {
            retry_data: RetryData {
                attempt: 2,
                total_attempts: 2,
            },
            output: make_split_output(Some(timeout_fail_result_internal), "", ""),
            result: timeout_fail_result.clone(),
            start_time: Local::now().into(),
            time_taken: Duration::from_secs(60),
            is_slow: false,
            delay_before_start: Duration::ZERO,
            error_summary: None,
            output_error_slice: None,
        };

        // --- Build descriptions ---
        let pass_describe = ExecutionDescription::Success {
            single_status: &pass_status,
        };
        let leak_pass_describe = ExecutionDescription::Success {
            single_status: &leak_pass_status,
        };
        let timeout_pass_describe = ExecutionDescription::Success {
            single_status: &timeout_pass_status,
        };
        let pass_slow_describe = ExecutionDescription::Success {
            single_status: &pass_slow_status,
        };
        let leak_pass_slow_describe = ExecutionDescription::Success {
            single_status: &leak_pass_slow_status,
        };
        let timeout_pass_slow_describe = ExecutionDescription::Success {
            single_status: &timeout_pass_slow_status,
        };
        let flaky_describe = ExecutionDescription::Flaky {
            last_status: &flaky_last_status,
            prior_statuses: std::slice::from_ref(&flaky_first_status),
        };
        let fail_describe = ExecutionDescription::Failure {
            first_status: &fail_status,
            last_status: &fail_status,
            retries: &[],
        };
        let fail_leak_describe = ExecutionDescription::Failure {
            first_status: &fail_leak_status,
            last_status: &fail_leak_status,
            retries: &[],
        };
        let exec_fail_describe = ExecutionDescription::Failure {
            first_status: &exec_fail_status,
            last_status: &exec_fail_status,
            retries: &[],
        };
        let leak_fail_describe = ExecutionDescription::Failure {
            first_status: &leak_fail_status,
            last_status: &leak_fail_status,
            retries: &[],
        };
        let timeout_fail_describe = ExecutionDescription::Failure {
            first_status: &timeout_fail_status,
            last_status: &timeout_fail_status,
            retries: &[],
        };
        let abort_unix_describe = ExecutionDescription::Failure {
            first_status: &abort_unix_status,
            last_status: &abort_unix_status,
            retries: &[],
        };
        let abort_windows_describe = ExecutionDescription::Failure {
            first_status: &abort_windows_status,
            last_status: &abort_windows_status,
            retries: &[],
        };
        let fail_retry_describe = ExecutionDescription::Failure {
            first_status: &fail_status,
            last_status: &fail_retry_status,
            retries: std::slice::from_ref(&fail_retry_status),
        };
        let fail_leak_retry_describe = ExecutionDescription::Failure {
            first_status: &fail_leak_status,
            last_status: &fail_leak_retry_status,
            retries: std::slice::from_ref(&fail_leak_retry_status),
        };
        let leak_fail_retry_describe = ExecutionDescription::Failure {
            first_status: &leak_fail_status,
            last_status: &leak_fail_retry_status,
            retries: std::slice::from_ref(&leak_fail_retry_status),
        };
        let timeout_fail_retry_describe = ExecutionDescription::Failure {
            first_status: &timeout_fail_status,
            last_status: &timeout_fail_retry_status,
            retries: std::slice::from_ref(&timeout_fail_retry_status),
        };

        // Collect all test cases: (label, description).
        // The label helps identify each case in the snapshot.
        let test_cases: Vec<(&str, ExecutionDescription<'_, ChildSingleOutput>)> = vec![
            // Success variants (is_slow = false).
            ("pass", pass_describe),
            ("leak pass", leak_pass_describe),
            ("timeout pass", timeout_pass_describe),
            // Success variants (is_slow = true) - only different for Final.
            ("pass slow", pass_slow_describe),
            ("leak pass slow", leak_pass_slow_describe),
            ("timeout pass slow", timeout_pass_slow_describe),
            // Flaky variant.
            ("flaky", flaky_describe),
            // First-attempt failure variants.
            ("fail", fail_describe),
            ("fail leak", fail_leak_describe),
            ("exec fail", exec_fail_describe),
            ("leak fail", leak_fail_describe),
            ("timeout fail", timeout_fail_describe),
            ("abort unix", abort_unix_describe),
            ("abort windows", abort_windows_describe),
            // Retry failure variants.
            ("fail retry", fail_retry_describe),
            ("fail leak retry", fail_leak_retry_describe),
            ("leak fail retry", leak_fail_retry_describe),
            ("timeout fail retry", timeout_fail_retry_describe),
        ];

        let mut out = String::new();
        let mut counter = 0usize;

        with_reporter(
            |mut reporter| {
                let writer = reporter.output.writer_mut().unwrap();

                // Loop over both StatusLineKind variants.
                for (kind_name, kind) in [
                    ("intermediate", StatusLineKind::Intermediate),
                    ("final", StatusLineKind::Final),
                ] {
                    writeln!(writer, "=== {kind_name} ===").unwrap();

                    for (label, describe) in &test_cases {
                        counter += 1;
                        let test_counter = TestInstanceCounter::Counter {
                            current: counter,
                            total: 100,
                        };

                        // Write label as a comment for clarity in snapshot.
                        writeln!(writer, "# {label}: ").unwrap();

                        reporter
                            .inner
                            .write_status_line_impl(
                                None,
                                test_counter,
                                test_instance,
                                *describe,
                                kind,
                                writer,
                            )
                            .unwrap();
                    }
                }
            },
            &mut out,
        );

        insta::assert_snapshot!("status_line_all_variants", out);
    }

    #[test]
    fn test_summary_line() {
        let run_id = ReportUuid::nil();
        let mut out = String::new();

        with_reporter(
            |mut reporter| {
                // Test single run with all passing tests
                let run_stats_success = RunStats {
                    initial_run_count: 5,
                    finished_count: 5,
                    setup_scripts_initial_count: 0,
                    setup_scripts_finished_count: 0,
                    setup_scripts_passed: 0,
                    setup_scripts_failed: 0,
                    setup_scripts_exec_failed: 0,
                    setup_scripts_timed_out: 0,
                    passed: 5,
                    passed_slow: 0,
                    passed_timed_out: 0,
                    flaky: 0,
                    failed: 0,
                    failed_slow: 0,
                    failed_timed_out: 0,
                    leaky: 0,
                    leaky_failed: 0,
                    exec_failed: 0,
                    skipped: 0,
                    cancel_reason: None,
                };

                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::RunFinished {
                            run_id,
                            start_time: Local::now().into(),
                            elapsed: Duration::from_secs(2),
                            run_stats: RunFinishedStats::Single(run_stats_success),
                            outstanding_not_seen: None,
                        },
                    })
                    .unwrap();

                // Test single run with mixed results
                let run_stats_mixed = RunStats {
                    initial_run_count: 10,
                    finished_count: 8,
                    setup_scripts_initial_count: 1,
                    setup_scripts_finished_count: 1,
                    setup_scripts_passed: 1,
                    setup_scripts_failed: 0,
                    setup_scripts_exec_failed: 0,
                    setup_scripts_timed_out: 0,
                    passed: 5,
                    passed_slow: 1,
                    passed_timed_out: 2,
                    flaky: 1,
                    failed: 2,
                    failed_slow: 0,
                    failed_timed_out: 1,
                    leaky: 1,
                    leaky_failed: 0,
                    exec_failed: 1,
                    skipped: 2,
                    cancel_reason: Some(CancelReason::Signal),
                };

                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::RunFinished {
                            run_id,
                            start_time: Local::now().into(),
                            elapsed: Duration::from_millis(15750),
                            run_stats: RunFinishedStats::Single(run_stats_mixed),
                            outstanding_not_seen: None,
                        },
                    })
                    .unwrap();

                // Test stress run with success
                let stress_stats_success = StressRunStats {
                    completed: StressIndex {
                        current: 25,
                        total: Some(NonZero::new(50).unwrap()),
                    },
                    success_count: 25,
                    failed_count: 0,
                    last_final_stats: FinalRunStats::Success,
                };

                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::RunFinished {
                            run_id,
                            start_time: Local::now().into(),
                            elapsed: Duration::from_secs(120),
                            run_stats: RunFinishedStats::Stress(stress_stats_success),
                            outstanding_not_seen: None,
                        },
                    })
                    .unwrap();

                // Test stress run with failures and cancellation
                let stress_stats_failed = StressRunStats {
                    completed: StressIndex {
                        current: 15,
                        total: None, // Unlimited iterations
                    },
                    success_count: 12,
                    failed_count: 3,
                    last_final_stats: FinalRunStats::Cancelled {
                        reason: Some(CancelReason::Interrupt),
                        kind: RunStatsFailureKind::SetupScript,
                    },
                };

                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::RunFinished {
                            run_id,
                            start_time: Local::now().into(),
                            elapsed: Duration::from_millis(45250),
                            run_stats: RunFinishedStats::Stress(stress_stats_failed),
                            outstanding_not_seen: None,
                        },
                    })
                    .unwrap();

                // Test no tests run case
                let run_stats_empty = RunStats {
                    initial_run_count: 0,
                    finished_count: 0,
                    setup_scripts_initial_count: 0,
                    setup_scripts_finished_count: 0,
                    setup_scripts_passed: 0,
                    setup_scripts_failed: 0,
                    setup_scripts_exec_failed: 0,
                    setup_scripts_timed_out: 0,
                    passed: 0,
                    passed_slow: 0,
                    passed_timed_out: 0,
                    flaky: 0,
                    failed: 0,
                    failed_slow: 0,
                    failed_timed_out: 0,
                    leaky: 0,
                    leaky_failed: 0,
                    exec_failed: 0,
                    skipped: 0,
                    cancel_reason: None,
                };

                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::RunFinished {
                            run_id,
                            start_time: Local::now().into(),
                            elapsed: Duration::from_millis(100),
                            run_stats: RunFinishedStats::Single(run_stats_empty),
                            outstanding_not_seen: None,
                        },
                    })
                    .unwrap();
            },
            &mut out,
        );

        insta::assert_snapshot!("summary_line_output", out,);
    }

    // ---

    /// Send an information response to the reporter and return the output.
    #[test]
    fn test_info_response() {
        let args = vec!["arg1".to_string(), "arg2".to_string()];
        let binary_id = RustBinaryId::new("my-binary-id");
        let test_name1 = TestCaseName::new("test1");
        let test_name2 = TestCaseName::new("test2");
        let test_name3 = TestCaseName::new("test3");
        let test_name4 = TestCaseName::new("test4");

        let mut out = String::new();

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
                                passed_timed_out: 3,
                                flaky: 2,
                                failed: 2,
                                failed_slow: 1,
                                failed_timed_out: 1,
                                leaky: 1,
                                leaky_failed: 2,
                                exec_failed: 1,
                                skipped: 5,
                                cancel_reason: None,
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
                                args: args.clone(),
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
                                args: args.clone(),
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
                                args: args.clone(),
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
                                args: args.clone(),
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
                                ))
                                .into(),
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
                                args: args.clone(),
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
                                ))
                                .into(),
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
                                    test_name: &test_name1,
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
                                    test_name: &test_name2,
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
                                    test_name: &test_name3,
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
                                    test_name: &test_name4,
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
                                    test_name: &test_name4,
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

        insta::assert_snapshot!("info_response_output", out,);
    }

    fn make_split_output(
        result: Option<ExecutionResult>,
        stdout: &str,
        stderr: &str,
    ) -> ChildExecutionOutputDescription<ChildSingleOutput> {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: Some(Bytes::from(stdout.to_owned()).into()),
                stderr: Some(Bytes::from(stderr.to_owned()).into()),
            }),
            errors: None,
        }
        .into()
    }

    fn make_split_output_with_errors(
        result: Option<ExecutionResult>,
        stdout: &str,
        stderr: &str,
        errors: Vec<ChildError>,
    ) -> ChildExecutionOutputDescription<ChildSingleOutput> {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: Some(Bytes::from(stdout.to_owned()).into()),
                stderr: Some(Bytes::from(stderr.to_owned()).into()),
            }),
            errors: ErrorList::new("testing split output", errors),
        }
        .into()
    }

    fn make_combined_output_with_errors(
        result: Option<ExecutionResult>,
        output: &str,
        errors: Vec<ChildError>,
    ) -> ChildExecutionOutputDescription<ChildSingleOutput> {
        ChildExecutionOutput::Output {
            result,
            output: ChildOutput::Combined {
                output: Bytes::from(output.to_owned()).into(),
            },
            errors: ErrorList::new("testing split output", errors),
        }
        .into()
    }

    #[test]
    fn verbose_command_line() {
        let binary_id = RustBinaryId::new("my-binary-id");
        let test_name = TestCaseName::new("test_name");
        let test_with_spaces = TestCaseName::new("test_with_spaces");
        let test_special_chars = TestCaseName::new("test_special_chars");
        let test_retry = TestCaseName::new("test_retry");
        let mut out = String::new();

        with_verbose_reporter(
            |mut reporter| {
                let current_stats = RunStats {
                    initial_run_count: 10,
                    finished_count: 0,
                    ..Default::default()
                };

                // Test a simple command.
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::TestStarted {
                            stress_index: None,
                            test_instance: TestInstanceId {
                                binary_id: &binary_id,
                                test_name: &test_name,
                            },
                            current_stats,
                            running: 1,
                            command_line: vec![
                                "/path/to/binary".to_string(),
                                "--exact".to_string(),
                                "test_name".to_string(),
                            ],
                        },
                    })
                    .unwrap();

                // Test a command with arguments that need quoting.
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::TestStarted {
                            stress_index: None,
                            test_instance: TestInstanceId {
                                binary_id: &binary_id,
                                test_name: &test_with_spaces,
                            },
                            current_stats,
                            running: 2,
                            command_line: vec![
                                "/path/to/binary".to_string(),
                                "--exact".to_string(),
                                "test with spaces".to_string(),
                                "--flag=value".to_string(),
                            ],
                        },
                    })
                    .unwrap();

                // Test a command with special characters.
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::TestStarted {
                            stress_index: None,
                            test_instance: TestInstanceId {
                                binary_id: &binary_id,
                                test_name: &test_special_chars,
                            },
                            current_stats,
                            running: 3,
                            command_line: vec![
                                "/path/to/binary".to_string(),
                                "test\"with\"quotes".to_string(),
                                "test'with'single".to_string(),
                            ],
                        },
                    })
                    .unwrap();

                // Test a retry (attempt 2) - should show "TRY 2 START".
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::TestRetryStarted {
                            stress_index: None,
                            test_instance: TestInstanceId {
                                binary_id: &binary_id,
                                test_name: &test_retry,
                            },
                            retry_data: RetryData {
                                attempt: 2,
                                total_attempts: 3,
                            },
                            running: 1,
                            command_line: vec![
                                "/path/to/binary".to_string(),
                                "--exact".to_string(),
                                "test_retry".to_string(),
                            ],
                        },
                    })
                    .unwrap();

                // Test a retry (attempt 3) - should show "TRY 3 START".
                reporter
                    .write_event(&TestEvent {
                        timestamp: Local::now().into(),
                        elapsed: Duration::ZERO,
                        kind: TestEventKind::TestRetryStarted {
                            stress_index: None,
                            test_instance: TestInstanceId {
                                binary_id: &binary_id,
                                test_name: &test_retry,
                            },
                            retry_data: RetryData {
                                attempt: 3,
                                total_attempts: 3,
                            },
                            running: 1,
                            command_line: vec![
                                "/path/to/binary".to_string(),
                                "--exact".to_string(),
                                "test_retry".to_string(),
                            ],
                        },
                    })
                    .unwrap();
            },
            &mut out,
        );

        insta::assert_snapshot!("verbose_command_line", out);
    }

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut out = String::new();

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
    use crate::reporter::events::AbortDescription;
    use windows_sys::Win32::{
        Foundation::{STATUS_CONTROL_C_EXIT, STATUS_CONTROL_STACK_VIOLATION},
        Globalization::SetThreadUILanguage,
    };

    #[test]
    fn test_write_windows_abort_line() {
        unsafe {
            // Set the thread UI language to US English for consistent output.
            SetThreadUILanguage(0x0409);
        }

        insta::assert_snapshot!(
            "ctrl_c_code",
            to_abort_line(AbortStatus::WindowsNtStatus(STATUS_CONTROL_C_EXIT))
        );
        insta::assert_snapshot!(
            "stack_violation_code",
            to_abort_line(AbortStatus::WindowsNtStatus(STATUS_CONTROL_STACK_VIOLATION)),
        );
        insta::assert_snapshot!("job_object", to_abort_line(AbortStatus::JobObject));
    }

    #[track_caller]
    fn to_abort_line(status: AbortStatus) -> String {
        let mut buf = String::new();
        let description = AbortDescription::from(status);
        write_windows_abort_line(&description, &Styles::default(), &mut buf).unwrap();
        buf
    }
}
