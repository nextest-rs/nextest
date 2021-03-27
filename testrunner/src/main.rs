// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use structopt::StructOpt;
use testrunner::dispatch::Opts;

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();
    opts.exec()
}
