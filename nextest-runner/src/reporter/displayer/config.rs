// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Consolidated display configuration: resolves all display decisions (progress
//! bar, counter, status levels) in a single place.

use super::{ShowProgress, status_level::StatusLevels};
use crate::reporter::displayer::{FinalStatusLevel, StatusLevel};

/// Unresolved display configuration passed to the displayer.
///
/// The displayer resolves this into a [`ResolvedDisplay`] based on the
/// progress display mode, terminal interactivity, and CI detection.
pub(crate) struct DisplayConfig {
    /// The progress display mode.
    pub(crate) show_progress: ShowProgress,

    /// Whether test output capture is disabled (`--no-capture`).
    pub(crate) no_capture: bool,

    /// Explicit override (from CLI `--status-level`), or `None` to use
    /// context-dependent defaults.
    pub(crate) status_level: Option<StatusLevel>,

    /// Explicit override (from CLI `--final-status-level`).
    pub(crate) final_status_level: Option<FinalStatusLevel>,

    /// Status level from the nextest profile, used as the default when no
    /// explicit override is set and no context-specific default applies.
    pub(crate) profile_status_level: StatusLevel,

    /// Final status level from the nextest profile, used as the default when
    /// no explicit override is set and no context-specific default applies.
    pub(crate) profile_final_status_level: FinalStatusLevel,
}

/// The fully resolved display decisions.
///
/// Produced by [`DisplayConfig::resolve()`].
pub(crate) struct ResolvedDisplay {
    /// What kind of progress indicator to show.
    pub(crate) progress_display: ProgressDisplay,

    /// The resolved status levels.
    pub(crate) status_levels: StatusLevels,
}

/// The kind of progress indicator to display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProgressDisplay {
    /// Show a progress bar with running tests.
    Bar,

    /// Show a counter (e.g. "(1/10)") on each status line.
    Counter,

    /// No progress indicator.
    None,
}

impl DisplayConfig {
    /// Creates a config with explicit status level overrides and no profile
    /// defaults.
    ///
    /// Used by replay, where status levels are always explicitly set.
    pub(crate) fn with_overrides(
        show_progress: ShowProgress,
        no_capture: bool,
        status_level: StatusLevel,
        final_status_level: FinalStatusLevel,
    ) -> Self {
        DisplayConfig {
            show_progress,
            no_capture,
            status_level: Some(status_level),
            final_status_level: Some(final_status_level),
            // These are unused because the overrides above are always Some,
            // but we set them to the same values for consistency.
            profile_status_level: status_level,
            profile_final_status_level: final_status_level,
        }
    }

    /// Resolves this configuration into concrete display decisions.
    ///
    /// Resolution depends on `is_terminal` and `is_ci`, both of which are
    /// passed in (rather than queried) for testability.
    ///
    /// The behavioral table:
    ///
    /// | `show_progress`                      | `bar_available` | bar | counter | status defaults |
    /// |--------------------------------------|-----------------|-----|---------|-----------------|
    /// | `Auto { suppress_success: false }`   | true            | yes | no      | profile         |
    /// | `Auto { suppress_success: true }`    | true            | yes | no      | Slow / None     |
    /// | `Auto { suppress_success: true }`    | false           | no  | yes     | profile         |
    /// | `Auto { suppress_success: false }`   | false           | no  | yes     | profile         |
    /// | `Running`                            | true            | yes | no      | profile         |
    /// | `Running`                            | false           | no  | no      | profile         |
    /// | `Counter`                            | *               | no  | yes     | profile         |
    /// | `None`                               | *               | no  | no      | profile         |
    ///
    /// In no-capture mode, the status level is raised to at least
    /// [`StatusLevel::Pass`].
    pub(crate) fn resolve(&self, is_terminal: bool, is_ci: bool) -> ResolvedDisplay {
        // The progress bar requires all three conditions:
        // - Capture enabled: with --no-capture, stderr is passed directly to
        //   child processes, making a progress bar incompatible. (A future
        //   pty-based approach could lift this, but would require curses-like
        //   handling for partial lines.)
        // - Not CI: some CI environments pretend to be terminals but don't
        //   render progress bars correctly.
        // - Terminal output: indicatif requires a real terminal target.
        let bar_available = !self.no_capture && !is_ci && is_terminal;
        let (sl, fsl) = (self.profile_status_level, self.profile_final_status_level);

        let mut resolved = match (self.show_progress, bar_available) {
            // Auto + interactive: show bar, profile defaults.
            (
                ShowProgress::Auto {
                    suppress_success: false,
                },
                true,
            ) => ResolvedDisplay {
                progress_display: ProgressDisplay::Bar,
                status_levels: self.apply_overrides(sl, fsl),
            },
            // Auto + suppress_success + interactive: show bar, hide successful output.
            (
                ShowProgress::Auto {
                    suppress_success: true,
                },
                true,
            ) => ResolvedDisplay {
                progress_display: ProgressDisplay::Bar,
                status_levels: self.apply_overrides(StatusLevel::Slow, FinalStatusLevel::None),
            },
            // Auto + suppress_success + non-interactive: suppress_success
            // is irrelevant without a progress bar; use profile defaults.
            (
                ShowProgress::Auto {
                    suppress_success: true,
                },
                false,
            ) => {
                tracing::debug!(
                    is_terminal,
                    is_ci,
                    no_capture = self.no_capture,
                    "suppress_success requested but progress bar is unavailable; \
                     using profile defaults",
                );
                ResolvedDisplay {
                    progress_display: ProgressDisplay::Counter,
                    status_levels: self.apply_overrides(sl, fsl),
                }
            }
            // Auto + non-interactive: show counter, profile defaults.
            (
                ShowProgress::Auto {
                    suppress_success: false,
                },
                false,
            ) => ResolvedDisplay {
                progress_display: ProgressDisplay::Counter,
                status_levels: self.apply_overrides(sl, fsl),
            },
            // Running + interactive: show bar.
            (ShowProgress::Running, true) => ResolvedDisplay {
                progress_display: ProgressDisplay::Bar,
                status_levels: self.apply_overrides(sl, fsl),
            },
            // Running + non-interactive: no visible progress.
            (ShowProgress::Running, false) => ResolvedDisplay {
                progress_display: ProgressDisplay::None,
                status_levels: self.apply_overrides(sl, fsl),
            },
            // Counter: always counter, never bar.
            (ShowProgress::Counter, _) => ResolvedDisplay {
                progress_display: ProgressDisplay::Counter,
                status_levels: self.apply_overrides(sl, fsl),
            },
            // None: no progress display of any kind.
            (ShowProgress::None, _) => ResolvedDisplay {
                progress_display: ProgressDisplay::None,
                status_levels: self.apply_overrides(sl, fsl),
            },
        };

        // In no-capture mode, raise status level to at least Pass.
        if self.no_capture {
            resolved.status_levels.status_level =
                resolved.status_levels.status_level.max(StatusLevel::Pass);
        }

        resolved
    }

    fn apply_overrides(
        &self,
        default_sl: StatusLevel,
        default_fsl: FinalStatusLevel,
    ) -> StatusLevels {
        StatusLevels {
            status_level: self.status_level.unwrap_or(default_sl),
            final_status_level: self.final_status_level.unwrap_or(default_fsl),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The profile defaults used in resolve tests.
    const DEFAULT_SL: StatusLevel = StatusLevel::Pass;
    const DEFAULT_FSL: FinalStatusLevel = FinalStatusLevel::Flaky;

    fn make_config(
        show_progress: ShowProgress,
        no_capture: bool,
        status_level: Option<StatusLevel>,
        final_status_level: Option<FinalStatusLevel>,
    ) -> DisplayConfig {
        DisplayConfig {
            show_progress,
            no_capture,
            status_level,
            final_status_level,
            profile_status_level: DEFAULT_SL,
            profile_final_status_level: DEFAULT_FSL,
        }
    }

    // -- Non-suppress modes use profile defaults --

    /// For all ShowProgress variants *except* `Auto { suppress_success: true }` with
    /// a progress bar, the resolved status levels should use profile defaults
    /// (when no CLI overrides are set).
    #[test]
    fn resolve_uses_profile_defaults() {
        let non_suppress_modes = [
            ShowProgress::Auto {
                suppress_success: false,
            },
            ShowProgress::None,
            ShowProgress::Counter,
            ShowProgress::Running,
        ];

        for show_progress in non_suppress_modes {
            // Interactive terminal, not CI.
            let config = make_config(show_progress, false, None, None);
            let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
            assert_eq!(
                resolved.status_levels.status_level, DEFAULT_SL,
                "show_progress={show_progress:?}, is_terminal=true, is_ci=false"
            );
            assert_eq!(
                resolved.status_levels.final_status_level, DEFAULT_FSL,
                "show_progress={show_progress:?}, is_terminal=true, is_ci=false"
            );

            // Non-interactive (piped).
            let config = make_config(show_progress, false, None, None);
            let resolved = config.resolve(/* is_terminal */ false, /* is_ci */ false);
            assert_eq!(
                resolved.status_levels.status_level, DEFAULT_SL,
                "show_progress={show_progress:?}, is_terminal=false, is_ci=false"
            );
            assert_eq!(
                resolved.status_levels.final_status_level, DEFAULT_FSL,
                "show_progress={show_progress:?}, is_terminal=false, is_ci=false"
            );

            // CI environment.
            let config = make_config(show_progress, false, None, None);
            let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ true);
            assert_eq!(
                resolved.status_levels.status_level, DEFAULT_SL,
                "show_progress={show_progress:?}, is_terminal=true, is_ci=true"
            );
            assert_eq!(
                resolved.status_levels.final_status_level, DEFAULT_FSL,
                "show_progress={show_progress:?}, is_terminal=true, is_ci=true"
            );
        }

        // Auto { suppress_success: true } without a progress bar also uses profile
        // defaults (non-interactive).
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(/* is_terminal */ false, /* is_ci */ false);
        assert_eq!(resolved.status_levels.status_level, DEFAULT_SL);
        assert_eq!(resolved.status_levels.final_status_level, DEFAULT_FSL);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);

        // Auto { suppress_success: true } in CI also uses profile defaults.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ true);
        assert_eq!(resolved.status_levels.status_level, DEFAULT_SL);
        assert_eq!(resolved.status_levels.final_status_level, DEFAULT_FSL);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);
    }

    /// `Auto { suppress_success: true }` with a progress bar hides successful output.
    #[test]
    fn resolve_suppress_success_with_progress_bar() {
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Slow,
            "suppress_success + progress bar should default to Slow"
        );
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::None,
            "suppress_success + progress bar should default to None"
        );
        assert_eq!(resolved.progress_display, ProgressDisplay::Bar);
    }

    // -- CLI overrides --

    /// Explicit CLI overrides always win, even in suppress_success with a progress
    /// bar.
    #[test]
    fn resolve_cli_overrides_win() {
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            Some(StatusLevel::Skip),
            Some(FinalStatusLevel::Pass),
        );

        // suppress_success + progress bar: CLI override should still win.
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Skip,
            "CLI override wins over suppress_success defaults"
        );
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::Pass,
            "CLI override wins over suppress_success defaults"
        );

        // Normal auto mode: CLI override should also win.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: false,
            },
            false,
            Some(StatusLevel::Skip),
            Some(FinalStatusLevel::Pass),
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(resolved.status_levels.status_level, StatusLevel::Skip);
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::Pass
        );

        // Non-auto mode: CLI override should also win.
        let config = make_config(
            ShowProgress::Running,
            false,
            Some(StatusLevel::Skip),
            Some(FinalStatusLevel::Pass),
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(resolved.status_levels.status_level, StatusLevel::Skip);
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::Pass
        );
    }

    /// Partial CLI overrides: only the overridden level uses the explicit
    /// value, the other falls back to the contextual default.
    #[test]
    fn resolve_partial_cli_overrides() {
        // Override only status_level.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            Some(StatusLevel::All),
            None,
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::All,
            "CLI override for status_level"
        );
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::None,
            "suppress_success default for final_status_level"
        );

        // Override only final_status_level.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            false,
            None,
            Some(FinalStatusLevel::All),
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Slow,
            "suppress_success default for status_level"
        );
        assert_eq!(
            resolved.status_levels.final_status_level,
            FinalStatusLevel::All,
            "CLI override for final_status_level"
        );
    }

    // -- no_capture mode --

    /// No-capture mode raises status_level to at least Pass.
    #[test]
    fn resolve_no_capture_raises_status_level() {
        // Without CLI override, profile default Pass is unchanged.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: false,
            },
            true,
            None,
            None,
        );
        let resolved = config.resolve(/* is_terminal */ false, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Pass,
            "no_capture raises to at least Pass (default was Pass)"
        );

        // With suppress_success + bar available: Slow is raised to Pass.
        // Note: no_capture prevents the bar from being available, so
        // bar_available=false and suppress_success is irrelevant. The profile default
        // (Pass) is used.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: true,
            },
            true,
            None,
            None,
        );
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Pass,
            "no_capture prevents bar, so profile default Pass is used"
        );
        // With no_capture, bar is not available so suppress_success falls through to
        // the Auto non-interactive branch which uses profile defaults.
        assert_eq!(
            resolved.status_levels.final_status_level, DEFAULT_FSL,
            "no_capture prevents bar, so profile final default is used"
        );
        assert_eq!(
            resolved.progress_display,
            ProgressDisplay::Counter,
            "no_capture prevents bar"
        );

        // With CLI override below Pass, no_capture raises it.
        let config = make_config(ShowProgress::Running, true, Some(StatusLevel::Fail), None);
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::Pass,
            "no_capture raises CLI override Fail to Pass"
        );

        // With CLI override above Pass, no_capture preserves it.
        let config = make_config(ShowProgress::Running, true, Some(StatusLevel::All), None);
        let resolved = config.resolve(/* is_terminal */ true, /* is_ci */ false);
        assert_eq!(
            resolved.status_levels.status_level,
            StatusLevel::All,
            "no_capture preserves CLI override All"
        );
    }

    // -- Progress bar and counter decisions --

    /// Auto mode: bar in interactive terminal, counter otherwise.
    #[test]
    fn resolve_auto_progress_decisions() {
        // Interactive terminal.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: false,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(true, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::Bar);

        // Non-interactive.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: false,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(false, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);

        // CI pretending to be a terminal.
        let config = make_config(
            ShowProgress::Auto {
                suppress_success: false,
            },
            false,
            None,
            None,
        );
        let resolved = config.resolve(true, true);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);
    }

    /// Running mode: bar in interactive terminal, nothing otherwise.
    #[test]
    fn resolve_running_progress_decisions() {
        // Interactive terminal.
        let config = make_config(ShowProgress::Running, false, None, None);
        let resolved = config.resolve(true, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::Bar);

        // Non-interactive.
        let config = make_config(ShowProgress::Running, false, None, None);
        let resolved = config.resolve(false, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::None);

        // CI pretending to be a terminal: no bar, no counter.
        let config = make_config(ShowProgress::Running, false, None, None);
        let resolved = config.resolve(true, true);
        assert_eq!(resolved.progress_display, ProgressDisplay::None);
    }

    /// Counter mode: always counter, never bar.
    #[test]
    fn resolve_counter_progress_decisions() {
        let config = make_config(ShowProgress::Counter, false, None, None);
        let resolved = config.resolve(true, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);

        let config = make_config(ShowProgress::Counter, false, None, None);
        let resolved = config.resolve(false, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::Counter);
    }

    /// None mode: no progress display.
    #[test]
    fn resolve_none_progress_decisions() {
        let config = make_config(ShowProgress::None, false, None, None);
        let resolved = config.resolve(true, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::None);

        let config = make_config(ShowProgress::None, false, None, None);
        let resolved = config.resolve(false, false);
        assert_eq!(resolved.progress_display, ProgressDisplay::None);
    }
}
