// Copyright (c) The diem-x Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::io;
use structopt::StructOpt;
use testrunner::dispatch::Opts;

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();

    let stdout = io::stdout();
    let stdout_lock = stdout.lock();
    opts.exec(stdout_lock)
}
