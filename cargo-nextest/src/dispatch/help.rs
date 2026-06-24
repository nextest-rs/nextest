// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for `cargo nextest self <command-path>`.
//!
//! For now, this just forwards to `cargo nextest <command> --help`, but in the
//! future this will also cover custom help topics.

use super::{EarlyArgs, app::CargoNextestApp, clap_error::handle_clap_error};
use crate::{Result, output::OutputContext};
use clap::CommandFactory;

/// Renders help output for the given command path..
pub(crate) fn exec_help(
    command_path: Vec<String>,
    early_args: &EarlyArgs,
    _output: OutputContext,
) -> Result<i32> {
    delegate_command_help(&command_path, early_args)
}

/// Forwards `[..path, --help]` to clap.
fn delegate_command_help(path: &[String], early_args: &EarlyArgs) -> Result<i32> {
    let mut argv = vec!["cargo".to_string(), "nextest".to_string()];
    argv.extend(path.iter().cloned());
    argv.push("--help".to_string());

    match CargoNextestApp::command().try_get_matches_from(argv) {
        Ok(_) => Ok(0),
        Err(err) => Ok(handle_clap_error(err, early_args)),
    }
}
