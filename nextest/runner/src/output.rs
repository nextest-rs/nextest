// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use structopt::StructOpt;
use supports_color::Stream;

#[derive(Copy, Clone, Debug, StructOpt)]
#[must_use]
pub(crate) struct OutputOpts {
    // TODO: quiet/verbose?
    /// Produce color output
    #[structopt(
        long,
        global = true,
        default_value = "auto",
        possible_values = &["auto", "always", "never"],
    )]
    pub(crate) color: Color,
}

impl OutputOpts {
    pub(crate) fn init(self) -> OutputContext {
        let OutputOpts { color } = self;

        color.init_colored();

        OutputContext { color }
    }
}

#[derive(Copy, Clone, Debug)]
#[must_use]
pub(crate) struct OutputContext {
    pub(crate) color: Color,
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[must_use]
pub enum Color {
    Auto,
    Always,
    Never,
}

impl Color {
    fn init_colored(self) {
        match self {
            Color::Auto => owo_colors::unset_override(),
            Color::Always => owo_colors::set_override(true),
            Color::Never => owo_colors::set_override(false),
        }
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
