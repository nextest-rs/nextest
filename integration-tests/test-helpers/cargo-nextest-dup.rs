// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! This is a duplicate of cargo-nextest's main.rs to avoid issues on Windows.
//! See tests/integration/main.rs for more.

use color_eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;
    let _ = enable_ansi_support::enable_ansi_support();

    cargo_nextest::main_impl()
}
