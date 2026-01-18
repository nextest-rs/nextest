// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Clap error handling with pager support for help output.
//!
//! This module provides early argument extraction and clap error handling,
//! enabling paged help output.
//!
//! The early argument extraction uses clap's own parsing with `ignore_errors`
//! to properly handle the `--` separator.

use crate::output::Color;
use clap::{ArgAction, Args, Command};
use nextest_runner::{
    pager::PagedOutput,
    platform::Platform,
    user_config::{
        EarlyUserConfig, UserConfigLocation,
        elements::{PagerSetting, PaginateSetting, UiConfig},
    },
    write_str::WriteStr,
};
use std::io::{IsTerminal, Write};
use tracing::debug;

/// Early setup information needed before full CLI parsing.
///
/// This contains the build-time host platform and early arguments extracted
/// from the command line. This is used for help output and other early
/// decisions where we don't want to run `rustc -vV`.
#[derive(Debug)]
pub struct EarlySetup {
    /// The build-time host platform.
    ///
    /// This uses `Platform::build_target()` rather than runtime detection via
    /// `rustc -vV`. For help output and early config, this is sufficient and
    /// avoids potential failures from missing/broken rustc.
    build_target_platform: Platform,
    /// Early arguments extracted before full parsing.
    pub early_args: EarlyArgs,
}

impl EarlySetup {
    /// Performs early setup: gets build target platform and extracts early args.
    ///
    /// This should be called once at startup, before CLI parsing. To avoid
    /// complex error handling, this uses `Platform::build_target()`, which is
    /// compile-time detection, not runtime `rustc -vV` detection.
    pub fn new(cli_args: &[String], app: &Command) -> Self {
        // Use the build target platform for early setup. This is compile-time
        // detection and essentially infallible for supported platforms.
        let build_target_platform =
            Platform::build_target().expect("nextest is built for a supported platform");
        let early_args = extract_early_args(cli_args, app);
        Self {
            build_target_platform,
            early_args,
        }
    }
}

/// Early arguments extracted before full CLI parsing.
///
/// These are needed to handle clap errors (especially help output) before
/// the full argument parsing completes. Using clap's derive system with
/// `global = true` ensures proper handling of the `--` separator.
#[derive(Clone, Debug, Default, Args)]
pub struct EarlyArgs {
    /// Produce color output: auto, always, never.
    #[arg(
        long,
        value_enum,
        default_value_t,
        hide_possible_values = true,
        global = true,
        value_name = "WHEN",
        env = "CARGO_TERM_COLOR"
    )]
    pub color: Color,

    /// Do not pipe output through a pager.
    #[arg(long, global = true)]
    pub no_pager: bool,

    /// User config file [default: ~/.config/nextest/config.toml or platform equivalent].
    ///
    /// User configuration stores per-user preferences like UI settings. Use "none" to skip
    /// loading user config entirely.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        env = "NEXTEST_USER_CONFIG_FILE",
        help_heading = "Config options"
    )]
    pub user_config_file: Option<String>,
}

impl EarlyArgs {
    /// Returns the user config location.
    pub fn user_config_location(&self) -> UserConfigLocation<'_> {
        UserConfigLocation::from_cli_or_env(self.user_config_file.as_deref())
    }

    /// Returns the effective pager and paginate settings, given the resolved UI
    /// config.
    ///
    /// If `--no-pager` is specified, returns `PaginateSetting::Never`.
    /// Otherwise, falls back to the resolved config values.
    pub fn resolve_pager(&self, resolved_ui: &UiConfig) -> (PagerSetting, PaginateSetting) {
        if self.no_pager {
            // --no-pager disables paging entirely.
            return (resolved_ui.pager.clone(), PaginateSetting::Never);
        }

        // Fall back to resolved config.
        (resolved_ui.pager.clone(), resolved_ui.paginate)
    }
}

/// Extracts early arguments from CLI args using clap's parsing.
///
/// This approach uses clap's own argument parsing with `ignore_errors(true)`
/// to properly handle the `--` separator and other edge cases.
///
/// The technique is inspired by Jujutsu, which uses a similar approach for
/// early argument extraction.
fn extract_early_args(args: &[String], app: &Command) -> EarlyArgs {
    // Clone the command and configure it for early parsing:
    //
    // - disable_version_flag: Don't stop at --version
    // - disable_help_flag: Don't stop at -h/--help (we add a dummy instead)
    // - ignore_errors: Continue parsing even with errors
    //
    // We add a dummy help flag that counts occurrences instead of triggering
    // the DisplayHelp error. This allows early parsing to complete and extract
    // our global options.
    let early_cmd = app
        .clone()
        .disable_version_flag(true)
        .disable_help_flag(true)
        // Add a dummy help arg that doesn't stop parsing.
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .global(true)
                .action(ArgAction::Count),
        )
        .ignore_errors(true);

    // Try to parse. With ignore_errors, this should always succeed.
    match early_cmd.try_get_matches_from(args) {
        Ok(matches) => {
            // Extract early args manually from matches.
            // We use try_get_one to handle cases where the arg might not be
            // present (e.g., if parsing failed partway through).
            let no_pager = matches
                .try_get_one::<bool>("no_pager")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false);
            let color = matches
                .try_get_one::<Color>("color")
                .ok()
                .flatten()
                .copied()
                .unwrap_or_default();
            let user_config_file = matches
                .try_get_one::<String>("user_config_file")
                .ok()
                .flatten()
                .cloned();
            EarlyArgs {
                color,
                no_pager,
                user_config_file,
            }
        }
        Err(_) => {
            // This shouldn't happen with ignore_errors, but fall back to defaults.
            EarlyArgs::default()
        }
    }
}

/// Handles a clap error, potentially paging help output.
///
/// Returns the exit code to use.
pub fn handle_clap_error(err: clap::Error, early_setup: &EarlySetup) -> i32 {
    use clap::error::ErrorKind;

    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            handle_help_output(
                err,
                &early_setup.early_args,
                &early_setup.build_target_platform,
            );
            0
        }
        ErrorKind::DisplayVersion => {
            // Version output is short, no paging needed.
            let _ = err.print();
            0
        }
        _ => {
            // Other errors go to stderr.
            let _ = err.print();
            err.exit_code()
        }
    }
}

/// Handles help output, potentially through a pager.
fn handle_help_output(err: clap::Error, early_args: &EarlyArgs, host_platform: &Platform) {
    let should_colorize = early_args
        .color
        .should_colorize(supports_color::Stream::Stdout);

    let help_text = if should_colorize {
        err.render().ansi().to_string()
    } else {
        err.render().to_string()
    };

    let should_page = !early_args.no_pager && std::io::stdout().is_terminal();

    if !should_page {
        // No paging: write output directly. Do this as an early check before
        // loading the user config.
        let _ = std::io::stdout().write_all(help_text.as_bytes());
        return;
    }

    let early_config =
        EarlyUserConfig::for_platform(host_platform, early_args.user_config_location());
    if early_config.paginate == PaginateSetting::Never {
        let _ = std::io::stdout().write_all(help_text.as_bytes());
        return;
    }

    let mut paged = PagedOutput::request_pager(
        &early_config.pager,
        early_config.paginate,
        &early_config.streampager,
    );

    if let Err(error) = paged.write_str(&help_text) {
        // If writing to pager fails, try stdout directly.
        debug!("failed to write to pager: {error}, falling back to stdout");
        let _ = std::io::stdout().write_all(help_text.as_bytes());
        return;
    }

    if let Err(error) = paged.write_str_flush() {
        debug!("failed to flush pager: {error}");
        // Don't write to stdout if flushing fails, because the text probably
        // got written out.
    }

    paged.finalize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::app::CargoNextestApp;
    use clap::CommandFactory;

    fn cmd() -> Command {
        CargoNextestApp::command()
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(Into::into).collect()
    }

    #[test]
    fn test_extract_early_args() {
        // Remove this env var (which is set in CI) so that without arguments,
        // color is auto.
        unsafe {
            std::env::remove_var("CARGO_TERM_COLOR");
        }

        // Empty args.
        let early = extract_early_args(&[], &cmd());
        assert!(!early.no_pager);
        assert!(matches!(early.color, Color::Auto));

        // --no-pager before subcommand.
        let early = extract_early_args(&args("cargo nextest --no-pager run"), &cmd());
        assert!(early.no_pager);

        // --color=always (equals syntax).
        let early = extract_early_args(&args("cargo nextest --color=always"), &cmd());
        assert!(matches!(early.color, Color::Always));

        // --color=never (equals syntax).
        let early = extract_early_args(&args("cargo nextest --color=never"), &cmd());
        assert!(matches!(early.color, Color::Never));

        // --color always (space syntax).
        let early = extract_early_args(&args("cargo nextest --color always"), &cmd());
        assert!(matches!(early.color, Color::Always));

        // Combined --no-pager and --color.
        let early = extract_early_args(&args("cargo nextest --no-pager --color=never run"), &cmd());
        assert!(early.no_pager);
        assert!(matches!(early.color, Color::Never));

        // --no-pager after -- should be ignored.
        let early = extract_early_args(&args("cargo nextest run -- --no-pager"), &cmd());
        assert!(!early.no_pager, "--no-pager after -- should be ignored");

        // --no-pager before -- should work.
        let early = extract_early_args(&args("cargo nextest --no-pager run -- test_name"), &cmd());
        assert!(early.no_pager, "--no-pager before -- should work");

        // --color after -- should be ignored.
        let early = extract_early_args(&args("cargo nextest run -- --color=never"), &cmd());
        assert!(
            matches!(early.color, Color::Auto),
            "--color after -- should be ignored"
        );
    }
}
