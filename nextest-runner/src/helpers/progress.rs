// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::cargo_config::{CargoConfigs, DiscoveredConfig};
use anstyle_progress::{TermProgress, supports_term_progress};
use indicatif::ProgressStyle;
use std::{
    env,
    io::{self, Write},
};
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
/// and rely on ProgressBarState::print_and_force_redraw to always flush the
/// terminal.
pub(crate) const PROGRESS_REFRESH_RATE_HZ: u8 = 1;

pub(crate) fn progress_bar_style(progress_chars: &str, suffix: &str) -> ProgressStyle {
    // Create the template. This is a little confusing -- {{foo}} is what's
    // passed into the ProgressBar, while {bar} is inserted by the format!()
    // statement.
    let template = format!("{{prefix:>12}} [{{elapsed_precise:>9}}] {{wide_bar}} {suffix}");
    ProgressStyle::default_bar()
        .progress_chars(progress_chars)
        .template(&template)
        .expect("template is known to be valid")
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

        let show = supports_term_progress(is_terminal);
        debug!(is_terminal, show, "autodetected terminal progress support");
        if show { Self::Yes } else { Self::No }
    }
}

/// OSC 9 terminal progress reporting.
#[derive(Default)]
pub(crate) struct TerminalProgress {
    last_value: TermProgress,
}

impl TerminalProgress {
    pub(crate) fn new(show: ShowTerminalProgress) -> Option<Self> {
        match show {
            ShowTerminalProgress::Yes => Some(Self::default()),
            ShowTerminalProgress::No => None,
        }
    }

    pub(crate) fn set(&mut self, value: TermProgress) {
        self.last_value = value;
    }

    pub(crate) fn emit(&self) {
        // Use the write! macro rather than eprint! so that we ignore errors
        // rather than panicking. Terminal progress reporting is cosmetic and
        // best-effort.
        let _ = write!(io::stderr(), "{}", self.last_value);
    }
}

pub(crate) fn term_progress_percent(done: usize, total: usize) -> u8 {
    if total == 0 {
        return 100;
    }
    ((done as f64 / total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::term_progress_percent;

    #[test]
    fn term_progress_percent_boundaries() {
        // Handle 0/0 properly.
        assert_eq!(term_progress_percent(0, 0), 100);
        assert_eq!(term_progress_percent(0, 10), 0);
        assert_eq!(term_progress_percent(1, 2), 50);
        assert_eq!(term_progress_percent(1, 4), 25);
        assert_eq!(term_progress_percent(10, 10), 100);

        // We round away from zero (12.5 -> 13, 37.5 -> 38).
        assert_eq!(term_progress_percent(1, 8), 13);
        assert_eq!(term_progress_percent(3, 8), 38);

        // We stay within 0..=100 for the percentage.
        assert_eq!(term_progress_percent(11, 10), 100);
    }
}
