// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{error, fmt};

/// An error that occurs while running a `cargo nextest` command.
#[derive(Debug)]
pub enum CommandError {
    /// Executing the process resulted in an error.
    Exec(std::io::Error),

    /// The command exited with a non-zero code.
    CommandFailed {
        /// The exit code for the process. Exit codes can be cross-referenced against
        /// [`NextestExitCode`](crate::NextestExitCode).
        exit_code: Option<i32>,

        /// Standard error for the process.
        stderr: Vec<u8>,
    },

    /// Error parsing JSON output.
    Json(serde_json::Error),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Exec(_) => {
                write!(f, "`cargo nextest` process execution failed")
            }
            Self::CommandFailed { exit_code, stderr } => {
                let exit_code_str =
                    exit_code.map_or(String::new(), |code| format!(" with exit code {code}"));
                let stderr = String::from_utf8_lossy(stderr);
                write!(
                    f,
                    "`cargo nextest` failed{exit_code_str}, stderr:\n{stderr}\n"
                )
            }
            Self::Json(_) => {
                write!(f, "parsing `cargo nextest` JSON output failed")
            }
        }
    }
}

impl error::Error for CommandError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Exec(err) => Some(err),
            Self::CommandFailed { .. } => None,
            Self::Json(err) => Some(err),
        }
    }
}
