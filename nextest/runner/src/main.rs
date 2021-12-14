// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use color_eyre::Result;
use nextest_runner::dispatch::Opts;
use structopt::StructOpt;

fn main() -> Result<()> {
    color_eyre::install()?;
    let _ = enable_ansi_support::enable_ansi_support();

    let opts = Opts::from_args();
    opts.exec()
}
