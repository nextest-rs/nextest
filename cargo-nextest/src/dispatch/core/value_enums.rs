// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Value enums shared across core commands.

use clap::ValueEnum;
use nextest_metadata::BuildPlatform;
use nextest_runner::{
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
    user_config::elements::UiShowProgress,
};

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
