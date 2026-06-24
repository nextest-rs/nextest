// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for `cargo nextest help <command-path | topic>`.
//!
//! This forwards a command path to `cargo nextest <command> --help`, and renders
//! a reference doc for a help topic such as `filterset`.

use super::{EarlyArgs, app::CargoNextestApp, clap_error::handle_clap_error};
use crate::{
    Result,
    errors::ExpectedError,
    output::{OutputContext, terminal_width},
};
use clap::{CommandFactory, builder::styling::Style, error::ErrorKind};
use nextest_filtering::FILTERSET_REFERENCE_MD;
use nextest_runner::{
    help_render::{self, HelpDoc, RenderOptions},
    pager::PagedOutput,
    user_config::{EarlyUserConfig, elements::PaginateSetting},
    write_str::WriteStr,
};
use std::io::Write as _;

/// A custom help topic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HelpTopic {
    Filterset,
}

impl HelpTopic {
    const ALL: &[Self] = &[Self::Filterset];

    /// Returns all names that match a topic.
    ///
    /// The first is the canonical name, and the rest are aliases.
    fn names(self) -> &'static [&'static str] {
        match self {
            Self::Filterset => &["filterset", "filtersets"],
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|topic| topic.names().contains(&name))
    }

    fn name(self) -> &'static str {
        self.names()[0]
    }

    fn max_name_width() -> usize {
        Self::ALL
            .iter()
            .map(|topic| topic.name().len())
            .max()
            .unwrap_or(0)
    }

    fn description(self) -> &'static str {
        match self {
            Self::Filterset => "the filterset DSL: predicates, operators, and matchers",
        }
    }

    fn render(self, opts: RenderOptions) -> String {
        help_render::render(&self.doc(), opts)
    }

    /// Returns the [`HelpDoc`] for this topic.
    pub(super) fn doc(self) -> HelpDoc {
        match self {
            Self::Filterset => HelpDoc {
                markdown: FILTERSET_REFERENCE_MD,
                site_dir: &["docs", "filtersets"],
            },
        }
    }
}

/// Renders help output for the given command path, or a reference doc for a help topic.
pub(crate) fn exec_help(
    command_path: Vec<String>,
    early_args: &EarlyArgs,
    output: OutputContext,
) -> Result<i32> {
    // Try using the first segment as a help topic. (If it matches, drop the
    // remaining segments -- doing this is better than producing a weird error
    // if an extra argument is provided.)
    if let [name, ..] = command_path.as_slice()
        && let Some(topic) = HelpTopic::from_name(name)
    {
        return render_topic(&topic, early_args, output);
    }

    delegate_command_help(&command_path, early_args)
}

fn render_topic(topic: &HelpTopic, early_args: &EarlyArgs, output: OutputContext) -> Result<i32> {
    let early_config = EarlyUserConfig::load(early_args.user_config_location());
    let paginate = if early_args.no_pager {
        PaginateSetting::Never
    } else {
        early_config.paginate
    };

    let color = output.color.should_colorize(supports_color::Stream::Stdout);
    let mut paged =
        PagedOutput::request_pager(&early_config.pager, paginate, &early_config.streampager);

    // TODO: it would be nice to support hyperlinks through pagers in the
    // future. less started supporting them around version 566, but many systems
    // are still on older versions.
    let hyperlinks =
        !paged.is_paged() && supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout);

    let rendered = topic.render(RenderOptions {
        color,
        hyperlinks,
        width: terminal_width(),
    });

    paged
        .write_str(&rendered)
        .map_err(|err| ExpectedError::WriteHelpError { err })?;
    paged
        .write_str_flush()
        .map_err(|err| ExpectedError::WriteHelpError { err })?;
    paged.finalize();
    Ok(0)
}

/// Forwards `[..path, --help]` to clap.
fn delegate_command_help(path: &[String], early_args: &EarlyArgs) -> Result<i32> {
    let mut argv = vec!["cargo".to_string(), "nextest".to_string()];
    argv.extend(path.iter().cloned());
    argv.push("--help".to_string());

    let err = match CargoNextestApp::command().try_get_matches_from(argv) {
        Ok(_) => return Ok(0),
        Err(err) => err,
    };

    // Only an unknown first segment (e.g. `cargo nextest help foo`) gets a
    // topic hint.
    let top_level_miss = !is_help_or_version(&err)
        && path
            .first()
            .is_some_and(|first| !is_nextest_subcommand(first));
    if !top_level_miss {
        return Ok(handle_clap_error(err, early_args));
    }

    // Append the topic hint to stderr.
    let colorize = early_args
        .color
        .should_colorize(supports_color::Stream::Stderr);
    let mut rendered = if colorize {
        err.render().ansi().to_string()
    } else {
        err.render().to_string()
    };
    append_topic_hint(&mut rendered, colorize);
    let _ = write!(std::io::stderr(), "{rendered}");

    Ok(err.exit_code())
}

fn is_help_or_version(err: &clap::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            | ErrorKind::DisplayVersion
    )
}

fn is_nextest_subcommand(name: &str) -> bool {
    CargoNextestApp::command()
        .find_subcommand("nextest")
        .is_some_and(|nextest| nextest.find_subcommand(name).is_some())
}

/// Returns a styled string with the "Help topics" section appended to the root
/// command's help.
///
/// This is written to be styled like clap's own sections.
pub(crate) fn topics_after_help() -> clap::builder::StyledStr {
    let styles = crate::output::clap_styles::style();
    let mut out = clap::builder::StyledStr::new();
    write_topics_section(&mut out, *styles.get_header(), *styles.get_literal());
    out
}

/// Appends the "Help topics" section to error output.
///
/// Unlike `topics_after_help`, this normalizes newlines and emits final
/// (colorized or not) text, since it's appended to an already-rendered clap
/// error and printed directly.
fn append_topic_hint(out: &mut String, colorize: bool) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    let styles = crate::output::clap_styles::style();
    let (header, literal) = if colorize {
        (*styles.get_header(), *styles.get_literal())
    } else {
        (Style::new(), Style::new())
    };
    write_topics_section(out, header, literal);
    out.push('\n');
}

/// Writes the "Help topics" section.
fn write_topics_section(out: &mut impl std::fmt::Write, header: Style, literal: Style) {
    let width = HelpTopic::max_name_width();
    let _ = write!(
        out,
        "{header}Help topics{header:#} (try {literal}cargo nextest help <topic>{literal:#}):"
    );
    for topic in HelpTopic::ALL {
        let pad = " ".repeat(width - topic.name().len() + 2);
        let _ = write!(
            out,
            "\n  {literal}{}{literal:#}{pad}{}",
            topic.name(),
            topic.description(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_names_resolve() {
        assert!(HelpTopic::from_name("filterset").is_some());
        assert!(HelpTopic::from_name("filtersets").is_some());
        assert!(HelpTopic::from_name("run").is_none());
        assert!(HelpTopic::from_name("").is_none());
    }

    #[test]
    fn subcommand_detection() {
        assert!(is_nextest_subcommand("run"));
        assert!(is_nextest_subcommand("self"));
        assert!(is_nextest_subcommand("help"));
        assert!(!is_nextest_subcommand("bogus"));
        // Topics are not subcommands.
        assert!(!is_nextest_subcommand("filterset"));
    }
}
