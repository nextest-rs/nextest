// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{CargoConfigs, DiscoveredConfig},
    helpers::{DisplayTestInstance, plural},
    list::TestInstanceId,
    reporter::{
        PROGRESS_REFRESH_RATE_HZ, displayer::formatters::DisplayBracketedHhMmSs, events::*,
        helpers::Styles,
    },
};
use console::AnsiCodeIterator;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use nextest_metadata::RustBinaryId;
use owo_colors::OwoColorize;
use std::{
    cmp::{max, min},
    env, fmt,
    io::{self, IsTerminal, Write},
    num::NonZero,
    str::FromStr,
    time::{Duration, Instant},
};
use swrite::{SWrite, swrite};
use tracing::debug;
use unicode_width::UnicodeWidthChar as _;

/// The maximum number of running tests to display with
/// `--show-progress=running` or `only`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxProgressRunning {
    /// Show a specific maximum number of running tests.
    Count(NonZero<usize>),

    /// Show all running tests (no limit).
    Infinite,
}

impl MaxProgressRunning {
    /// The default value (8 tests).
    pub const DEFAULT_VALUE: Self = Self::Count(NonZero::new(8).unwrap());
}

impl Default for MaxProgressRunning {
    fn default() -> Self {
        Self::DEFAULT_VALUE
    }
}

impl FromStr for MaxProgressRunning {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("infinite") {
            return Ok(Self::Infinite);
        }

        match s.parse::<usize>() {
            Err(e) => Err(format!("Error: {e} parsing {s}")),
            Ok(0) => Err(
                "max-progress-running may not be 0 (use \"infinite\" for unlimited)".to_string(),
            ),
            Ok(n) => Ok(Self::Count(
                NonZero::new(n).expect("we just checked that this isn't 0"),
            )),
        }
    }
}

impl fmt::Display for MaxProgressRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Infinite => write!(f, "infinite"),
            Self::Count(n) => write!(f, "{n}"),
        }
    }
}

/// How to show progress.
#[derive(Default, Clone, Copy, Debug)]
pub enum ShowProgress {
    /// Automatically decide based on environment.
    #[default]
    Auto,

    /// No progress display.
    None,

    /// Show a progress bar.
    Bar,

    /// Show a counter on each line.
    Counter,

    /// Show a progress bar and the running tests
    Running,
}

#[derive(Debug)]
pub(super) enum RunningTestStatus {
    Running,
    Slow,
    Delay(Duration),
    Retry,
}

#[derive(Debug)]
pub(super) struct RunningTest {
    binary_id: RustBinaryId,
    test_name: String,
    status: RunningTestStatus,
    start_time: Instant,
    paused_for: Duration,
}

impl RunningTest {
    fn message(&self, now: &Instant, width: usize, styles: &Styles) -> String {
        let mut elapsed = (*now - self.start_time) - self.paused_for;
        let status = match self.status {
            RunningTestStatus::Running => "     ".to_owned(),
            RunningTestStatus::Slow => " SLOW".style(styles.skip).to_string(),
            RunningTestStatus::Delay(d) => {
                elapsed = d - elapsed;
                "DELAY".style(styles.retry).to_string()
            }
            RunningTestStatus::Retry => "RETRY".style(styles.retry).to_string(),
        };
        let elapsed = format!(
            "{:0>2}:{:0>2}:{:0>2}",
            elapsed.as_secs() / 3600,
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60,
        );
        let mut test = format!(
            "{}",
            DisplayTestInstance::new(
                None,
                None,
                TestInstanceId {
                    binary_id: &self.binary_id,

                    test_name: &self.test_name
                },
                &styles.list_styles
            )
        );
        let max_width = width.saturating_sub(25);
        let test_width = measure_text_width(&test);
        if test_width > max_width {
            test = ansi_get(&test, test_width - max_width, test_width)
        }
        format!("       {} [{:>9}] {}", status, elapsed, test)
    }
}

pub fn measure_text_width(s: &str) -> usize {
    AnsiCodeIterator::new(s)
        .filter_map(|(s, is_ansi)| match is_ansi {
            false => Some(s.chars().count()),
            true => None,
        })
        .sum()
}

pub fn ansi_get(text: &str, start: usize, end: usize) -> String {
    let mut pos = 0;
    let mut res = String::new();
    for (s, is_ansi) in AnsiCodeIterator::new(text) {
        if is_ansi {
            res.push_str(s);
            continue;
        } else if pos >= end {
            continue;
        }

        for c in s.chars() {
            let c_width = c.width().unwrap_or(0);
            if start <= pos && pos + c_width <= end {
                res.push(c);
            }
            pos += c_width;
            if pos > end {
                // no need to iterate over the rest of s
                break;
            }
        }
    }
    res
}

#[derive(Debug)]
pub(super) struct ProgressBarState {
    bar: ProgressBar,
    stats: RunStats,
    running: usize,
    max_progress_running: MaxProgressRunning,
    // Keep track of the maximum number of lines used. This allows to adapt the
    // size of the 'viewport' to what we are using, and not just to the maximum
    // number of tests that can be run in parallel
    max_running_displayed: usize,
    // None when the running tests are not displayed
    running_tests: Option<Vec<RunningTest>>,
    buffer: Vec<u8>,
    // Reasons for hiding the progress bar. We show the progress bar if none of
    // these are set and hide it if any of them are set.
    //
    // indicatif cannot handle this kind of "stacked" state management, so it
    // falls on us to do so.
    //
    // The current draw target is a pure function of these three booleans: if
    // any of them are set, the draw target is hidden, otherwise it's stderr. If
    // this changes, we'll need to track those other inputs.
    hidden_no_capture: bool,
    hidden_run_paused: bool,
    hidden_info_response: bool,
    hidden_between_sub_runs: bool,
}

impl ProgressBarState {
    pub(super) fn new(
        test_count: usize,
        progress_chars: &str,
        show_running: bool,
        max_progress_running: MaxProgressRunning,
    ) -> Self {
        let bar = ProgressBar::new(test_count as u64);
        let test_count_width = format!("{test_count}").len();
        // Create the template using the width as input. This is a
        // little confusing -- {{foo}} is what's passed into the
        // ProgressBar, while {bar} is inserted by the format!()
        // statement.
        let template = format!(
            "{{prefix:>12}} [{{elapsed_precise:>9}}] {{wide_bar}} \
            {{pos:>{test_count_width}}}/{{len:{test_count_width}}}: {{msg}}"
        );
        bar.set_style(
            ProgressStyle::default_bar()
                .progress_chars(progress_chars)
                .template(&template)
                .expect("template is known to be valid"),
        );

        let running_tests = show_running.then(Vec::new);

        Self {
            bar,
            stats: RunStats::default(),
            running: 0,
            max_progress_running,
            max_running_displayed: 0,
            running_tests,
            buffer: Vec::new(),
            hidden_no_capture: false,
            hidden_run_paused: false,
            hidden_info_response: false,
            hidden_between_sub_runs: false,
        }
    }

    pub(super) fn tick(&mut self, styles: &Styles) {
        self.update_message(styles);
        self.print_and_clear_buffer();
    }

    fn print_and_clear_buffer(&mut self) {
        self.print_buffer();
        self.buffer.clear();
    }

    fn print_buffer(&self) {
        // ProgressBar::println doesn't print status lines if the bar is
        // hidden. The suspend method prints it in all cases.
        // suspend forces a full redraw, so we call it only if there is
        // something in the buffer
        if !self.buffer.is_empty() {
            self.bar.suspend(|| {
                std::io::stderr()
                    .write_all(&self.buffer)
                    .expect("write to succeed")
            });
        }
    }

    pub(super) fn update_message(&mut self, styles: &Styles) {
        let mut msg = progress_bar_msg(&self.stats, self.running, styles);
        msg += "     ";

        if let Some(running_tests) = &self.running_tests {
            let (_, width) = console::Term::stderr().size();
            let width = max(width as usize, 40);
            let now = Instant::now();
            let mut count = match self.max_progress_running {
                MaxProgressRunning::Count(count) => min(running_tests.len(), count.into()),
                MaxProgressRunning::Infinite => running_tests.len(),
            };
            for running_test in &running_tests[..count] {
                msg.push('\n');
                msg.push_str(&running_test.message(&now, width, styles));
            }
            if count < running_tests.len() {
                let overflow_count = running_tests.len() - count;
                msg.push_str(&format!(
                    "\n             ... and {} more {} running",
                    overflow_count.style(styles.count),
                    plural::tests_str(overflow_count),
                ));
                count += 1;
            }
            self.max_running_displayed = max(self.max_running_displayed, count);
            msg.push_str(&"\n".to_string().repeat(self.max_running_displayed - count));
        }
        self.bar.set_message(msg);
    }

    pub(super) fn update_progress_bar(&mut self, event: &TestEvent<'_>, styles: &Styles) {
        let before_should_hide = self.should_hide();

        match &event.kind {
            TestEventKind::StressSubRunStarted { .. } => {
                self.bar.reset();
            }
            TestEventKind::StressSubRunFinished { .. } => {
                // Clear all test bars to remove empty lines of output between
                // sub-runs.
                self.bar.finish_and_clear();
                // Hide the progress bar between sub runs to avoid a spurious
                // progress bar.
                self.hidden_between_sub_runs = true;
            }
            TestEventKind::SetupScriptStarted { no_capture, .. } => {
                // Hide the progress bar if either stderr or stdout are being passed through.
                if *no_capture {
                    self.hidden_no_capture = true;
                }
                self.hidden_between_sub_runs = false;
            }
            TestEventKind::SetupScriptFinished { no_capture, .. } => {
                // Restore the progress bar if it was hidden.
                if *no_capture {
                    self.hidden_no_capture = false;
                }
                self.hidden_between_sub_runs = false;
            }
            TestEventKind::TestStarted {
                current_stats,
                running,
                test_instance,
                ..
            } => {
                self.running = *running;
                self.hidden_between_sub_runs = false;

                self.bar.set_prefix(progress_bar_prefix(
                    current_stats,
                    current_stats.cancel_reason,
                    styles,
                ));
                // If there are skipped tests, the initial run count will be lower than when constructed
                // in ProgressBar::new.
                self.bar.set_length(current_stats.initial_run_count as u64);
                self.bar.set_position(current_stats.finished_count as u64);

                if let Some(running_tests) = &mut self.running_tests {
                    running_tests.push(RunningTest {
                        binary_id: test_instance.id().binary_id.clone(),
                        test_name: test_instance.id().test_name.to_owned(),
                        status: RunningTestStatus::Running,
                        start_time: Instant::now(),
                        paused_for: Duration::ZERO,
                    });
                }
            }
            TestEventKind::TestFinished {
                current_stats,
                running,
                test_instance,
                ..
            } => {
                self.running = *running;
                self.remove_test(&test_instance.id());

                self.hidden_between_sub_runs = false;

                self.bar.set_prefix(progress_bar_prefix(
                    current_stats,
                    current_stats.cancel_reason,
                    styles,
                ));
                // If there are skipped tests, the initial run count will be lower than when constructed
                // in ProgressBar::new.
                self.bar.set_length(current_stats.initial_run_count as u64);
                self.bar.set_position(current_stats.finished_count as u64);
            }
            TestEventKind::TestAttemptFailedWillRetry {
                test_instance,
                delay_before_next_attempt,
                ..
            } => {
                self.remove_test(&test_instance.id());
                if let Some(running_tests) = &mut self.running_tests {
                    running_tests.push(RunningTest {
                        binary_id: test_instance.id().binary_id.clone(),
                        test_name: test_instance.id().test_name.to_owned(),
                        status: RunningTestStatus::Delay(*delay_before_next_attempt),
                        start_time: Instant::now(),
                        paused_for: Duration::ZERO,
                    });
                }
            }
            TestEventKind::TestRetryStarted { test_instance, .. } => {
                self.remove_test(&test_instance.id());
                if let Some(running_tests) = &mut self.running_tests {
                    running_tests.push(RunningTest {
                        binary_id: test_instance.id().binary_id.clone(),
                        test_name: test_instance.id().test_name.to_owned(),
                        status: RunningTestStatus::Retry,
                        start_time: Instant::now(),
                        paused_for: Duration::ZERO,
                    });
                }
            }
            TestEventKind::TestSlow { test_instance, .. } => {
                if let Some(running_tests) = &mut self.running_tests {
                    running_tests
                        .iter_mut()
                        .find(|rt| {
                            &rt.binary_id == test_instance.id().binary_id
                                && rt.test_name == test_instance.id().test_name
                        })
                        .expect("a slow test to be already running")
                        .status = RunningTestStatus::Slow;
                }
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
                let current_global_elapsed = self.bar.elapsed();

                // `ProgressBar` is a lightweight handle and calling methods
                // like `with_elapsed` on a clone also affects the original.
                // Ideally there would be a `set_elapsed` method on self.bar,
                // though.
                self.bar.clone().with_elapsed(event.elapsed);

                if let Some(running_tests) = &mut self.running_tests {
                    let delta = current_global_elapsed.saturating_sub(event.elapsed);
                    for running_test in running_tests {
                        running_test.paused_for += delta;
                    }
                }
            }
            TestEventKind::RunBeginCancel { current_stats, .. }
            | TestEventKind::RunBeginKill { current_stats, .. } => {
                self.bar.set_prefix(progress_bar_cancel_prefix(
                    current_stats.cancel_reason,
                    styles,
                ));
            }
            _ => {}
        }

        let after_should_hide = self.should_hide();

        match (before_should_hide, after_should_hide) {
            (false, true) => self.bar.set_draw_target(Self::hidden_target()),
            (true, false) => self.bar.set_draw_target(Self::stderr_target()),
            _ => {}
        }
    }

    fn remove_test(&mut self, test_instance: &TestInstanceId) {
        if let Some(running_tests) = &mut self.running_tests {
            running_tests.remove(
                running_tests
                    .iter()
                    .position(|e| {
                        &e.binary_id == test_instance.binary_id
                            && e.test_name == test_instance.test_name
                    })
                    .expect("finished test to have started"),
            );
        }
    }

    pub(super) fn write_buf(&mut self, buf: &[u8]) {
        self.buffer.extend_from_slice(buf);
    }

    #[inline]
    pub(super) fn finish_and_clear(&self) {
        self.print_buffer();
        self.bar.finish_and_clear();
    }

    fn stderr_target() -> ProgressDrawTarget {
        ProgressDrawTarget::stderr_with_hz(PROGRESS_REFRESH_RATE_HZ)
    }

    fn hidden_target() -> ProgressDrawTarget {
        ProgressDrawTarget::hidden()
    }

    fn should_hide(&self) -> bool {
        self.hidden_no_capture
            || self.hidden_run_paused
            || self.hidden_info_response
            || self.hidden_between_sub_runs
    }

    pub(super) fn is_hidden(&self) -> bool {
        self.bar.is_hidden()
    }
}

/// OSC 9 terminal progress reporting.
pub(super) struct TerminalProgress {}

impl TerminalProgress {
    const ENV: &str = "CARGO_TERM_PROGRESS_TERM_INTEGRATION";

    pub(super) fn new(configs: &CargoConfigs, stream: &dyn IsTerminal) -> Option<Self> {
        // See whether terminal integration is enabled in Cargo.
        for config in configs.discovered_configs() {
            match config {
                DiscoveredConfig::CliOption { config, source } => {
                    if let Some(v) = config.term.progress.term_integration {
                        if v {
                            debug!("enabling terminal progress reporting based on {source:?}");
                            return Some(Self {});
                        } else {
                            debug!("disabling terminal progress reporting based on {source:?}");
                            return None;
                        }
                    }
                }
                DiscoveredConfig::Env => {
                    if let Some(v) = env::var_os(Self::ENV) {
                        if v == "true" {
                            debug!(
                                "enabling terminal progress reporting based on \
                                 CARGO_TERM_PROGRESS_TERM_INTEGRATION environment variable"
                            );
                            return Some(Self {});
                        } else if v == "false" {
                            debug!(
                                "disabling terminal progress reporting based on \
                                 CARGO_TERM_PROGRESS_TERM_INTEGRATION environment variable"
                            );
                            return None;
                        } else {
                            debug!(
                                "invalid value for CARGO_TERM_PROGRESS_TERM_INTEGRATION \
                                 environment variable: {v:?}, ignoring"
                            );
                        }
                    }
                }
                DiscoveredConfig::File { config, source } => {
                    if let Some(v) = config.term.progress.term_integration {
                        if v {
                            debug!("enabling terminal progress reporting based on {source:?}");
                            return Some(Self {});
                        } else {
                            debug!("disabling terminal progress reporting based on {source:?}");
                            return None;
                        }
                    }
                }
            }
        }

        supports_osc_9_4(stream).then_some(TerminalProgress {})
    }

    pub(super) fn update_progress(
        &self,
        event: &TestEvent<'_>,
        writer: &mut dyn Write,
    ) -> Result<(), io::Error> {
        let value = match &event.kind {
            TestEventKind::RunStarted { .. }
            | TestEventKind::StressSubRunStarted { .. }
            | TestEventKind::StressSubRunFinished { .. }
            | TestEventKind::SetupScriptStarted { .. }
            | TestEventKind::SetupScriptSlow { .. }
            | TestEventKind::SetupScriptFinished { .. } => TerminalProgressValue::None,
            TestEventKind::TestStarted { current_stats, .. }
            | TestEventKind::TestFinished { current_stats, .. } => {
                let percentage = (current_stats.finished_count as f64
                    / current_stats.initial_run_count as f64)
                    * 100.0;
                if current_stats.has_failures() || current_stats.cancel_reason.is_some() {
                    TerminalProgressValue::Error(percentage)
                } else {
                    TerminalProgressValue::Value(percentage)
                }
            }
            TestEventKind::TestSlow { .. }
            | TestEventKind::TestAttemptFailedWillRetry { .. }
            | TestEventKind::TestRetryStarted { .. }
            | TestEventKind::TestSkipped { .. }
            | TestEventKind::InfoStarted { .. }
            | TestEventKind::InfoResponse { .. }
            | TestEventKind::InfoFinished { .. }
            | TestEventKind::InputEnter { .. } => TerminalProgressValue::None,
            TestEventKind::RunBeginCancel { current_stats, .. }
            | TestEventKind::RunBeginKill { current_stats, .. } => {
                // In this case, always indicate an error.
                let percentage = (current_stats.finished_count as f64
                    / current_stats.initial_run_count as f64)
                    * 100.0;
                TerminalProgressValue::Error(percentage)
            }
            TestEventKind::RunPaused { .. }
            | TestEventKind::RunContinued { .. }
            | TestEventKind::RunFinished { .. } => {
                // Reset the terminal state to nothing, since nextest is giving
                // up control of the terminal at this point.
                TerminalProgressValue::Remove
            }
        };

        write!(writer, "{value}")
    }
}

/// Determines whether the terminal supports ANSI OSC 9;4.
fn supports_osc_9_4(stream: &dyn IsTerminal) -> bool {
    if !stream.is_terminal() {
        debug!(
            "autodetect terminal progress reporting: disabling since \
             passed-in stream (usually stderr) is not a terminal"
        );
        return false;
    }
    if std::env::var_os("WT_SESSION").is_some() {
        debug!("autodetect terminal progress reporting: enabling since WT_SESSION is set");
        return true;
    };
    if std::env::var_os("ConEmuANSI").is_some_and(|term| term == "ON") {
        debug!("autodetect terminal progress reporting: enabling since ConEmuANSI is ON");
        return true;
    }
    if let Ok(term) = std::env::var("TERM_PROGRAM")
        && (term == "WezTerm" || term == "ghostty")
    {
        debug!("autodetect terminal progress reporting: enabling since TERM_PROGRAM is {term}");
        return true;
    }

    false
}

/// A progress status value printable as an ANSI OSC 9;4 escape code.
///
/// Adapted from Cargo 1.87.
#[derive(PartialEq, Debug)]
enum TerminalProgressValue {
    /// No output.
    None,
    /// Remove progress.
    Remove,
    /// Progress value (0-100).
    Value(f64),
    /// Indeterminate state (no bar, just animation)
    ///
    /// We don't use this yet, but might in the future.
    #[expect(dead_code)]
    Indeterminate,
    /// Progress value in an error state (0-100).
    Error(f64),
}

impl fmt::Display for TerminalProgressValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // From https://conemu.github.io/en/AnsiEscapeCodes.html#ConEmu_specific_OSC
        // ESC ] 9 ; 4 ; st ; pr ST
        // When st is 0: remove progress.
        // When st is 1: set progress value to pr (number, 0-100).
        // When st is 2: set error state in taskbar, pr is optional.
        // When st is 3: set indeterminate state, pr is ignored.
        // When st is 4: set paused state, pr is optional.
        let (state, progress) = match self {
            Self::None => return Ok(()), // No output
            Self::Remove => (0, 0.0),
            Self::Value(v) => (1, *v),
            Self::Indeterminate => (3, 0.0),
            Self::Error(v) => (2, *v),
        };
        write!(f, "\x1b]9;4;{state};{progress:.0}\x1b\\")
    }
}

/// Returns a summary of current progress.
pub(super) fn progress_str(
    elapsed: Duration,
    current_stats: &RunStats,
    running: usize,
    styles: &Styles,
) -> String {
    // First, show the prefix.
    let mut s = progress_bar_prefix(current_stats, current_stats.cancel_reason, styles);

    // Then, the time elapsed, test counts, and message.
    swrite!(
        s,
        " {}{}/{}: {}",
        DisplayBracketedHhMmSs(elapsed),
        current_stats.finished_count,
        current_stats.initial_run_count,
        progress_bar_msg(current_stats, running, styles)
    );

    s
}

pub(super) fn write_summary_str(run_stats: &RunStats, styles: &Styles, out: &mut String) {
    // Written in this style to ensure new fields are accounted for.
    let &RunStats {
        initial_run_count: _,
        finished_count: _,
        setup_scripts_initial_count: _,
        setup_scripts_finished_count: _,
        setup_scripts_passed: _,
        setup_scripts_failed: _,
        setup_scripts_exec_failed: _,
        setup_scripts_timed_out: _,
        passed,
        passed_slow,
        flaky,
        failed,
        failed_slow: _,
        timed_out,
        leaky,
        leaky_failed,
        exec_failed,
        skipped,
        cancel_reason: _,
    } = run_stats;

    swrite!(
        out,
        "{} {}",
        passed.style(styles.count),
        "passed".style(styles.pass)
    );

    if passed_slow > 0 || flaky > 0 || leaky > 0 {
        let mut text = Vec::with_capacity(3);
        if passed_slow > 0 {
            text.push(format!(
                "{} {}",
                passed_slow.style(styles.count),
                "slow".style(styles.skip),
            ));
        }
        if flaky > 0 {
            text.push(format!(
                "{} {}",
                flaky.style(styles.count),
                "flaky".style(styles.skip),
            ));
        }
        if leaky > 0 {
            text.push(format!(
                "{} {}",
                leaky.style(styles.count),
                "leaky".style(styles.skip),
            ));
        }
        swrite!(out, " ({})", text.join(", "));
    }
    swrite!(out, ", ");

    if failed > 0 {
        swrite!(
            out,
            "{} {}",
            failed.style(styles.count),
            "failed".style(styles.fail),
        );
        if leaky_failed > 0 {
            swrite!(
                out,
                " ({} due to being {})",
                leaky_failed.style(styles.count),
                "leaky".style(styles.fail),
            );
        }
        swrite!(out, ", ");
    }

    if exec_failed > 0 {
        swrite!(
            out,
            "{} {}, ",
            exec_failed.style(styles.count),
            "exec failed".style(styles.fail),
        );
    }

    if timed_out > 0 {
        swrite!(
            out,
            "{} {}, ",
            timed_out.style(styles.count),
            "timed out".style(styles.fail),
        );
    }

    swrite!(
        out,
        "{} {}",
        skipped.style(styles.count),
        "skipped".style(styles.skip),
    );
}

fn progress_bar_cancel_prefix(reason: Option<CancelReason>, styles: &Styles) -> String {
    let status = match reason {
        Some(CancelReason::SetupScriptFailure)
        | Some(CancelReason::TestFailure)
        | Some(CancelReason::ReportError)
        | Some(CancelReason::GlobalTimeout)
        | Some(CancelReason::TestFailureImmediate)
        | Some(CancelReason::Signal)
        | Some(CancelReason::Interrupt)
        | None => "Cancelling",
        Some(CancelReason::SecondSignal) => "Killing",
    };
    format!("{:>12}", status.style(styles.fail))
}

fn progress_bar_prefix(
    run_stats: &RunStats,
    cancel_reason: Option<CancelReason>,
    styles: &Styles,
) -> String {
    if let Some(reason) = cancel_reason {
        return progress_bar_cancel_prefix(Some(reason), styles);
    }

    let style = if run_stats.has_failures() {
        styles.fail
    } else {
        styles.pass
    };

    format!("{:>12}", "Running".style(style))
}

pub(super) fn progress_bar_msg(
    current_stats: &RunStats,
    running: usize,
    styles: &Styles,
) -> String {
    let mut s = format!("{} running, ", running.style(styles.count));
    write_summary_str(current_stats, styles, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar_prefix() {
        let mut styles = Styles::default();
        styles.colorize();

        for (name, stats) in run_stats_test_failure_examples() {
            let prefix = progress_bar_prefix(&stats, Some(CancelReason::TestFailure), &styles);
            assert_eq!(
                prefix,
                "  Cancelling".style(styles.fail).to_string(),
                "{name} matches"
            );
        }
        for (name, stats) in run_stats_setup_script_failure_examples() {
            let prefix =
                progress_bar_prefix(&stats, Some(CancelReason::SetupScriptFailure), &styles);
            assert_eq!(
                prefix,
                "  Cancelling".style(styles.fail).to_string(),
                "{name} matches"
            );
        }

        let prefix = progress_bar_prefix(&RunStats::default(), Some(CancelReason::Signal), &styles);
        assert_eq!(prefix, "  Cancelling".style(styles.fail).to_string());

        let prefix = progress_bar_prefix(&RunStats::default(), None, &styles);
        assert_eq!(prefix, "     Running".style(styles.pass).to_string());

        for (name, stats) in run_stats_test_failure_examples() {
            let prefix = progress_bar_prefix(&stats, None, &styles);
            assert_eq!(
                prefix,
                "     Running".style(styles.fail).to_string(),
                "{name} matches"
            );
        }
        for (name, stats) in run_stats_setup_script_failure_examples() {
            let prefix = progress_bar_prefix(&stats, None, &styles);
            assert_eq!(
                prefix,
                "     Running".style(styles.fail).to_string(),
                "{name} matches"
            );
        }
    }

    #[test]
    fn progress_str_snapshots() {
        let mut styles = Styles::default();
        styles.colorize();

        // This elapsed time is arbitrary but reasonably large.
        let elapsed = Duration::from_secs(123456);
        let running = 10;

        for (name, stats) in run_stats_test_failure_examples() {
            let s = progress_str(elapsed, &stats, running, &styles);
            insta::assert_snapshot!(format!("{name}_with_cancel_reason"), s);

            let mut stats = stats;
            stats.cancel_reason = None;
            let s = progress_str(elapsed, &stats, running, &styles);
            insta::assert_snapshot!(format!("{name}_without_cancel_reason"), s);
        }

        for (name, stats) in run_stats_setup_script_failure_examples() {
            let s = progress_str(elapsed, &stats, running, &styles);
            insta::assert_snapshot!(format!("{name}_with_cancel_reason"), s);

            let mut stats = stats;
            stats.cancel_reason = None;
            let s = progress_str(elapsed, &stats, running, &styles);
            insta::assert_snapshot!(format!("{name}_without_cancel_reason"), s);
        }
    }

    fn run_stats_test_failure_examples() -> Vec<(&'static str, RunStats)> {
        vec![
            (
                "one_failed",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 1,
                    failed: 1,
                    cancel_reason: Some(CancelReason::TestFailure),
                    ..RunStats::default()
                },
            ),
            (
                "one_failed_one_passed",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 2,
                    failed: 1,
                    passed: 1,
                    cancel_reason: Some(CancelReason::TestFailure),
                    ..RunStats::default()
                },
            ),
            (
                "one_exec_failed",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 10,
                    exec_failed: 1,
                    cancel_reason: Some(CancelReason::TestFailure),
                    ..RunStats::default()
                },
            ),
            (
                "one_timed_out",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 10,
                    timed_out: 1,
                    cancel_reason: Some(CancelReason::TestFailure),
                    ..RunStats::default()
                },
            ),
        ]
    }

    fn run_stats_setup_script_failure_examples() -> Vec<(&'static str, RunStats)> {
        vec![
            (
                "one_setup_script_failed",
                RunStats {
                    initial_run_count: 30,
                    setup_scripts_failed: 1,
                    cancel_reason: Some(CancelReason::SetupScriptFailure),
                    ..RunStats::default()
                },
            ),
            (
                "one_setup_script_exec_failed",
                RunStats {
                    initial_run_count: 35,
                    setup_scripts_exec_failed: 1,
                    cancel_reason: Some(CancelReason::SetupScriptFailure),
                    ..RunStats::default()
                },
            ),
            (
                "one_setup_script_timed_out",
                RunStats {
                    initial_run_count: 40,
                    setup_scripts_timed_out: 1,
                    cancel_reason: Some(CancelReason::SetupScriptFailure),
                    ..RunStats::default()
                },
            ),
        ]
    }
}
