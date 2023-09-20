// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use clap::{Args, ValueEnum};
use env_logger::fmt::Formatter;
use log::{Level, LevelFilter, Record};
use miette::{GraphicalTheme, MietteHandlerOpts, ThemeStyles};
use nextest_runner::reporter::ReporterStderr;
use owo_colors::{style, OwoColorize, Style};
use std::{
    fmt,
    io::{BufWriter, Stderr, Stdout, Write},
    marker::PhantomData,
};

#[derive(Copy, Clone, Debug, Args)]
#[must_use]
pub(crate) struct OutputOpts {
    /// Verbose output
    #[arg(long, short, global = true, env = "NEXTEST_VERBOSE")]
    pub(crate) verbose: bool,
    // TODO: quiet?
    /// Produce color output: auto, always, never
    #[arg(
        long,
        value_enum,
        default_value_t,
        hide_possible_values = true,
        global = true,
        value_name = "WHEN",
        env = "CARGO_TERM_COLOR"
    )]
    pub(crate) color: Color,
}

impl OutputOpts {
    pub(crate) fn init(self) -> OutputContext {
        let OutputOpts { verbose, color } = self;

        color.init();

        OutputContext { verbose, color }
    }
}

#[derive(Copy, Clone, Debug)]
#[must_use]
pub(crate) struct OutputContext {
    pub(crate) verbose: bool,
    pub(crate) color: Color,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[must_use]
#[derive(Default)]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

impl Color {
    pub(crate) fn init(self) {
        match self {
            Color::Auto => {
                owo_colors::unset_override();
                overrides::unset_override();
            }
            Color::Always => {
                owo_colors::set_override(true);
                overrides::set_override(true);
            }
            Color::Never => {
                owo_colors::set_override(false);
                overrides::set_override(false);
            }
        }

        INIT_LOGGER.call_once(|| {
            env_logger::Builder::new()
                .filter_level(LevelFilter::Info)
                .parse_env("NEXTEST_LOG")
                .format(format_fn)
                .init();

            miette::set_hook(Box::new(move |_| {
                let theme_styles = if self.should_colorize(supports_color::Stream::Stderr) {
                    ThemeStyles {
                        error: style().red().bold(),
                        warning: style().yellow().bold(),
                        advice: style().bright_cyan().bold(),
                        help: style().cyan(),
                        link: style().cyan().underline().bold(),
                        linum: style().dimmed(),
                        highlights: vec![style().red(), style().yellow(), style().bright_cyan()],
                    }
                } else {
                    ThemeStyles::none()
                };
                let mut graphical_theme = if supports_unicode::on(supports_unicode::Stream::Stderr)
                {
                    GraphicalTheme::unicode()
                } else {
                    GraphicalTheme::ascii()
                };
                graphical_theme.characters.error = "error:".into();
                graphical_theme.styles = theme_styles;

                let handler = MietteHandlerOpts::new().graphical_theme(graphical_theme);
                Box::new(handler.build())
            }))
            .expect("miette::set_hook should only be called once");
        });
    }

    pub(crate) fn should_colorize(self, stream: supports_color::Stream) -> bool {
        match self {
            Color::Auto => supports_color::on_cached(stream).is_some(),
            Color::Always => true,
            Color::Never => false,
        }
    }

    pub(crate) fn to_arg(self) -> &'static str {
        match self {
            Color::Auto => "--color=auto",
            Color::Always => "--color=always",
            Color::Never => "--color=never",
        }
    }
}

fn format_fn(f: &mut Formatter, record: &Record<'_>) -> std::io::Result<()> {
    if record.target() == "cargo_nextest::no_heading" {
        writeln!(f, "{}", record.args())?;
        return Ok(());
    }

    match record.level() {
        Level::Error => writeln!(
            f,
            "{}: {}",
            "error".if_supports_color_2(supports_color::Stream::Stderr, |s| s
                .style(Style::new().red().bold())),
            record.args()
        ),
        Level::Warn => writeln!(
            f,
            "{}: {}",
            "warning".if_supports_color_2(supports_color::Stream::Stderr, |s| s
                .style(Style::new().yellow().bold())),
            record.args()
        ),
        Level::Info => writeln!(
            f,
            "{}: {}",
            "info".if_supports_color_2(supports_color::Stream::Stderr, |s| s.bold()),
            record.args()
        ),
        Level::Debug => writeln!(
            f,
            "{}: {}",
            "debug".if_supports_color_2(supports_color::Stream::Stderr, |s| s.bold()),
            record.args()
        ),
        _other => Ok(()),
    }
}

/// A helper for capturing output in tests
///
/// The test pass is gated by `#[cfg(test)]` to allow a better
/// optimization in the binary.
pub enum OutputWriter {
    /// No capture
    Normal,
    /// Output captured
    #[cfg(test)]
    Test {
        /// stdout capture
        stdout: Vec<u8>,
        /// stderr capture
        stderr: Vec<u8>,
    },
}

impl Default for OutputWriter {
    fn default() -> Self {
        Self::Normal
    }
}

impl OutputWriter {
    pub(crate) fn stdout_writer(&mut self) -> StdoutWriter<'_> {
        match self {
            Self::Normal => StdoutWriter::Normal {
                buf: BufWriter::new(std::io::stdout()),
                _lifetime: PhantomData,
            },
            #[cfg(test)]
            Self::Test { ref mut stdout, .. } => StdoutWriter::Test {
                buf: BufWriter::new(stdout),
            },
        }
    }

    pub(crate) fn reporter_output(&mut self) -> ReporterStderr<'_> {
        match self {
            Self::Normal => ReporterStderr::Terminal,
            #[cfg(test)]
            Self::Test { ref mut stderr, .. } => ReporterStderr::Buffer(stderr),
        }
    }

    pub(crate) fn stderr_writer(&mut self) -> StderrWriter<'_> {
        match self {
            Self::Normal => StderrWriter::Normal {
                buf: BufWriter::new(std::io::stderr()),
                _lifetime: PhantomData,
            },
            #[cfg(test)]
            Self::Test { ref mut stderr, .. } => StderrWriter::Test {
                buf: BufWriter::new(stderr),
            },
        }
    }
}

pub(crate) enum StdoutWriter<'a> {
    Normal {
        buf: BufWriter<Stdout>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(test)]
    Test { buf: BufWriter<&'a mut Vec<u8>> },
}

impl<'a> Write for StdoutWriter<'a> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Normal { buf, .. } => buf.write(data),
            #[cfg(test)]
            Self::Test { buf } => buf.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
            #[cfg(test)]
            Self::Test { .. } => Ok(()),
        }
    }
}

pub(crate) enum StderrWriter<'a> {
    Normal {
        buf: BufWriter<Stderr>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(test)]
    Test { buf: BufWriter<&'a mut Vec<u8>> },
}

impl<'a> Write for StderrWriter<'a> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Normal { buf, .. } => buf.write(data),
            #[cfg(test)]
            Self::Test { buf } => buf.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.flush(),
            #[cfg(test)]
            Self::Test { .. } => Ok(()),
        }
    }
}

/// Override support. Used by SupportsColorsV2Display.
mod overrides {
    use std::sync::atomic::{AtomicU8, Ordering};

    pub(crate) fn set_override(enabled: bool) {
        OVERRIDE.set_force(enabled)
    }

    pub(crate) fn unset_override() {
        OVERRIDE.unset()
    }

    pub(crate) static OVERRIDE: Override = Override::none();

    pub(crate) struct Override(AtomicU8);

    const FORCE_MASK: u8 = 0b10;
    const FORCE_ENABLE: u8 = 0b11;
    const FORCE_DISABLE: u8 = 0b10;
    const NO_FORCE: u8 = 0b00;

    impl Override {
        const fn none() -> Self {
            Self(AtomicU8::new(NO_FORCE))
        }

        fn inner(&self) -> u8 {
            self.0.load(Ordering::SeqCst)
        }

        pub(crate) fn is_force_enabled_or_disabled(&self) -> (bool, bool) {
            let inner = self.inner();

            (inner == FORCE_ENABLE, inner == FORCE_DISABLE)
        }

        fn set_force(&self, enable: bool) {
            self.0.store(FORCE_MASK | (enable as u8), Ordering::SeqCst)
        }

        fn unset(&self) {
            self.0.store(0, Ordering::SeqCst);
        }
    }
}

/// An extension trait for applying supports-color v2 to owo-colors.
///
/// supports-color v2 has some fixes that nextest needs.
pub(crate) trait SupportsColorsV2: OwoColorize {
    fn if_supports_color_2<'a, Out, ApplyFn>(
        &'a self,
        stream: supports_color::Stream,
        apply: ApplyFn,
    ) -> SupportsColorsV2Display<'a, Self, Out, ApplyFn>
    where
        ApplyFn: Fn(&'a Self) -> Out,
    {
        SupportsColorsV2Display(self, apply, stream)
    }
}

impl<T: OwoColorize> SupportsColorsV2 for T {}

/// A display wrapper which applies a transformation based on if the given stream supports
/// colored terminal output
pub struct SupportsColorsV2Display<'a, InVal, Out, ApplyFn>(
    pub(crate) &'a InVal,
    pub(crate) ApplyFn,
    pub(crate) supports_color::Stream,
)
where
    InVal: ?Sized,
    ApplyFn: Fn(&'a InVal) -> Out;

macro_rules! impl_fmt_for {
    ($($trait:path),* $(,)?) => {
        $(
            impl<'a, In, Out, F> $trait for SupportsColorsV2Display<'a, In, Out, F>
                where In: $trait,
                      Out: $trait,
                      F: Fn(&'a In) -> Out,
            {
                #[inline(always)]
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    // OVERRIDE is currently not supported
                    let (force_enabled, force_disabled) = overrides::OVERRIDE.is_force_enabled_or_disabled();
                    if force_enabled || (
                        supports_color::on_cached(self.2)
                            .map(|level| level.has_basic)
                            .unwrap_or(false)
                        && !force_disabled
                    ) {
                        <Out as $trait>::fmt(&self.1(self.0), f)
                    } else {
                        <In as $trait>::fmt(self.0, f)
                    }
                }
            }
        )*
    };
}

impl_fmt_for! {
    fmt::Display,
    fmt::Debug,
    fmt::UpperHex,
    fmt::LowerHex,
    fmt::Binary,
    fmt::UpperExp,
    fmt::LowerExp,
    fmt::Octal,
    fmt::Pointer,
}
