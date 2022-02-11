// Copyright (c) The nextest Contributors
// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use clap::{ArgEnum, Args};
use env_logger::fmt::Formatter;
use log::{Level, Record};
use owo_colors::{OwoColorize, Style};
use std::io::Write;
use supports_color::Stream;

#[derive(Copy, Clone, Debug, Args)]
#[must_use]
pub(crate) struct OutputOpts {
    /// Verbose output
    #[clap(long, short, global = true)]
    pub(crate) verbose: bool,
    // TODO: quiet?
    /// Produce color output: auto, always, never
    #[clap(
        long,
        arg_enum,
        default_value_t,
        hide_possible_values = true,
        global = true,
        value_name = "WHEN"
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

#[derive(Copy, Clone, Debug, PartialEq, ArgEnum)]
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

impl Color {
    fn init(self) {
        match self {
            Color::Auto => owo_colors::unset_override(),
            Color::Always => owo_colors::set_override(true),
            Color::Never => owo_colors::set_override(false),
        }

        env_logger::Builder::from_env("NEXTEST_LOG")
            .format(format_fn)
            .init();
    }

    pub(crate) fn should_colorize(self, stream: Stream) -> bool {
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
            "error".if_supports_color(Stream::Stderr, |s| s.style(Style::new().bold().red())),
            record.args()
        ),
        Level::Warn => writeln!(
            f,
            "{}: {}",
            "warning".if_supports_color(Stream::Stderr, |s| s.style(Style::new().bold().yellow())),
            record.args()
        ),
        Level::Info => writeln!(
            f,
            "{}: {}",
            "info".if_supports_color(Stream::Stderr, |s| s.bold()),
            record.args()
        ),
        Level::Debug => writeln!(
            f,
            "{}: {}",
            "debug".if_supports_color(Stream::Stderr, |s| s.bold()),
            record.args()
        ),
        _other => Ok(()),
    }
}
