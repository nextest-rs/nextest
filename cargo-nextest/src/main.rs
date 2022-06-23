// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use cargo_nextest::{CargoNextestApp, OutputWriter};
use clap::Parser;
use color_eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;
    let _ = enable_ansi_support::enable_ansi_support();

    let opts = CargoNextestApp::parse();
    match opts.exec(&mut OutputWriter::default()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            error.display_to_stderr();
            std::process::exit(error.process_exit_code())
        }
    }
}
