// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{CargoConfigs, DiscoveredConfig},
    reporter::{displayer::formatters::DisplayBracketedHhMmSs, events::*, helpers::Styles},
};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use owo_colors::OwoColorize;
use std::{
    env, fmt,
    io::{self, IsTerminal, Write},
    time::Duration,
};
use swrite::{SWrite, swrite};
use tracing::debug;

#[derive(Debug)]
pub(super) struct ProgressBarState {
    bar: ProgressBar,
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
    pub(super) fn new(test_count: usize, progress_chars: &str) -> Self {
        let bar = ProgressBar::new(test_count as u64);

        let test_count_width = format!("{}", test_count).len();
        // Create the template using the width as input. This is a
        // little confusing -- {{foo}} is what's passed into the
        // ProgressBar, while {bar} is inserted by the format!()
        // statement.
        let template = format!(
            "{{prefix:>12}} [{{elapsed_precise:>9}}] {{wide_bar}} \
            {{pos:>{test_count_width}}}/{{len:{test_count_width}}}: {{msg}}     "
        );
        bar.set_style(
            ProgressStyle::default_bar()
                .progress_chars(progress_chars)
                .template(&template)
                .expect("template is known to be valid"),
        );

        // NOTE: set_draw_target must be called before enable_steady_tick to avoid a
        // spurious extra line from being printed as the draw target changes.
        bar.set_draw_target(Self::stderr_target());
        // Enable a steady tick 10 times a second.
        bar.enable_steady_tick(Duration::from_millis(100));

        Self {
            bar,
            hidden_no_capture: false,
            hidden_run_paused: false,
            hidden_info_response: false,
        }
    }

    pub(super) fn update_progress_bar(&mut self, event: &TestEvent<'_>, styles: &Styles) {
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
                self.bar
                    .set_prefix(progress_bar_prefix(current_stats, *cancel_state, styles));
                self.bar
                    .set_message(progress_bar_msg(current_stats, *running, styles));
                // If there are skipped tests, the initial run count will be lower than when constructed
                // in ProgressBar::new.
                self.bar.set_length(current_stats.initial_run_count as u64);
                self.bar.set_position(current_stats.finished_count as u64);
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
                // Wish a mutable form of with_elapsed were supported.
                let bar = std::mem::replace(&mut self.bar, ProgressBar::hidden());
                self.bar = bar.with_elapsed(event.elapsed);
            }
            TestEventKind::RunBeginCancel { reason, .. }
            | TestEventKind::RunBeginKill { reason, .. } => {
                self.bar
                    .set_prefix(progress_bar_cancel_prefix(*reason, styles));
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

    pub(super) fn write_buf(&self, buf: &[u8]) -> io::Result<()> {
        // ProgressBar::println doesn't print status lines if the bar is
        // hidden. The suspend method prints it in all cases.
        self.bar.suspend(|| std::io::stderr().write_all(buf))
    }

    #[inline]
    pub(super) fn finish_and_clear(&self) {
        self.bar.finish_and_clear();
    }

    fn stderr_target() -> ProgressDrawTarget {
        // This used to be unbuffered, but that option went away from indicatif
        // 0.17.0. The refresh rate is now 20hz so that it's double the steady
        // tick rate.
        ProgressDrawTarget::stderr_with_hz(20)
    }

    fn hidden_target() -> ProgressDrawTarget {
        ProgressDrawTarget::hidden()
    }

    fn should_hide(&self) -> bool {
        self.hidden_no_capture || self.hidden_run_paused || self.hidden_info_response
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
            | TestEventKind::SetupScriptStarted { .. }
            | TestEventKind::SetupScriptSlow { .. }
            | TestEventKind::SetupScriptFinished { .. } => TerminalProgressValue::None,
            TestEventKind::TestStarted {
                current_stats,
                cancel_state,
                ..
            }
            | TestEventKind::TestFinished {
                current_stats,
                cancel_state,
                ..
            } => {
                let percentage = (current_stats.finished_count as f64
                    / current_stats.initial_run_count as f64)
                    * 100.0;
                if current_stats.has_failures() || cancel_state.is_some() {
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
             pass in stream (usually stderr) is not a terminal"
        );
        return false;
    }
    if std::env::var("WT_SESSION").is_ok() {
        debug!("autodetect terminal progress reporting: enabling since WT_SESSION is set");
        return true;
    };
    if std::env::var("ConEmuANSI").ok() == Some("ON".into()) {
        debug!("autodetect terminal progress reporting: enabling since ConEmuANSI is ON");
        return true;
    }
    if std::env::var("TERM_PROGRAM").ok() == Some("WezTerm".into()) {
        debug!("autodetect terminal progress reporting: enabling since TERM_PROGRAM is WezTerm");
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
    cancel_reason: Option<CancelReason>,
    styles: &Styles,
) -> String {
    // First, show the prefix.
    let mut s = progress_bar_prefix(current_stats, cancel_reason, styles);

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

fn progress_bar_cancel_prefix(reason: CancelReason, styles: &Styles) -> String {
    let status = match reason {
        CancelReason::SetupScriptFailure
        | CancelReason::TestFailure
        | CancelReason::ReportError
        | CancelReason::Signal
        | CancelReason::Interrupt => "Cancelling",
        CancelReason::SecondSignal => "Killing",
    };
    format!("{:>12}", status.style(styles.fail))
}

fn progress_bar_prefix(
    run_stats: &RunStats,
    cancel_reason: Option<CancelReason>,
    styles: &Styles,
) -> String {
    if let Some(reason) = cancel_reason {
        return progress_bar_cancel_prefix(reason, styles);
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
            let s = progress_str(
                elapsed,
                &stats,
                running,
                Some(CancelReason::TestFailure),
                &styles,
            );
            insta::assert_snapshot!(format!("{name}_with_cancel_reason"), s);

            let s = progress_str(elapsed, &stats, running, None, &styles);
            insta::assert_snapshot!(format!("{name}_without_cancel_reason"), s);
        }

        for (name, stats) in run_stats_setup_script_failure_examples() {
            let s = progress_str(
                elapsed,
                &stats,
                running,
                Some(CancelReason::SetupScriptFailure),
                &styles,
            );
            insta::assert_snapshot!(format!("{name}_with_cancel_reason"), s);

            let s = progress_str(elapsed, &stats, running, None, &styles);
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
                    ..RunStats::default()
                },
            ),
            (
                "one_exec_failed",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 10,
                    exec_failed: 1,
                    ..RunStats::default()
                },
            ),
            (
                "one_timed_out",
                RunStats {
                    initial_run_count: 20,
                    finished_count: 10,
                    timed_out: 1,
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
                    ..RunStats::default()
                },
            ),
            (
                "one_setup_script_exec_failed",
                RunStats {
                    initial_run_count: 35,
                    setup_scripts_exec_failed: 1,
                    ..RunStats::default()
                },
            ),
            (
                "one_setup_script_timed_out",
                RunStats {
                    initial_run_count: 40,
                    setup_scripts_timed_out: 1,
                    ..RunStats::default()
                },
            ),
        ]
    }
}
