// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{ExpectedError, Result, output::OutputContext};
use camino::Utf8PathBuf;
use clap::Args;
use nextest_runner::double_spawn::double_spawn_child_init;
use std::os::unix::process::CommandExt;

#[derive(Debug, Args)]
pub(crate) struct DoubleSpawnOpts {
    /// The program to execute.
    program: Utf8PathBuf,

    /// The args to execute the program with, provided as a string.
    args: String,
}

impl DoubleSpawnOpts {
    // output is passed in to ensure that the context is initialized.
    pub(crate) fn exec(self, _output: OutputContext) -> Result<i32> {
        double_spawn_child_init();
        let args = shell_words::split(&self.args).map_err(|err| {
            ExpectedError::DoubleSpawnParseArgsError {
                args: self.args,
                err,
            }
        })?;
        let mut command = std::process::Command::new(&self.program);
        // Note: exec only returns an error -- in the success case it never returns.
        let err = command.args(args).exec();
        Err(ExpectedError::DoubleSpawnExecError {
            command: Box::new(command),
            current_dir: std::env::current_dir(),
            err,
        })
    }
}
