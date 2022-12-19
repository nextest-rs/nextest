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
pub enum Color {
    Auto,
    Always,
    Never,
}

impl Default for Color {
    fn default() -> Self {
        Color::Auto
    }
}

static INIT_LOGGER: std::sync::Once = std::sync::Once::new();

impl Color {
    pub(crate) fn init(self) {
        match self {
            Color::Auto => owo_colors::unset_override(),
            Color::Always => owo_colors::set_override(true),
            Color::Never => owo_colors::set_override(false),
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
                        error: style().bright_red().bold(),
                        warning: style().bright_yellow().bold(),
                        advice: style().bright_cyan().bold(),
                        help: style().cyan(),
                        link: style().cyan().underline().bold(),
                        linum: style().dimmed(),
                        highlights: vec![
                            style().bright_red(),
                            style().bright_yellow(),
                            style().bright_cyan(),
                        ],
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

impl std::str::FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Color::Auto),
            "always" => Ok(Color::Always),
            "never" => Ok(Color::Never),
            s => Err(format!(
                "{} is not a valid option, expected `auto`, `always` or `never`",
                s
            )),
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
            "error".if_supports_color(owo_colors::Stream::Stderr, |s| s
                .style(Style::new().bright_red().bold())),
            record.args()
        ),
        Level::Warn => writeln!(
            f,
            "{}: {}",
            "warning".if_supports_color(owo_colors::Stream::Stderr, |s| s
                .style(Style::new().bright_yellow().bold())),
            record.args()
        ),
        Level::Info => writeln!(
            f,
            "{}: {}",
            "info".if_supports_color(owo_colors::Stream::Stderr, |s| s.bold()),
            record.args()
        ),
        Level::Debug => writeln!(
            f,
            "{}: {}",
            "debug".if_supports_color(owo_colors::Stream::Stderr, |s| s.bold()),
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
                _lifetime: PhantomData::default(),
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
                _lifetime: PhantomData::default(),
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
