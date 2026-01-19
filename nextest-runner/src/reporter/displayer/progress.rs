// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{CargoConfigs, DiscoveredConfig},
    helpers::{DisplayTestInstance, plural},
    list::TestInstanceId,
    reporter::{
        displayer::formatters::DisplayBracketedHhMmSs,
        events::*,
        helpers::{Styles, print_lines_in_chunks},
    },
    run_mode::NextestRunMode,
};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use nextest_metadata::{RustBinaryId, TestCaseName};
use owo_colors::OwoColorize;
use std::{
    cmp::{max, min},
    env, fmt,
    str::FromStr,
    time::{Duration, Instant},
};
use swrite::{SWrite, swrite};
use tracing::debug;

/// The refresh rate for the progress bar, set to a minimal value.
///
/// For progress, during each tick, two things happen:
///
/// - We update the message, calling self.bar.set_message.
/// - We print any buffered output.
///
/// We want both of these updates to be combined into one terminal flush, so we
/// set *this* to a minimal value (so self.bar.set_message doesn't do a redraw),
/// and rely on ProgressBar::print_and_flush_buffer to always flush the
/// terminal.
const PROGRESS_REFRESH_RATE_HZ: u8 = 1;

/// The maximum number of running tests to display with
/// `--show-progress=running` or `only`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxProgressRunning {
    /// Show a specific maximum number of running tests.
    /// If 0, running tests (including the overflow summary) aren't displayed.
    Count(usize),

    /// Show all running tests (no limit).
    Infinite,
}

impl MaxProgressRunning {
    /// The default value (8 tests).
    pub const DEFAULT_VALUE: Self = Self::Count(8);
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
            Ok(n) => Ok(Self::Count(n)),
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
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowProgress {
    /// Automatically decide based on environment.
    #[default]
    Auto,

    /// No progress display.
    None,

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
    test_name: TestCaseName,
    status: RunningTestStatus,
    start_time: Instant,
    paused_for: Duration,
}

impl RunningTest {
    fn message(&self, now: Instant, width: usize, styles: &Styles) -> String {
        let mut elapsed = (now - self.start_time).saturating_sub(self.paused_for);
        let status = match self.status {
            RunningTestStatus::Running => "     ".to_owned(),
            RunningTestStatus::Slow => " SLOW".style(styles.skip).to_string(),
            RunningTestStatus::Delay(d) => {
                // The elapsed might be greater than the delay duration in case
                // we ticked past the delay duration without receiving a
                // notification that the test retry started.
                elapsed = d.saturating_sub(elapsed);
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
        let max_width = width.saturating_sub(25);
        let test = DisplayTestInstance::new(
            None,
            None,
            TestInstanceId {
                binary_id: &self.binary_id,

                test_name: &self.test_name,
            },
            &styles.list_styles,
        )
        .with_max_width(max_width);
        format!("       {} [{:>9}] {}", status, elapsed, test)
    }
}

#[derive(Debug)]
pub(super) struct ProgressBarState {
    bar: ProgressBar,
    mode: NextestRunMode,
    stats: RunStats,
    running: usize,
    max_progress_running: MaxProgressRunning,
    // Keep track of the maximum number of lines used. This allows to adapt the
    // size of the 'viewport' to what we are using, and not just to the maximum
    // number of tests that can be run in parallel
    max_running_displayed: usize,
    // None when the running tests are not displayed
    running_tests: Option<Vec<RunningTest>>,
    buffer: String,
    // Size in bytes for chunking println calls. Configurable via the
    // undocumented __NEXTEST_PROGRESS_PRINTLN_CHUNK_SIZE env var.
    println_chunk_size: usize,
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
}

impl ProgressBarState {
    pub(super) fn new(
        mode: NextestRunMode,
        test_count: usize,
        progress_chars: &str,
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

        let running_tests =
            (!matches!(max_progress_running, MaxProgressRunning::Count(0))).then(Vec::new);

        // The println chunk size defaults to a value chosen by experimentation,
        // locally and over SSH. This controls how often the progress bar
        // refreshes during large output bursts.
        let println_chunk_size = env::var("__NEXTEST_PROGRESS_PRINTLN_CHUNK_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(4096);

        Self {
            bar,
            mode,
            stats: RunStats::default(),
            running: 0,
            max_progress_running,
            max_running_displayed: 0,
            running_tests,
            buffer: String::new(),
            println_chunk_size,
            hidden_no_capture: false,
            hidden_run_paused: false,
            hidden_info_response: false,
        }
    }

    pub(super) fn tick(&mut self, styles: &Styles) {
        self.update_message(styles);
        self.print_and_clear_buffer();
    }

    fn print_and_clear_buffer(&mut self) {
        self.print_and_force_redraw();
        self.buffer.clear();
    }

    /// Prints the contents of the buffer, and always forces a redraw.
    fn print_and_force_redraw(&self) {
        if self.buffer.is_empty() {
            // Force a redraw as part of our contract. See the documentation for
            // `PROGRESS_REFRESH_RATE_HZ`.
            self.bar.force_draw();
            return;
        }

        // println below also forces a redraw, so we don't need to call
        // force_draw in this case.

        // ProgressBar::println is only called if there's something in the
        // buffer, for two reasons:
        //
        // 1. If passed in nothing at all, it prints an empty line.
        // 2. It forces a full redraw.
        //
        // But if self.buffer is too large, we can overwhelm the terminal with
        // large amounts of non-progress-bar output, causing the progress bar to
        // flicker in and out. To avoid those issues, we chunk the output to
        // maintain progress bar visibility by redrawing it regularly.
        print_lines_in_chunks(&self.buffer, self.println_chunk_size, |chunk| {
            self.bar.println(chunk);
        });
    }

    fn update_message(&mut self, styles: &Styles) {
        let mut msg = self.progress_bar_msg(styles);
        msg += "     ";

        if let Some(running_tests) = &self.running_tests {
            let (_, width) = console::Term::stderr().size();
            let width = max(width as usize, 40);
            let now = Instant::now();
            let mut count = match self.max_progress_running {
                MaxProgressRunning::Count(count) => min(running_tests.len(), count),
                MaxProgressRunning::Infinite => running_tests.len(),
            };
            for running_test in &running_tests[..count] {
                msg.push('\n');
                msg.push_str(&running_test.message(now, width, styles));
            }
            if count < running_tests.len() {
                let overflow_count = running_tests.len() - count;
                swrite!(
                    msg,
                    "\n             ... and {} more {} running",
                    overflow_count.style(styles.count),
                    plural::tests_str(self.mode, overflow_count),
                );
                count += 1;
            }
            self.max_running_displayed = max(self.max_running_displayed, count);
            msg.push_str(&"\n".to_string().repeat(self.max_running_displayed - count));
        }
        self.bar.set_message(msg);
    }

    fn progress_bar_msg(&self, styles: &Styles) -> String {
        progress_bar_msg(&self.stats, self.running, styles)
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
            }
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
                test_instance,
                ..
            } => {
                self.running = *running;
                self.stats = *current_stats;

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
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.to_owned(),
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
                self.stats = *current_stats;
                self.remove_test(test_instance);

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
                self.remove_test(test_instance);
                if let Some(running_tests) = &mut self.running_tests {
                    running_tests.push(RunningTest {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.to_owned(),
                        status: RunningTestStatus::Delay(*delay_before_next_attempt),
                        start_time: Instant::now(),
                        paused_for: Duration::ZERO,
                    });
                }
            }
            TestEventKind::TestRetryStarted { test_instance, .. } => {
                self.remove_test(test_instance);
                if let Some(running_tests) = &mut self.running_tests {
                    running_tests.push(RunningTest {
                        binary_id: test_instance.binary_id.clone(),
                        test_name: test_instance.test_name.to_owned(),
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
                            &rt.binary_id == test_instance.binary_id
                                && &rt.test_name == test_instance.test_name
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
                self.bar.set_elapsed(event.elapsed);

                if let Some(running_tests) = &mut self.running_tests {
                    let delta = current_global_elapsed.saturating_sub(event.elapsed);
                    for running_test in running_tests {
                        running_test.paused_for += delta;
                    }
                }
            }
            TestEventKind::RunBeginCancel {
                current_stats,
                running,
                ..
            }
            | TestEventKind::RunBeginKill {
                current_stats,
                running,
                ..
            } => {
                self.running = *running;
                self.stats = *current_stats;
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
                            && &e.test_name == test_instance.test_name
                    })
                    .expect("finished test to have started"),
            );
        }
    }

    pub(super) fn write_buf(&mut self, buf: &str) {
        self.buffer.push_str(buf);
    }

    #[inline]
    pub(super) fn finish_and_clear(&self) {
        self.print_and_force_redraw();
        self.bar.finish_and_clear();
    }

    fn stderr_target() -> ProgressDrawTarget {
        ProgressDrawTarget::stderr_with_hz(PROGRESS_REFRESH_RATE_HZ)
    }

    fn hidden_target() -> ProgressDrawTarget {
        ProgressDrawTarget::hidden()
    }

    fn should_hide(&self) -> bool {
        self.hidden_no_capture || self.hidden_run_paused || self.hidden_info_response
    }

    pub(super) fn is_hidden(&self) -> bool {
        self.bar.is_hidden()
    }
}

/// Whether to show OSC 9;4 terminal progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShowTerminalProgress {
    /// Show terminal progress.
    Yes,

    /// Do not show terminal progress.
    No,
}

impl ShowTerminalProgress {
    const ENV: &str = "CARGO_TERM_PROGRESS_TERM_INTEGRATION";

    /// Determines whether to show terminal progress based on Cargo configs and
    /// whether the output is a terminal.
    pub fn from_cargo_configs(configs: &CargoConfigs, is_terminal: bool) -> Self {
        // See whether terminal integration is enabled in Cargo.
        for config in configs.discovered_configs() {
            match config {
                DiscoveredConfig::CliOption { config, source } => {
                    if let Some(v) = config.term.progress.term_integration {
                        if v {
                            debug!("enabling terminal progress reporting based on {source:?}");
                            return Self::Yes;
                        } else {
                            debug!("disabling terminal progress reporting based on {source:?}");
                            return Self::No;
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
                            return Self::Yes;
                        } else if v == "false" {
                            debug!(
                                "disabling terminal progress reporting based on \
                                 CARGO_TERM_PROGRESS_TERM_INTEGRATION environment variable"
                            );
                            return Self::No;
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
                            return Self::Yes;
                        } else {
                            debug!("disabling terminal progress reporting based on {source:?}");
                            return Self::No;
                        }
                    }
                }
            }
        }

        if supports_osc_9_4(is_terminal) {
            Self::Yes
        } else {
            Self::No
        }
    }
}

/// OSC 9 terminal progress reporting.
#[derive(Default)]
pub(super) struct TerminalProgress {
    last_value: TerminalProgressValue,
}

impl TerminalProgress {
    pub(super) fn new(show: ShowTerminalProgress) -> Option<Self> {
        match show {
            ShowTerminalProgress::Yes => Some(Self::default()),
            ShowTerminalProgress::No => None,
        }
    }

    pub(super) fn update_progress(&mut self, event: &TestEvent<'_>) {
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

        self.last_value = value;
    }

    pub(super) fn last_value(&self) -> &TerminalProgressValue {
        &self.last_value
    }
}

/// Determines whether the terminal supports ANSI OSC 9;4.
fn supports_osc_9_4(is_terminal: bool) -> bool {
    if !is_terminal {
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
        && (term == "WezTerm" || term == "ghostty" || term == "iTerm.app")
    {
        debug!("autodetect terminal progress reporting: enabling since TERM_PROGRAM is {term}");
        return true;
    }

    false
}

/// A progress status value printable as an ANSI OSC 9;4 escape code.
///
/// Adapted from Cargo 1.87.
#[derive(PartialEq, Debug, Default)]
pub(super) enum TerminalProgressValue {
    /// No output.
    #[default]
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
        passed_timed_out: _,
        flaky,
        failed,
        failed_slow: _,
        failed_timed_out,
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

    if failed_timed_out > 0 {
        swrite!(
            out,
            "{} {}, ",
            failed_timed_out.style(styles.count),
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
    use crate::{
        reporter::TestOutputDisplay,
        test_output::{ChildExecutionOutput, ChildOutput, ChildSingleOutput, ChildSplitOutput},
    };
    use bytes::Bytes;
    use chrono::Local;

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

    #[test]
    fn running_test_snapshots() {
        let styles = Styles::default();
        let now = Instant::now();

        for (name, running_test) in running_test_examples(now) {
            let msg = running_test.message(now, 80, &styles);
            insta::assert_snapshot!(name, msg);
        }
    }

    fn running_test_examples(now: Instant) -> Vec<(&'static str, RunningTest)> {
        let binary_id = RustBinaryId::new("my-binary");
        let test_name = TestCaseName::new("test::my_test");
        let start_time = now - Duration::from_secs(125); // 2 minutes 5 seconds ago

        vec![
            (
                "running_status",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Running,
                    start_time,
                    paused_for: Duration::ZERO,
                },
            ),
            (
                "slow_status",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Slow,
                    start_time,
                    paused_for: Duration::ZERO,
                },
            ),
            (
                "delay_status",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Delay(Duration::from_secs(130)),
                    start_time,
                    paused_for: Duration::ZERO,
                },
            ),
            (
                "delay_status_underflow",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Delay(Duration::from_secs(124)),
                    start_time,
                    paused_for: Duration::ZERO,
                },
            ),
            (
                "retry_status",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Retry,
                    start_time,
                    paused_for: Duration::ZERO,
                },
            ),
            (
                "with_paused_duration",
                RunningTest {
                    binary_id: binary_id.clone(),
                    test_name: test_name.clone(),
                    status: RunningTestStatus::Running,
                    start_time,
                    paused_for: Duration::from_secs(30),
                },
            ),
        ]
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
                    failed_timed_out: 1,
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

    /// Test that `update_progress_bar` correctly updates `self.stats` when
    /// processing `TestStarted` and `TestFinished` events.
    ///
    /// This test verifies both:
    ///
    /// 1. State: `self.stats` equals the event's `current_stats` after processing.
    /// 2. Output: `state.progress_bar_msg()` reflects the updated stats.
    #[test]
    fn update_progress_bar_updates_stats() {
        let styles = Styles::default();
        let binary_id = RustBinaryId::new("test-binary");
        let test_name = TestCaseName::new("test_name");

        // Create ProgressBarState with initial (default) stats.
        let mut state = ProgressBarState::new(
            NextestRunMode::Test,
            10,
            "=> ",
            MaxProgressRunning::default(),
        );

        // Verify the initial state.
        assert_eq!(state.stats, RunStats::default());
        assert_eq!(state.running, 0);

        // Create a TestStarted event.
        let started_stats = RunStats {
            initial_run_count: 10,
            passed: 5,
            ..RunStats::default()
        };
        let started_event = TestEvent {
            timestamp: Local::now().fixed_offset(),
            elapsed: Duration::ZERO,
            kind: TestEventKind::TestStarted {
                stress_index: None,
                test_instance: TestInstanceId {
                    binary_id: &binary_id,
                    test_name: &test_name,
                },
                current_stats: started_stats,
                running: 3,
                command_line: vec![],
            },
        };

        state.update_progress_bar(&started_event, &styles);

        // Verify the state was updated.
        assert_eq!(
            state.stats, started_stats,
            "stats should be updated on TestStarted"
        );
        assert_eq!(state.running, 3, "running should be updated on TestStarted");

        // Verify that the output reflects the updated stats.
        let msg = state.progress_bar_msg(&styles);
        insta::assert_snapshot!(msg, @"3 running, 5 passed, 0 skipped");

        // Create a TestFinished event with different stats.
        let finished_stats = RunStats {
            initial_run_count: 10,
            finished_count: 1,
            passed: 8,
            ..RunStats::default()
        };
        let finished_event = TestEvent {
            timestamp: Local::now().fixed_offset(),
            elapsed: Duration::ZERO,
            kind: TestEventKind::TestFinished {
                stress_index: None,
                test_instance: TestInstanceId {
                    binary_id: &binary_id,
                    test_name: &test_name,
                },
                success_output: TestOutputDisplay::Never,
                failure_output: TestOutputDisplay::Never,
                junit_store_success_output: false,
                junit_store_failure_output: false,
                run_statuses: ExecutionStatuses::new(vec![ExecuteStatus {
                    retry_data: RetryData {
                        attempt: 1,
                        total_attempts: 1,
                    },
                    output: make_test_output(),
                    result: ExecutionResultDescription::Pass,
                    start_time: Local::now().fixed_offset(),
                    time_taken: Duration::from_secs(1),
                    is_slow: false,
                    delay_before_start: Duration::ZERO,
                    error_summary: None,
                    output_error_slice: None,
                }]),
                current_stats: finished_stats,
                running: 2,
            },
        };

        state.update_progress_bar(&finished_event, &styles);

        // Verify the state was updated.
        assert_eq!(
            state.stats, finished_stats,
            "stats should be updated on TestFinished"
        );
        assert_eq!(
            state.running, 2,
            "running should be updated on TestFinished"
        );

        // Verify that the output reflects the updated stats.
        let msg = state.progress_bar_msg(&styles);
        insta::assert_snapshot!(msg, @"2 running, 8 passed, 0 skipped");

        // Create a RunBeginCancel event.
        let cancel_stats = RunStats {
            initial_run_count: 10,
            finished_count: 3,
            passed: 2,
            failed: 1,
            cancel_reason: Some(CancelReason::TestFailure),
            ..RunStats::default()
        };
        let cancel_event = TestEvent {
            timestamp: Local::now().fixed_offset(),
            elapsed: Duration::ZERO,
            kind: TestEventKind::RunBeginCancel {
                setup_scripts_running: 0,
                current_stats: cancel_stats,
                running: 4,
            },
        };

        state.update_progress_bar(&cancel_event, &styles);

        // Verify the state was updated.
        assert_eq!(
            state.stats, cancel_stats,
            "stats should be updated on RunBeginCancel"
        );
        assert_eq!(
            state.running, 4,
            "running should be updated on RunBeginCancel"
        );

        // Verify that the output reflects the updated stats.
        let msg = state.progress_bar_msg(&styles);
        insta::assert_snapshot!(msg, @"4 running, 2 passed, 1 failed, 0 skipped");

        // Create a RunBeginKill event with different stats.
        let kill_stats = RunStats {
            initial_run_count: 10,
            finished_count: 5,
            passed: 3,
            failed: 2,
            cancel_reason: Some(CancelReason::Signal),
            ..RunStats::default()
        };
        let kill_event = TestEvent {
            timestamp: Local::now().fixed_offset(),
            elapsed: Duration::ZERO,
            kind: TestEventKind::RunBeginKill {
                setup_scripts_running: 0,
                current_stats: kill_stats,
                running: 2,
            },
        };

        state.update_progress_bar(&kill_event, &styles);

        // Verify the state was updated.
        assert_eq!(
            state.stats, kill_stats,
            "stats should be updated on RunBeginKill"
        );
        assert_eq!(
            state.running, 2,
            "running should be updated on RunBeginKill"
        );

        // Verify that the output reflects the updated stats.
        let msg = state.progress_bar_msg(&styles);
        insta::assert_snapshot!(msg, @"2 running, 3 passed, 2 failed, 0 skipped");
    }

    // Helper to create minimal output for ExecuteStatus.
    fn make_test_output() -> ChildExecutionOutputDescription<ChildSingleOutput> {
        ChildExecutionOutput::Output {
            result: Some(ExecutionResult::Pass),
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: Some(Bytes::new().into()),
                stderr: Some(Bytes::new().into()),
            }),
            errors: None,
        }
        .into()
    }
}
