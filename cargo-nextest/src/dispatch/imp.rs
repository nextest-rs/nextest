// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Main entry point implementation.

use super::{
    app::CargoNextestApp,
    clap_error::{EarlySetup, handle_clap_error},
};
use clap::{CommandFactory, Parser};

/// Main entry point for cargo-nextest.
///
/// This function handles CLI parsing, early setup (for paged help), and
/// dispatches to the appropriate command. Both the main binary and the
/// integration test duplicate use this.
pub fn main_impl() -> ! {
    let cli_args: Vec<_> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    let app = CargoNextestApp::command();
    let early_setup = EarlySetup::new(&cli_args, &app);

    match CargoNextestApp::try_parse() {
        Ok(opts) => {
            let output = opts.init_output();
            match opts.exec(cli_args, output, &mut crate::OutputWriter::default()) {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    error.display_to_stderr(&output.stderr_styles());
                    std::process::exit(error.process_exit_code())
                }
            }
        }
        Err(err) => {
            let code = handle_clap_error(err, &early_setup);
            std::process::exit(code);
        }
    }
}
