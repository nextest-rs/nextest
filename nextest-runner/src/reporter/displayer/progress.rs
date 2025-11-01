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
use iddqd::{IdHashItem, IdHashMap, id_upcast};
use indexmap::IndexSet;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use itertools::Itertools as _;
use nextest_metadata::RustBinaryId;
use owo_colors::OwoColorize;
use std::{
    cmp::max,
    env, fmt,
    io::{self, IsTerminal, Write},
    num::NonZero,
    str::FromStr,
    time::Duration,
};
use swrite::{SWrite, swrite};
use tracing::debug;

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
struct SummaryBar {
    bar: ProgressBar,
    overflow_count: usize,
}

impl SummaryBar {
    /// Creates a new summary bar and inserts it into the multi-progress display.
    fn new(
        multi: &MultiProgress,
        displayed_count: usize,
        overflow_count: usize,
        styles: &Styles,
    ) -> Self {
        let bar = ProgressBar::hidden();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("             ... and {msg}")
                .expect("template is valid"),
        );

        // Add the summary bar after all displayed test bars (at the end
        // of the list). displayed_count is the number of tests
        // currently visible, so add it at (displayed_count + 1).
        let insert_pos = displayed_count + 1;

        bar.set_message(format!(
            "{} more {} running",
            overflow_count.style(styles.count),
            plural::tests_str(overflow_count)
        ));

        let summary_bar = multi.insert(insert_pos, bar);
        summary_bar.tick();

        Self {
            bar: summary_bar,
            overflow_count,
        }
    }

    fn set_overflow_count(&mut self, overflow_count: usize, styles: &Styles) {
        if overflow_count != self.overflow_count {
            self.bar.set_message(format!(
                "{} more {} running",
                overflow_count.style(styles.count),
                plural::tests_str(overflow_count)
            ));
            self.overflow_count = overflow_count;
        }
    }
}

#[derive(Debug)]
pub(super) struct TestProgressBars {
    used_bars: IdHashMap<TestProgressBar>,

    // Overflow tests in FIFO order (the IndexSet preserves insertion order).
    //
    // Tests are displayed if they're in used_bars but not in overflow_order.
    overflow_queue: IndexSet<(RustBinaryId, String)>,
    // Maximum number of tests to display.
    max_progress_running: MaxProgressRunning,
    // Actual maximum number of test displayed
    max_displayed: usize,

    // Summary bar showing the overflow count. This is Some if the summary bar
    // is currently displayed.
    summary_bar: Option<SummaryBar>,

    spinner_chars: &'static str,
}

impl TestProgressBars {
    fn new(spinner_chars: &'static str, max_progress_running: MaxProgressRunning) -> Self {
        Self {
            used_bars: IdHashMap::new(),
            overflow_queue: IndexSet::new(),
            max_progress_running,
            max_displayed: 0,
            summary_bar: None,
            spinner_chars,
        }
    }

    /// Adds a test to the bar if it's not present, or reset its duration to zero
    /// if it is.
    fn add_test(
        &mut self,
        multi: &MultiProgress,
        id: &TestInstanceId<'_>,
        prefix: String,
        running_count: usize,
        styles: &Styles,
    ) {
        if let Some(tb) = self.used_bars.get(id) {
            // Reset the bar to 0.
            tb.bar.set_prefix(prefix);
            // `ProgressBar` is a lightweight handle and calling methods like
            // `with_elapsed` on a clone also affects the original. Ideally
            // there would be a `set_elapsed` method on self.bar, though.
            tb.bar.clone().with_elapsed(Duration::ZERO);
        } else {
            self.add_test_inner(multi, id, prefix, running_count, styles);
        }
    }

    /// Sets the prefix for a test bar.
    fn set_prefix(&mut self, id: &TestInstanceId<'_>, prefix: String) {
        if let Some(tb) = self.used_bars.get(id) {
            tb.bar.set_prefix(prefix);
        }
    }

    fn remove_test(
        &mut self,
        multi: &MultiProgress,
        id: &TestInstanceId<'_>,
        running_count: usize,
        styles: &Styles,
    ) {
        if let Some(data) = self.used_bars.remove(id) {
            let test_key = (data.binary_id.clone(), data.test_name.clone());

            let was_overflow = self.overflow_queue.shift_remove(&test_key);

            if !was_overflow {
                // This test was displayed, so remove its bar from the
                // multi-progress.
                multi.remove(&data.bar);

                self.promote_next_overflow_test(multi);
            }

            // Update summary bar to reflect the new overflow count.
            self.update_summary_bar(multi, running_count, styles);
        }
    }

    fn on_run_continued(&self, delta: Duration) {
        for tb in &self.used_bars {
            let current_elapsed = tb.bar.elapsed();
            // `ProgressBar` is a lightweight handle and calling methods like
            // `with_elapsed` on a clone also affects the original. Ideally
            // there would be a `set_elapsed` method on self.bar, though.
            tb.bar
                .clone()
                .with_elapsed(current_elapsed.saturating_sub(delta));
        }
    }

    /// Adds a test to the progress display.
    fn add_test_inner(
        &mut self,
        multi: &MultiProgress,
        id: &TestInstanceId<'_>,
        prefix: String,
        running_count: usize,
        styles: &Styles,
    ) {
        let new_bar = ProgressBar::hidden();

        new_bar.set_style(
            ProgressStyle::default_bar()
                .template("{prefix:>10} {spinner} [{elapsed_precise:>9}] {wide_msg}")
                .expect("template to be valid")
                .tick_chars(self.spinner_chars),
        );
        if !prefix.is_empty() {
            new_bar.set_prefix(prefix);
        }
        new_bar.set_message(
            DisplayTestInstance::new(None, None, *id, &styles.list_styles).to_string(),
        );

        let displayed_count = self
            .used_bars
            .len()
            .saturating_sub(self.overflow_queue.len());

        let should_display = match self.max_progress_running {
            MaxProgressRunning::Infinite => true,
            MaxProgressRunning::Count(max) => displayed_count < max.get(),
        };

        let bar = if should_display {
            // Insert the bar at the correct position.
            //
            // displayed_count doesn't include this test yet, so insert at
            // (displayed_count + 1). This places it before the summary bar if
            // it exists, or before all used bars if not.
            let insert_pos = displayed_count + 1;
            let bar = multi.insert(insert_pos, new_bar);
            bar.tick();

            bar
        } else {
            // This test is in the overflow; track it in FIFO order.
            self.overflow_queue
                .insert((id.binary_id.clone(), id.test_name.to_owned()));
            new_bar
        };

        // We always add to used_bars, regardless of the display state.
        self.used_bars.insert_overwrite(TestProgressBar {
            binary_id: id.binary_id.clone(),
            test_name: id.test_name.to_owned(),
            bar,
        });

        // Update the summary bar to reflect the overflow count.
        self.update_summary_bar(multi, running_count, styles);

        self.max_displayed = max(self.max_displayed, self.len());
    }

    /// Updates or creates/removes the summary bar showing the overflow count.
    fn update_summary_bar(&mut self, multi: &MultiProgress, running_count: usize, styles: &Styles) {
        // Use the running count rather than the length of the overflow queue.
        // Since tests are added to the queue with a bit of a delay, using the
        // running count reduces flickering.
        let overflow_count = match self.max_progress_running {
            MaxProgressRunning::Count(count) => running_count.saturating_sub(count.get()),
            MaxProgressRunning::Infinite => {
                // No summary bar is displayed in this case.
                return;
            }
        };

        if overflow_count > 0 {
            if let Some(bar) = &mut self.summary_bar {
                bar.set_overflow_count(overflow_count, styles);
            } else {
                // Add a summary bar.
                let displayed_count = self
                    .used_bars
                    .len()
                    .saturating_sub(self.overflow_queue.len());

                self.summary_bar = Some(SummaryBar::new(
                    multi,
                    displayed_count,
                    overflow_count,
                    styles,
                ));
            }
        } else if let Some(summary_bar) = self.summary_bar.take() {
            // The above Option::take removes the summary bar from
            // self.summary_bar when the overflow count reaches 0.
            multi.remove(&summary_bar.bar);
        }
    }

    /// Promotes the next overflow test to be displayed.
    ///
    /// Returns true if a test was promoted.
    fn promote_next_overflow_test(&mut self, multi: &MultiProgress) -> bool {
        // Pop the first (oldest) overflow test from the FIFO IndexSet.
        let next_test_key = self.overflow_queue.shift_remove_index(0);

        if let Some(test_key) = next_test_key {
            // Find the test bar in used_bars.
            let tb = self
                .used_bars
                .iter()
                .find(|tb| tb.binary_id == test_key.0 && tb.test_name == test_key.1);

            if let Some(tb) = tb {
                // Make the bar visible by adding it to the multi-progress.
                //
                // We've already removed this test from overflow_order, so it's
                // included in this count even though it's not visually
                // displayed yet. Therefore we insert at displayed_count (not
                // +1) to place it before the summary bar if it exists.
                let displayed_count = self
                    .used_bars
                    .len()
                    .saturating_sub(self.overflow_queue.len());
                let insert_pos = displayed_count;
                multi.insert(insert_pos, tb.bar.clone());
                tb.bar.tick(); // Initial render
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    fn tick_all(&self) {
        // XXX tick in sequence rather than random order?
        for tb in &self.used_bars {
            tb.bar.tick();
        }
    }

    fn finish_and_clear_all(&self) {
        // XXX finish in sequence rather than random order?
        for tb in &self.used_bars {
            tb.bar.finish_and_clear();
        }
        if let Some(summary_bar) = &self.summary_bar {
            summary_bar.bar.finish_and_clear();
        }
    }

    fn len(&self) -> usize {
        self.used_bars.len() + self.summary_bar.is_some() as usize
    }

    fn new_free_bar(&self, multi_progress: &MultiProgress) -> ProgressBar {
        // push an bar with as much line as needed to match the max total lines displayed
        let missing_lines = self.max_displayed - self.len();
        let bar = ProgressBar::hidden();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template(&std::iter::repeat_n(" ", missing_lines).join("\n"))
                .expect("this template is valid"),
        );
        let bar = multi_progress.add(bar);
        bar.tick();
        bar
    }
}

#[derive(Debug)]
pub(super) struct TestProgressBar {
    binary_id: RustBinaryId,
    test_name: String,
    bar: ProgressBar,
}

impl IdHashItem for TestProgressBar {
    type Key<'a> = TestInstanceId<'a>;

    fn key(&self) -> Self::Key<'_> {
        TestInstanceId {
            binary_id: &self.binary_id,
            test_name: &self.test_name,
        }
    }

    id_upcast!();
}

#[derive(Debug)]
pub(super) struct ProgressBarState {
    multi_progress: MultiProgress,
    bar: ProgressBar,
    test_bars: Option<TestProgressBars>,
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
        spinner_chars: &'static str,
        show_running: bool,
        max_progress_running: MaxProgressRunning,
    ) -> Self {
        let multi_progress = MultiProgress::new();
        multi_progress.set_draw_target(Self::stderr_target());
        // multi_progress.set_move_cursor(true);

        let bar = multi_progress.add(ProgressBar::new(test_count as u64));
        let test_count_width = format!("{test_count}").len();
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

        let test_bars =
            show_running.then(|| TestProgressBars::new(spinner_chars, max_progress_running));

        Self {
            multi_progress,
            bar,
            test_bars,
            hidden_no_capture: false,
            hidden_run_paused: false,
            hidden_info_response: false,
            hidden_between_sub_runs: false,
        }
    }

    pub(super) fn tick(&mut self) {
        self.bar.tick();
        // Also tick all test bars.
        if let Some(test_bars) = &mut self.test_bars {
            test_bars.tick_all();
        }
    }

    pub(super) fn update_progress_bar(&mut self, event: &TestEvent<'_>, styles: &Styles) {
        let before_should_hide = self.should_hide();

        match &event.kind {
            TestEventKind::StressSubRunFinished { .. } => {
                // Clear all test bars to remove empty lines of output between
                // sub-runs.
                if let Some(test_bars) = &self.test_bars {
                    test_bars.finish_and_clear_all();
                }
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
                self.hidden_between_sub_runs = false;

                self.bar.set_prefix(progress_bar_prefix(
                    current_stats,
                    current_stats.cancel_reason,
                    styles,
                ));
                self.bar
                    .set_message(progress_bar_msg(current_stats, *running, styles));
                // If there are skipped tests, the initial run count will be lower than when constructed
                // in ProgressBar::new.
                self.bar.set_length(current_stats.initial_run_count as u64);
                self.bar.set_position(current_stats.finished_count as u64);

                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.update_summary_bar(&self.multi_progress, *running, styles);
                }
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.add_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        String::new(),
                        *running,
                        styles,
                    );
                }
            }
            TestEventKind::TestFinished {
                current_stats,
                running,
                test_instance,
                ..
            } => {
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.remove_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        *running,
                        styles,
                    );
                }

                self.hidden_between_sub_runs = false;

                self.bar.set_prefix(progress_bar_prefix(
                    current_stats,
                    current_stats.cancel_reason,
                    styles,
                ));
                self.bar
                    .set_message(progress_bar_msg(current_stats, *running, styles));
                // If there are skipped tests, the initial run count will be lower than when constructed
                // in ProgressBar::new.
                self.bar.set_length(current_stats.initial_run_count as u64);
                self.bar.set_position(current_stats.finished_count as u64);
            }
            TestEventKind::TestAttemptFailedWillRetry {
                test_instance,
                running,
                ..
            } => {
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.remove_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        *running,
                        styles,
                    );
                    // TODO: it would be nice to count the delay down rather
                    // than up. But it probably will require changes to
                    // indicatif to enable that (or for us to render time
                    // ourselves).
                    test_bars.add_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        "DELAY".style(styles.retry).to_string(),
                        *running,
                        styles,
                    );
                }
            }
            TestEventKind::TestRetryStarted {
                test_instance,
                running,
                ..
            } => {
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.remove_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        *running,
                        styles,
                    );
                    test_bars.add_test(
                        &self.multi_progress,
                        &test_instance.id(),
                        "RETRY".style(styles.retry).to_string(),
                        *running,
                        styles,
                    );
                }
            }
            TestEventKind::TestSlow { test_instance, .. } => {
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars
                        .set_prefix(&test_instance.id(), "SLOW".style(styles.skip).to_string());
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

                let delta = current_global_elapsed.saturating_sub(event.elapsed);
                if let Some(test_bars) = &mut self.test_bars {
                    test_bars.on_run_continued(delta);
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
            (false, true) => self.multi_progress.set_draw_target(Self::hidden_target()),
            (true, false) => self.multi_progress.set_draw_target(Self::stderr_target()),
            _ => {}
        }
    }

    pub(super) fn write_buf(&self, buf: &[u8]) -> io::Result<()> {
        // ProgressBar::println doesn't print status lines if the bar is
        // hidden. The suspend method prints it in all cases.
        // suspend forces a full redraw, so we call it only if there is
        // something in the buffer
        if !buf.is_empty() {
            let mut free_bar = None;
            if let Some(test_bars) = &self.test_bars {
                free_bar = Some(test_bars.new_free_bar(&self.multi_progress));
            }
            let res = self
                .multi_progress
                .suspend(|| std::io::stderr().write_all(buf));
            if let Some(bar) = &free_bar {
                self.multi_progress.remove(bar);
            }
            res
        } else {
            Ok(())
        }
    }

    #[inline]
    pub(super) fn finish_and_clear(&self) {
        self.bar.finish_and_clear();
        if let Some(test_bars) = &self.test_bars {
            test_bars.finish_and_clear_all();
        }
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
        self.multi_progress.is_hidden()
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
            TestEventKind::TestShowProgress { .. }
            | TestEventKind::TestSlow { .. }
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
