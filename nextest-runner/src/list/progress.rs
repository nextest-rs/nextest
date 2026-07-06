// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    helpers::{
        ThemeCharacters, decimal_char_width, plural,
        progress::{
            PROGRESS_REFRESH_RATE_HZ, ShowTerminalProgress, TerminalProgress, progress_bar_style,
            term_progress_percent,
        },
    },
    reporter::ShowProgress,
};
use anstyle_progress::TermProgress;
use indicatif::{ProgressBar, ProgressDrawTarget};
use owo_colors::{OwoColorize, Style};
use std::{
    env,
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

const LIST_PROGRESS_REVEAL_DELAY: Duration = Duration::from_secs(2);

const LIST_PROGRESS_TICK_INTERVAL: Duration = Duration::from_millis(100);

const LIST_PROGRESS_DELAY_ENV: &str = "__NEXTEST_LIST_PROGRESS_DELAY_MS";

pub(crate) enum ListProgressEvent {
    Tick,
    BinaryProcessed,
}

/// Options for the list progress reporter.
#[derive(Debug)]
pub struct ListProgressOptions {
    show_progress: ShowProgress,
    show_terminal_progress: ShowTerminalProgress,
    theme_characters: ThemeCharacters,
    should_colorize: bool,
}

impl ListProgressOptions {
    /// Creates a new set of list progress reporter options.
    pub fn new(
        show_progress: ShowProgress,
        show_terminal_progress: ShowTerminalProgress,
        theme_characters: ThemeCharacters,
        should_colorize: bool,
    ) -> Self {
        Self {
            show_progress,
            show_terminal_progress,
            theme_characters,
            should_colorize,
        }
    }
}

pub(crate) struct ListProgressReporter {
    bar: Option<ProgressBar>,
    term_progress: Option<TerminalProgress>,
    total: usize,
    listed: usize,
    state: RevealState,
}

#[derive(Clone, Copy, Debug)]
enum RevealState {
    Hidden { start: Instant, delay: Duration },
    Revealed,
    Finished,
}

impl ListProgressReporter {
    pub(crate) fn new(total: usize, options: &ListProgressOptions) -> Self {
        let is_terminal = io::stderr().is_terminal();
        let is_ci = is_ci::uncached();
        let bar = list_bar_enabled(options.show_progress, is_terminal, is_ci).then(|| {
            make_bar(
                total,
                options.theme_characters.progress_chars(),
                options.should_colorize,
            )
        });
        let term_progress = TerminalProgress::new(options.show_terminal_progress);
        Self::from_parts(total, bar, term_progress, reveal_delay_from_env())
    }

    fn from_parts(
        total: usize,
        bar: Option<ProgressBar>,
        term_progress: Option<TerminalProgress>,
        reveal_delay: Duration,
    ) -> Self {
        Self {
            bar,
            term_progress,
            total,
            listed: 0,
            state: RevealState::Hidden {
                start: Instant::now(),
                delay: reveal_delay,
            },
        }
    }

    pub(crate) fn tick_interval(&self) -> Duration {
        LIST_PROGRESS_TICK_INTERVAL
    }

    pub(crate) fn handle_event(&mut self, event: ListProgressEvent) {
        match event {
            ListProgressEvent::BinaryProcessed => {
                self.listed += 1;
                debug_assert!(
                    self.listed <= self.total,
                    "at most {} binaries can be processed, but saw {}",
                    self.total,
                    self.listed,
                );
                if let Some(bar) = &self.bar {
                    bar.inc(1);
                }
                if self.is_revealed() {
                    // Terminal progress doesn't have a timer, so re-emit it
                    // only when we learn that a new binary has been listed.
                    //
                    // Worth noting that we do not install a signal handler
                    // during this phase, so in case of a Ctrl-C we don't clear
                    // terminal progress. A signal handler is quite tricky here
                    // and adds complexity for minimal benefit (unlike at
                    // runtime, where signals are part of the test lifecycle
                    // state machine).
                    let value = self.current_term_progress();
                    self.emit_terminal_progress(value);
                }
            }
            ListProgressEvent::Tick => {
                if let RevealState::Hidden { start, delay } = self.state
                    && start.elapsed() >= delay
                {
                    self.reveal();
                }
                if self.is_revealed()
                    && let Some(bar) = &self.bar
                {
                    // Force a redraw each tick so the elapsed timer advances.
                    bar.force_draw();
                }
            }
        }
    }

    fn finish(&mut self) {
        let was_revealed = match self.state {
            RevealState::Hidden { .. } => false,
            RevealState::Revealed => true,
            RevealState::Finished => return,
        };
        self.state = RevealState::Finished;
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
        if was_revealed {
            self.emit_terminal_progress(TermProgress::remove());
        }
    }

    fn reveal(&mut self) {
        self.state = RevealState::Revealed;
        if let Some(bar) = &self.bar {
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(PROGRESS_REFRESH_RATE_HZ));
        }
        let value = self.current_term_progress();
        self.emit_terminal_progress(value);
    }

    fn emit_terminal_progress(&mut self, value: TermProgress) {
        if let Some(term_progress) = &mut self.term_progress {
            term_progress.set(value);
            term_progress.emit();
        }
    }

    fn current_term_progress(&self) -> TermProgress {
        TermProgress::start().percent(term_progress_percent(self.listed, self.total))
    }

    fn is_revealed(&self) -> bool {
        match self.state {
            RevealState::Revealed => true,
            RevealState::Hidden { .. } | RevealState::Finished => false,
        }
    }
}

impl Drop for ListProgressReporter {
    fn drop(&mut self) {
        self.finish();
    }
}

fn list_bar_enabled(show_progress: ShowProgress, is_terminal: bool, is_ci: bool) -> bool {
    match show_progress {
        ShowProgress::Auto { .. } | ShowProgress::Running => is_terminal && !is_ci,
        ShowProgress::Counter | ShowProgress::None => false,
    }
}

fn make_bar(total: usize, progress_chars: &str, should_colorize: bool) -> ProgressBar {
    let styles = ListProgressStyles::new(should_colorize);
    let bar = ProgressBar::new(total as u64);
    // We start with the progress bar being hidden, and only reveal it after the
    // delay. This makes fast listings be silent while showing progress for
    // slower ones.
    bar.set_draw_target(ProgressDrawTarget::hidden());
    let count_width = decimal_char_width(total);
    let suffix = format!("{{pos:>{count_width}}}/{{len:{count_width}}} {{msg}}");
    bar.set_style(progress_bar_style(progress_chars, &suffix));
    bar.set_prefix("Listing".style(styles.label).to_string());
    bar.set_message(plural::binaries_str(total).style(styles.label).to_string());
    bar
}

struct ListProgressStyles {
    label: Style,
}

impl ListProgressStyles {
    fn new(should_colorize: bool) -> Self {
        let label = if should_colorize {
            Style::new().green().bold()
        } else {
            Style::new()
        };
        Self { label }
    }
}

fn reveal_delay_from_env() -> Duration {
    let Some(raw) = env::var_os(LIST_PROGRESS_DELAY_ENV) else {
        return LIST_PROGRESS_REVEAL_DELAY;
    };
    let raw = raw.to_str().unwrap_or_else(|| {
        panic!("{LIST_PROGRESS_DELAY_ENV} contains non-UTF-8 bytes");
    });
    let ms: u64 = raw.parse().unwrap_or_else(|err| {
        panic!(
            "{LIST_PROGRESS_DELAY_ENV}={raw:?} is not a valid number of \
             milliseconds: {err}"
        )
    });
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_reporter(total: usize, reveal_delay: Duration) -> ListProgressReporter {
        ListProgressReporter::from_parts(
            total,
            Some(make_bar(total, "=> ", false)),
            None,
            reveal_delay,
        )
    }

    #[test]
    fn list_bar_enabled_gating() {
        let bar_modes = [
            ShowProgress::Auto {
                suppress_success: false,
            },
            ShowProgress::Auto {
                suppress_success: true,
            },
            ShowProgress::Running,
        ];
        for mode in bar_modes {
            assert!(
                list_bar_enabled(mode, true, false),
                "{mode:?}: interactive terminal shows a bar"
            );
            assert!(
                !list_bar_enabled(mode, false, false),
                "{mode:?}: piped output shows no bar"
            );
            assert!(
                !list_bar_enabled(mode, true, true),
                "{mode:?}: CI shows no bar"
            );
            assert!(
                !list_bar_enabled(mode, false, true),
                "{mode:?}: piped in CI shows no bar"
            );
        }

        for mode in [ShowProgress::Counter, ShowProgress::None] {
            for is_terminal in [true, false] {
                for is_ci in [true, false] {
                    assert!(
                        !list_bar_enabled(mode, is_terminal, is_ci),
                        "{mode:?} never shows a list bar"
                    );
                }
            }
        }
    }

    #[test]
    fn bar_hidden_until_reveal_delay() {
        let mut reporter = enabled_reporter(10, Duration::MAX);
        assert!(!reporter.is_revealed(), "the bar starts unrevealed");

        reporter.handle_event(ListProgressEvent::Tick);
        assert!(
            !reporter.is_revealed(),
            "the bar is not revealed before the reveal delay elapses"
        );

        let mut reporter = enabled_reporter(10, Duration::ZERO);
        assert!(!reporter.is_revealed(), "the bar starts unrevealed");

        reporter.handle_event(ListProgressEvent::Tick);
        assert!(
            reporter.is_revealed(),
            "the bar is revealed once the reveal delay elapses"
        );
    }

    #[test]
    fn binary_processed_advances_position() {
        let mut reporter = enabled_reporter(4, Duration::MAX);
        let bar = reporter.bar.as_ref().expect("bar is enabled").clone();
        assert_eq!(bar.position(), 0);

        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        reporter.handle_event(ListProgressEvent::BinaryProcessed);

        assert_eq!(bar.position(), 2);
        assert_eq!(reporter.listed, 2);
    }

    fn terminal_only_reporter(total: usize, reveal_delay: Duration) -> ListProgressReporter {
        let term_progress = TerminalProgress::new(ShowTerminalProgress::Yes)
            .expect("Yes yields a terminal progress reporter");
        ListProgressReporter::from_parts(total, None, Some(term_progress), reveal_delay)
    }

    fn last_osc(reporter: &ListProgressReporter) -> String {
        reporter
            .term_progress
            .as_ref()
            .expect("terminal progress is enabled")
            .last_value()
            .to_string()
    }

    #[test]
    fn terminal_progress_silent_until_revealed() {
        let mut reporter = terminal_only_reporter(4, Duration::MAX);

        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        reporter.handle_event(ListProgressEvent::Tick);
        assert!(
            last_osc(&reporter).is_empty(),
            "no OSC value is emitted before the reveal delay"
        );

        reporter.finish();
        assert!(
            last_osc(&reporter).is_empty(),
            "a listing that never revealed emits no Remove"
        );
    }

    #[test]
    fn terminal_progress_emits_after_reveal() {
        let mut reporter = terminal_only_reporter(4, Duration::ZERO);

        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        assert!(
            last_osc(&reporter).is_empty(),
            "no OSC value is emitted before the first tick reveals"
        );

        reporter.handle_event(ListProgressEvent::Tick);
        assert!(reporter.is_revealed());
        assert_eq!(
            last_osc(&reporter),
            TermProgress::start().percent(50).to_string(),
            "reveal emits the current percentage"
        );

        reporter.handle_event(ListProgressEvent::BinaryProcessed);
        assert_eq!(
            last_osc(&reporter),
            TermProgress::start().percent(75).to_string(),
            "a subsequent binary re-emits the updated percentage"
        );

        reporter.finish();
        assert_eq!(
            last_osc(&reporter),
            TermProgress::remove().to_string(),
            "finish emits Remove once revealed"
        );
    }

    #[test]
    fn reveal_delay_from_env_values() {
        unsafe { env::remove_var(LIST_PROGRESS_DELAY_ENV) };
        assert_eq!(
            reveal_delay_from_env(),
            LIST_PROGRESS_REVEAL_DELAY,
            "an unset variable falls back to the default delay"
        );

        unsafe { env::set_var(LIST_PROGRESS_DELAY_ENV, "250") };
        assert_eq!(
            reveal_delay_from_env(),
            Duration::from_millis(250),
            "a valid value is parsed as milliseconds"
        );
    }

    #[test]
    #[should_panic(expected = "is not a valid number of milliseconds")]
    fn reveal_delay_from_env_panics_on_malformed_value() {
        unsafe { env::set_var(LIST_PROGRESS_DELAY_ENV, "0ms") };
        reveal_delay_from_env();
    }
}
