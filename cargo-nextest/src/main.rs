// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use cargo_nextest::{CargoNextestApp, OutputWriter};
use clap::Parser;
use color_eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;
    let _ = enable_ansi_support::enable_ansi_support();

    let cli_args: Vec<_> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    let opts = CargoNextestApp::parse();
    let output = opts.init_output();

    match opts.exec(cli_args, output, &mut OutputWriter::default()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            error.display_to_stderr(&output.stderr_styles());
            std::process::exit(error.process_exit_code())
        }
    }
}
