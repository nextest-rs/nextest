// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use clap::{Args, ValueEnum};
use miette::{GraphicalTheme, MietteHandlerOpts, ThemeStyles};
use nextest_runner::{output::{Color, OutputContext}, reporter::ReporterStderr, write_str::WriteStr};
use owo_colors::{OwoColorize, Style, style};
use std::{
    fmt,
    io::{self, BufWriter, Stderr, Stdout, Write},
    marker::PhantomData,
};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    level_filters::LevelFilter,
};
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt::{FmtContext, FormatEvent, FormatFields, format},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

pub(crate) mod clap_styles {
    use clap::builder::{
        Styles,
        styling::{AnsiColor, Effects, Style},
    };

    const HEADER: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
    const USAGE: Style = AnsiColor::Green.on_default().effects(Effects::BOLD);
    const LITERAL: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
    const PLACEHOLDER: Style = AnsiColor::Cyan.on_default();
    const ERROR: Style = AnsiColor::Red.on_default().effects(Effects::BOLD);
    const VALID: Style = AnsiColor::Cyan.on_default().effects(Effects::BOLD);
    const INVALID: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);

    pub(crate) const fn style() -> Styles {
        // Copied from
        // https://github.com/rust-lang/cargo/blob/98f6bf3700e2918678acd87b7e1a1450df579853/src/bin/cargo/cli.rs#L552-L561
        // to match Cargo's style.
        Styles::styled()
            .header(HEADER)
            .usage(USAGE)
            .literal(LITERAL)
            .placeholder(PLACEHOLDER)
            .error(ERROR)
            .valid(VALID)
            .invalid(INVALID)
    }
}

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
            Self::Test { stdout, .. } => StdoutWriter::Test { buf: stdout },
        }
    }

    pub(crate) fn reporter_output(&mut self) -> ReporterStderr<'_> {
        match self {
            Self::Normal => ReporterStderr::Terminal,
            #[cfg(test)]
            Self::Test { stderr, .. } => ReporterStderr::Buffer(stderr),
        }
    }

    pub(crate) fn stderr_writer(&mut self) -> StderrWriter<'_> {
        match self {
            Self::Normal => StderrWriter::Normal {
                buf: BufWriter::new(std::io::stderr()),
                _lifetime: PhantomData,
            },
            #[cfg(test)]
            Self::Test { stderr, .. } => StderrWriter::Test { buf: stderr },
        }
    }
}

pub(crate) enum StdoutWriter<'a> {
    Normal {
        buf: BufWriter<Stdout>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(test)]
    Test { buf: &'a mut Vec<u8> },
}

impl Write for StdoutWriter<'_> {
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

impl WriteStr for StdoutWriter<'_> {
    fn write_str(&mut self, s: &str) -> io::Result<()> {
        match self {
            Self::Normal { buf, .. } => buf.write_all(s.as_bytes()),
            #[cfg(test)]
            Self::Test { buf } => buf.write_all(s.as_bytes()),
        }
    }

    fn write_str_flush(&mut self) -> io::Result<()> {
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
    Test { buf: &'a mut Vec<u8> },
}

impl Write for StderrWriter<'_> {
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

pub(crate) fn should_redact() -> bool {
    std::env::var("__NEXTEST_REDACT") == Ok("1".to_string())
}
