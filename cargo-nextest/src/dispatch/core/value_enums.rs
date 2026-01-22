// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Value enums shared across core commands.

use crate::errors::CargoMessageFormatError;
use clap::ValueEnum;
use nextest_metadata::BuildPlatform;
use nextest_runner::{
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
    user_config::elements::UiShowProgress,
};
use std::collections::HashSet;

/// Platform filter options.
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(crate) enum PlatformFilterOpts {
    Target,
    Host,
    #[default]
    Any,
}

impl From<PlatformFilterOpts> for Option<BuildPlatform> {
    fn from(opt: PlatformFilterOpts) -> Self {
        match opt {
            PlatformFilterOpts::Target => Some(BuildPlatform::Target),
            PlatformFilterOpts::Host => Some(BuildPlatform::Host),
            PlatformFilterOpts::Any => None,
        }
    }
}

/// List type options.
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(crate) enum ListType {
    #[default]
    Full,
    BinariesOnly,
}

/// Message format options for list command.
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub(crate) enum MessageFormatOpts {
    /// Auto-detect: **human** if stdout is an interactive terminal, **oneline**
    /// otherwise.
    #[default]
    Auto,
    /// A human-readable output format.
    Human,
    /// One test per line.
    Oneline,
    /// JSON with no whitespace.
    Json,
    /// JSON, prettified.
    JsonPretty,
}

impl MessageFormatOpts {
    pub(crate) fn to_output_format(
        self,
        verbose: bool,
        is_terminal: bool,
    ) -> nextest_runner::list::OutputFormat {
        use nextest_runner::list::{OutputFormat, SerializableFormat};

        match self {
            Self::Auto => {
                if is_terminal {
                    OutputFormat::Human { verbose }
                } else {
                    OutputFormat::Oneline { verbose }
                }
            }
            Self::Human => OutputFormat::Human { verbose },
            Self::Oneline => OutputFormat::Oneline { verbose },
            Self::Json => OutputFormat::Serializable(SerializableFormat::Json),
            Self::JsonPretty => OutputFormat::Serializable(SerializableFormat::JsonPretty),
        }
    }

    /// Returns true if this format is human-readable (suitable for paging).
    ///
    /// Machine-readable formats (JSON) should not be paged.
    pub(crate) fn is_human_readable(self) -> bool {
        match self {
            Self::Auto | Self::Human | Self::Oneline => true,
            Self::Json | Self::JsonPretty => false,
        }
    }
}

/// Run ignored test options.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum RunIgnoredOpt {
    /// Run non-ignored tests.
    Default,

    /// Run ignored tests.
    #[clap(alias = "ignored-only")]
    Only,

    /// Run both ignored and non-ignored tests.
    All,
}

impl From<RunIgnoredOpt> for nextest_runner::test_filter::RunIgnored {
    fn from(opt: RunIgnoredOpt) -> Self {
        match opt {
            RunIgnoredOpt::Default => Self::Default,
            RunIgnoredOpt::Only => Self::Only,
            RunIgnoredOpt::All => Self::All,
        }
    }
}

/// No tests behavior options.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum NoTestsBehaviorOpt {
    /// Automatically determine behavior, defaulting to `fail`.
    #[default]
    Auto,

    /// Silently exit with code 0.
    Pass,

    /// Produce a warning and exit with code 0.
    Warn,

    /// Produce an error message and exit with code 4.
    #[clap(alias = "error")]
    Fail,
}

/// Test output display options.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum TestOutputDisplayOpt {
    Immediate,
    ImmediateFinal,
    Final,
    Never,
}

impl From<TestOutputDisplayOpt> for TestOutputDisplay {
    fn from(opt: TestOutputDisplayOpt) -> Self {
        match opt {
            TestOutputDisplayOpt::Immediate => TestOutputDisplay::Immediate,
            TestOutputDisplayOpt::ImmediateFinal => TestOutputDisplay::ImmediateFinal,
            TestOutputDisplayOpt::Final => TestOutputDisplay::Final,
            TestOutputDisplayOpt::Never => TestOutputDisplay::Never,
        }
    }
}

/// Status level options.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum StatusLevelOpt {
    None,
    Fail,
    Retry,
    Slow,
    Leak,
    Pass,
    Skip,
    All,
}

impl From<StatusLevelOpt> for StatusLevel {
    fn from(opt: StatusLevelOpt) -> Self {
        match opt {
            StatusLevelOpt::None => StatusLevel::None,
            StatusLevelOpt::Fail => StatusLevel::Fail,
            StatusLevelOpt::Retry => StatusLevel::Retry,
            StatusLevelOpt::Slow => StatusLevel::Slow,
            StatusLevelOpt::Leak => StatusLevel::Leak,
            StatusLevelOpt::Pass => StatusLevel::Pass,
            StatusLevelOpt::Skip => StatusLevel::Skip,
            StatusLevelOpt::All => StatusLevel::All,
        }
    }
}

/// Final status level options.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum FinalStatusLevelOpt {
    None,
    Fail,
    #[clap(alias = "retry")]
    Flaky,
    Slow,
    Skip,
    Pass,
    All,
}

impl From<FinalStatusLevelOpt> for FinalStatusLevel {
    fn from(opt: FinalStatusLevelOpt) -> Self {
        match opt {
            FinalStatusLevelOpt::None => FinalStatusLevel::None,
            FinalStatusLevelOpt::Fail => FinalStatusLevel::Fail,
            FinalStatusLevelOpt::Flaky => FinalStatusLevel::Flaky,
            FinalStatusLevelOpt::Slow => FinalStatusLevel::Slow,
            FinalStatusLevelOpt::Skip => FinalStatusLevel::Skip,
            FinalStatusLevelOpt::Pass => FinalStatusLevel::Pass,
            FinalStatusLevelOpt::All => FinalStatusLevel::All,
        }
    }
}

/// Show progress options.
#[derive(Default, Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ShowProgressOpt {
    /// Automatically choose the best progress display based on whether nextest
    /// is running in an interactive terminal.
    #[default]
    Auto,

    /// Do not display a progress bar or counter.
    None,

    /// Display a progress bar with running tests: default for interactive
    /// terminals.
    #[clap(alias = "running")]
    Bar,

    /// Display a counter next to each completed test.
    Counter,

    /// Display a progress bar with running tests, and hide successful test
    /// output; equivalent to `--show-progress=running --status-level=slow
    /// --final-status-level=none`.
    Only,
}

impl From<ShowProgressOpt> for UiShowProgress {
    fn from(opt: ShowProgressOpt) -> Self {
        match opt {
            ShowProgressOpt::Auto => UiShowProgress::Auto,
            ShowProgressOpt::None => UiShowProgress::None,
            ShowProgressOpt::Bar => UiShowProgress::Bar,
            ShowProgressOpt::Counter => UiShowProgress::Counter,
            ShowProgressOpt::Only => UiShowProgress::Only,
        }
    }
}

/// Ignore overrides options.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum IgnoreOverridesOpt {
    Retries,
    All,
}

/// Message format for run command (experimental).
#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub(crate) enum MessageFormat {
    /// The default output format.
    #[default]
    Human,
    /// Output test information in the same format as libtest.
    LibtestJson,
    /// Output test information in the same format as libtest, with a `nextest` subobject that
    /// includes additional metadata.
    LibtestJsonPlus,
}

/// Configuration for how to invoke Cargo and handle its output.
///
/// This type controls:
///
/// - What `--message-format` arguments to pass to Cargo.
/// - Whether to forward JSON output to stdout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CargoMessageFormat {
    /// Human-readable diagnostics rendered by Cargo.
    ///
    /// Cargo args: `json-render-diagnostics`
    ///
    /// Note: `--cargo-message-format short` also maps to this variant, since
    /// `cargo test --message-format short` produces the same output as human.
    #[default]
    Human,

    /// JSON output forwarded to stdout.
    ///
    /// Cargo args: combinations of `json`, `json-diagnostic-short`,
    /// `json-diagnostic-rendered-ansi`, and/or `json-render-diagnostics`
    Json {
        /// Whether to also render diagnostics to stderr via Cargo.
        ///
        /// When true, Cargo uses `json-render-diagnostics`, which:
        /// - Renders compiler messages to stderr
        /// - **Removes** `compiler-message` entries from the JSON stdout
        ///
        /// When false, `compiler-message` entries remain in the JSON output.
        render_diagnostics: bool,

        /// Whether to use short diagnostic format.
        ///
        /// Cargo arg: `json-diagnostic-short`
        short: bool,

        /// Whether to include ANSI color codes in the `rendered` field.
        ///
        /// Cargo arg: `json-diagnostic-rendered-ansi`
        ansi: bool,
    },
}

impl CargoMessageFormat {
    /// Returns the `--message-format` argument(s) to pass to Cargo.
    pub(crate) fn cargo_arg(&self) -> &'static str {
        match self {
            Self::Human => "json-render-diagnostics",
            Self::Json {
                render_diagnostics: false,
                short: false,
                ansi: false,
            } => "json",
            Self::Json {
                render_diagnostics: false,
                short: true,
                ansi: false,
            } => "json-diagnostic-short",
            Self::Json {
                render_diagnostics: false,
                short: false,
                ansi: true,
            } => "json-diagnostic-rendered-ansi",
            Self::Json {
                render_diagnostics: false,
                short: true,
                ansi: true,
            } => "json-diagnostic-short,json-diagnostic-rendered-ansi",
            Self::Json {
                render_diagnostics: true,
                short: false,
                ansi: false,
            } => "json-render-diagnostics",
            Self::Json {
                render_diagnostics: true,
                short: true,
                ansi: false,
            } => "json-render-diagnostics,json-diagnostic-short",
            Self::Json {
                render_diagnostics: true,
                short: false,
                ansi: true,
            } => "json-render-diagnostics,json-diagnostic-rendered-ansi",
            Self::Json {
                render_diagnostics: true,
                short: true,
                ansi: true,
            } => "json-render-diagnostics,json-diagnostic-short,json-diagnostic-rendered-ansi",
        }
    }

    /// Returns whether JSON should be forwarded to stdout.
    pub(crate) fn forward_json(&self) -> bool {
        matches!(self, Self::Json { .. })
    }
}

/// Cargo message format CLI options.
///
/// Controls the format of Cargo's build output and whether to forward JSON
/// messages to stdout.
///
/// JSON modifiers can be combined:
/// `json,json-diagnostic-short,json-diagnostic-rendered-ansi`
#[derive(Clone, Copy, Debug, ValueEnum, Default, PartialEq, Eq, Hash)]
pub(crate) enum CargoMessageFormatOpt {
    /// Render diagnostics in the default human-readable format.
    #[default]
    Human,

    /// Alias for `human`.
    Short,

    /// Emit JSON messages to stdout.
    Json,

    /// Ensure the `rendered` field of JSON messages contains the "short"
    /// rendering from rustc.
    JsonDiagnosticShort,

    /// Ensure the `rendered` field of JSON messages contains ANSI color codes.
    JsonDiagnosticRenderedAnsi,

    /// Output JSON messages with human-readable diagnostics.
    JsonRenderDiagnostics,
}

impl CargoMessageFormatOpt {
    /// Returns the string representation of this option.
    fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Short => "short",
            Self::Json => "json",
            Self::JsonDiagnosticShort => "json-diagnostic-short",
            Self::JsonDiagnosticRenderedAnsi => "json-diagnostic-rendered-ansi",
            Self::JsonRenderDiagnostics => "json-render-diagnostics",
        }
    }

    /// Combines CLI options into a single domain model.
    pub(crate) fn combine(opts: &[Self]) -> Result<CargoMessageFormat, CargoMessageFormatError> {
        let mut base_format: Option<Self> = None;
        let mut short = false;
        let mut ansi = false;
        let mut render_diagnostics = false;
        let mut seen = HashSet::new();

        for &opt in opts {
            if !seen.insert(opt) {
                return Err(CargoMessageFormatError::Duplicate {
                    option: opt.as_str(),
                });
            }

            match opt {
                Self::Human | Self::Short | Self::Json => {
                    if let Some(existing) = base_format {
                        return Err(CargoMessageFormatError::ConflictingBaseFormats {
                            first: existing.as_str(),
                            second: opt.as_str(),
                        });
                    }
                    base_format = Some(opt);
                }
                Self::JsonDiagnosticShort => short = true,
                Self::JsonDiagnosticRenderedAnsi => ansi = true,
                Self::JsonRenderDiagnostics => render_diagnostics = true,
            }
        }

        let has_json_modifiers = short || ansi || render_diagnostics;

        match base_format {
            None if has_json_modifiers => {
                // JSON modifiers without explicit base imply JSON.
                Ok(CargoMessageFormat::Json {
                    render_diagnostics,
                    short,
                    ansi,
                })
            }
            None => {
                // No options specified, use default.
                Ok(CargoMessageFormat::Human)
            }
            Some(fmt @ Self::Human) | Some(fmt @ Self::Short) => {
                if has_json_modifiers {
                    return Err(CargoMessageFormatError::JsonModifierWithNonJson {
                        modifiers: json_modifiers_str(short, ansi, render_diagnostics),
                        base: fmt.as_str(),
                    });
                }
                // Both human and short map to Human (short produces same output
                // as human for `cargo test`).
                Ok(CargoMessageFormat::Human)
            }
            Some(Self::Json) => Ok(CargoMessageFormat::Json {
                render_diagnostics,
                short,
                ansi,
            }),
            Some(_) => unreachable!(),
        }
    }
}

/// Returns a formatted string listing all JSON modifiers that are set.
fn json_modifiers_str(short: bool, ansi: bool, render_diagnostics: bool) -> &'static str {
    match (short, ansi, render_diagnostics) {
        (true, false, false) => "`json-diagnostic-short`",
        (false, true, false) => "`json-diagnostic-rendered-ansi`",
        (false, false, true) => "`json-render-diagnostics`",
        (true, true, false) => "`json-diagnostic-short` and `json-diagnostic-rendered-ansi`",
        (true, false, true) => "`json-diagnostic-short` and `json-render-diagnostics`",
        (false, true, true) => "`json-diagnostic-rendered-ansi` and `json-render-diagnostics`",
        (true, true, true) => {
            "`json-diagnostic-short`, `json-diagnostic-rendered-ansi`, \
             and `json-render-diagnostics`"
        }
        (false, false, false) => unreachable!("at least one modifier must be set"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CargoMessageFormatOpt::*;

    /// Helper to assert successful combination.
    fn assert_combine(opts: &[CargoMessageFormatOpt], expected: CargoMessageFormat) {
        let result = CargoMessageFormatOpt::combine(opts);
        assert_eq!(result, Ok(expected), "opts={opts:?}");
    }

    /// Helper to assert conflicting base formats error.
    fn assert_conflicting(
        opts: &[CargoMessageFormatOpt],
        first: &'static str,
        second: &'static str,
    ) {
        let result = CargoMessageFormatOpt::combine(opts);
        assert_eq!(
            result,
            Err(CargoMessageFormatError::ConflictingBaseFormats { first, second }),
            "opts={opts:?}"
        );
    }

    /// Helper to assert JSON modifier with non-JSON base error.
    fn assert_modifier_with_non_json(
        opts: &[CargoMessageFormatOpt],
        modifiers: &'static str,
        base: &'static str,
    ) {
        let result = CargoMessageFormatOpt::combine(opts);
        assert_eq!(
            result,
            Err(CargoMessageFormatError::JsonModifierWithNonJson { modifiers, base }),
            "opts={opts:?}"
        );
    }

    /// Helper to assert duplicate error.
    fn assert_duplicate(opts: &[CargoMessageFormatOpt], option: &'static str) {
        let result = CargoMessageFormatOpt::combine(opts);
        assert_eq!(
            result,
            Err(CargoMessageFormatError::Duplicate { option }),
            "opts={opts:?}"
        );
    }

    #[test]
    fn test_cargo_message_format_opt_combine() {
        // ---
        // Empty input
        // ---
        assert_combine(&[], CargoMessageFormat::Human);

        // ---
        // Single base formats
        // ---

        assert_combine(&[Human], CargoMessageFormat::Human);

        // Short maps to Human (cargo test --message-format short produces the
        // same output as human).
        assert_combine(&[Short], CargoMessageFormat::Human);

        assert_combine(
            &[Json],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: false,
            },
        );

        // ---
        // Single JSON modifiers (imply Json base)
        // ---

        assert_combine(
            &[JsonDiagnosticShort],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: false,
            },
        );

        assert_combine(
            &[JsonDiagnosticRenderedAnsi],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: true,
            },
        );

        assert_combine(
            &[JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: false,
            },
        );

        // ---
        // Json base + one modifier
        // ---

        assert_combine(
            &[Json, JsonDiagnosticShort],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: false,
            },
        );

        assert_combine(
            &[Json, JsonDiagnosticRenderedAnsi],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: true,
            },
        );

        assert_combine(
            &[Json, JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: false,
            },
        );

        // ---
        // Json base + two modifiers
        // ---

        assert_combine(
            &[Json, JsonDiagnosticShort, JsonDiagnosticRenderedAnsi],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: true,
            },
        );

        assert_combine(
            &[Json, JsonDiagnosticShort, JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: false,
            },
        );

        assert_combine(
            &[Json, JsonDiagnosticRenderedAnsi, JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: true,
            },
        );

        // ---
        // Json base + all three modifiers
        // ---

        assert_combine(
            &[
                Json,
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                JsonRenderDiagnostics,
            ],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: true,
            },
        );

        // ---
        // Two modifiers without explicit Json base
        // ---

        assert_combine(
            &[JsonDiagnosticShort, JsonDiagnosticRenderedAnsi],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: true,
            },
        );

        assert_combine(
            &[JsonDiagnosticShort, JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: false,
            },
        );

        assert_combine(
            &[JsonDiagnosticRenderedAnsi, JsonRenderDiagnostics],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: true,
            },
        );

        // ---
        // Three modifiers without explicit Json base
        // ---

        assert_combine(
            &[
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                JsonRenderDiagnostics,
            ],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: true,
            },
        );

        // ---
        // Order independence: same combinations in different orders
        // ---

        assert_combine(
            &[JsonDiagnosticRenderedAnsi, JsonDiagnosticShort],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: true,
            },
        );

        assert_combine(
            &[JsonDiagnosticShort, Json],
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: false,
            },
        );

        assert_combine(
            &[
                JsonRenderDiagnostics,
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                Json,
            ],
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: true,
            },
        );

        // ---
        // Error: conflicting base formats
        // ---

        assert_conflicting(&[Human, Json], "human", "json");
        assert_conflicting(&[Human, Short], "human", "short");
        assert_conflicting(&[Short, Json], "short", "json");
        assert_conflicting(&[Json, Human], "json", "human");
        assert_conflicting(&[Json, Short], "json", "short");
        assert_conflicting(&[Short, Human], "short", "human");

        // ---
        // Error: JSON modifier with Human base
        // ---

        assert_modifier_with_non_json(
            &[Human, JsonDiagnosticShort],
            "`json-diagnostic-short`",
            "human",
        );
        assert_modifier_with_non_json(
            &[Human, JsonDiagnosticRenderedAnsi],
            "`json-diagnostic-rendered-ansi`",
            "human",
        );
        assert_modifier_with_non_json(
            &[Human, JsonRenderDiagnostics],
            "`json-render-diagnostics`",
            "human",
        );
        assert_modifier_with_non_json(
            &[Human, JsonDiagnosticShort, JsonDiagnosticRenderedAnsi],
            "`json-diagnostic-short` and `json-diagnostic-rendered-ansi`",
            "human",
        );
        assert_modifier_with_non_json(
            &[Human, JsonDiagnosticShort, JsonRenderDiagnostics],
            "`json-diagnostic-short` and `json-render-diagnostics`",
            "human",
        );
        assert_modifier_with_non_json(
            &[Human, JsonDiagnosticRenderedAnsi, JsonRenderDiagnostics],
            "`json-diagnostic-rendered-ansi` and `json-render-diagnostics`",
            "human",
        );
        assert_modifier_with_non_json(
            &[
                Human,
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                JsonRenderDiagnostics,
            ],
            "`json-diagnostic-short`, `json-diagnostic-rendered-ansi`, \
             and `json-render-diagnostics`",
            "human",
        );

        // ---
        // Error: JSON modifier with Short base
        // ---

        assert_modifier_with_non_json(
            &[Short, JsonDiagnosticShort],
            "`json-diagnostic-short`",
            "short",
        );
        assert_modifier_with_non_json(
            &[Short, JsonDiagnosticRenderedAnsi],
            "`json-diagnostic-rendered-ansi`",
            "short",
        );
        assert_modifier_with_non_json(
            &[Short, JsonRenderDiagnostics],
            "`json-render-diagnostics`",
            "short",
        );
        assert_modifier_with_non_json(
            &[Short, JsonDiagnosticShort, JsonDiagnosticRenderedAnsi],
            "`json-diagnostic-short` and `json-diagnostic-rendered-ansi`",
            "short",
        );
        assert_modifier_with_non_json(
            &[Short, JsonDiagnosticShort, JsonRenderDiagnostics],
            "`json-diagnostic-short` and `json-render-diagnostics`",
            "short",
        );
        assert_modifier_with_non_json(
            &[Short, JsonDiagnosticRenderedAnsi, JsonRenderDiagnostics],
            "`json-diagnostic-rendered-ansi` and `json-render-diagnostics`",
            "short",
        );
        assert_modifier_with_non_json(
            &[
                Short,
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                JsonRenderDiagnostics,
            ],
            "`json-diagnostic-short`, `json-diagnostic-rendered-ansi`, \
             and `json-render-diagnostics`",
            "short",
        );

        // ---
        // Error: duplicate options
        // ---

        assert_duplicate(&[Human, Human], "human");
        assert_duplicate(&[Short, Short], "short");
        assert_duplicate(&[Json, Json], "json");
        assert_duplicate(
            &[JsonDiagnosticShort, JsonDiagnosticShort],
            "json-diagnostic-short",
        );
        assert_duplicate(
            &[JsonDiagnosticRenderedAnsi, JsonDiagnosticRenderedAnsi],
            "json-diagnostic-rendered-ansi",
        );
        assert_duplicate(
            &[JsonRenderDiagnostics, JsonRenderDiagnostics],
            "json-render-diagnostics",
        );

        // Duplicates with other options present.
        assert_duplicate(&[Json, JsonDiagnosticShort, Json], "json");
        assert_duplicate(
            &[
                JsonDiagnosticShort,
                JsonDiagnosticRenderedAnsi,
                JsonDiagnosticShort,
            ],
            "json-diagnostic-short",
        );
    }

    #[test]
    fn test_cargo_message_format_to_cargo_arg() {
        // Test the domain model's to_cargo_arg method.
        assert_eq!(
            CargoMessageFormat::Human.cargo_arg(),
            "json-render-diagnostics"
        );

        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: false
            }
            .cargo_arg(),
            "json"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: false
            }
            .cargo_arg(),
            "json-diagnostic-short"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: true
            }
            .cargo_arg(),
            "json-diagnostic-rendered-ansi"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: true,
                ansi: true
            }
            .cargo_arg(),
            "json-diagnostic-short,json-diagnostic-rendered-ansi"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: false
            }
            .cargo_arg(),
            "json-render-diagnostics"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: false
            }
            .cargo_arg(),
            "json-render-diagnostics,json-diagnostic-short"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: false,
                ansi: true
            }
            .cargo_arg(),
            "json-render-diagnostics,json-diagnostic-rendered-ansi"
        );
        assert_eq!(
            CargoMessageFormat::Json {
                render_diagnostics: true,
                short: true,
                ansi: true
            }
            .cargo_arg(),
            "json-render-diagnostics,json-diagnostic-short,json-diagnostic-rendered-ansi"
        );
    }

    #[test]
    fn test_cargo_message_format_predicates() {
        // Test forward_json.
        assert!(!CargoMessageFormat::Human.forward_json());
        assert!(
            CargoMessageFormat::Json {
                render_diagnostics: false,
                short: false,
                ansi: false
            }
            .forward_json()
        );
    }
}
