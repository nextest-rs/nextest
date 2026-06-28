// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{ExpectedError, Result, output::OutputContext};
use camino::Utf8PathBuf;
use clap::Args;
use nextest_runner::double_spawn::{double_spawn_child_init, double_spawn_child_set_priority};
use std::os::unix::process::CommandExt;

#[derive(Debug, Args)]
pub(crate) struct DoubleSpawnOpts {
    /// The nice value to apply.
    #[arg(long)]
    nice: Option<i32>,

    /// The program to execute.
    program: Utf8PathBuf,

    /// The args to execute the program with, provided as a string.
    args: String,
}

impl DoubleSpawnOpts {
    // output is passed in to ensure that the context is initialized.
    pub(crate) fn exec(self, _output: OutputContext) -> Result<i32> {
        double_spawn_child_init();
        if let Some(nice) = self.nice {
            double_spawn_child_set_priority(nice);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestApp {
        #[command(flatten)]
        opts: DoubleSpawnOpts,
    }

    #[test]
    fn nice_accepts_negative_values() {
        let app =
            TestApp::try_parse_from(["cargo-nextest", "--nice=-10", "test-program", "arg1 arg2"])
                .expect("--nice with a negative value parses");
        assert_eq!(app.opts.nice, Some(-10));
        assert_eq!(app.opts.program, "test-program");
        assert_eq!(app.opts.args, "arg1 arg2");
    }

    #[test]
    fn nice_is_optional() {
        let app = TestApp::try_parse_from(["cargo-nextest", "test-program", "arg1 arg2"])
            .expect("the program parses without --nice");
        assert_eq!(app.opts.nice, None);
    }
}
