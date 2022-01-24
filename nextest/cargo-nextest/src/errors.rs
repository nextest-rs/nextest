// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use nextest_summaries::NextestExitCodes;
use owo_colors::{OwoColorize, Stream};
use std::{error, fmt};

/// An error occurred in a program that nextest ran, not in nextest itself.
#[derive(Clone, Debug)]
#[doc(hidden)]
pub enum ExpectedError {
    CargoMetadataFailed,
    BuildFailed {
        escaped_command: Vec<String>,
        exit_code: Option<i32>,
    },
    TestRunFailed,
}

impl ExpectedError {
    pub(crate) fn cargo_metadata_failed() -> Self {
        Self::CargoMetadataFailed
    }

    pub(crate) fn build_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
        exit_code: Option<i32>,
    ) -> Self {
        Self::BuildFailed {
            escaped_command: command
                .into_iter()
                .map(|arg| shellwords::escape(arg.as_ref()))
                .collect(),
            exit_code,
        }
    }

    pub(crate) fn test_run_failed() -> Self {
        Self::TestRunFailed
    }

    /// Returns the exit code for the process.
    pub fn process_exit_code(&self) -> i32 {
        match self {
            Self::CargoMetadataFailed => NextestExitCodes::CARGO_METADATA_FAILED,
            Self::BuildFailed { .. } => NextestExitCodes::BUILD_FAILED,
            Self::TestRunFailed => NextestExitCodes::TEST_RUN_FAILED,
        }
    }

    pub fn display_to_stderr(&self) {
        match &self {
            Self::CargoMetadataFailed => {
                // The error produced by `cargo metadata` is enough.
            }
            Self::BuildFailed {
                escaped_command,
                exit_code,
            } => {
                let with_code_str = match exit_code {
                    Some(code) => {
                        format!(
                            " with code {}",
                            code.if_supports_color(Stream::Stderr, |x| x.bold())
                        )
                    }
                    None => "".to_owned(),
                };

                log::error!(
                    "command {} exited{}",
                    escaped_command
                        .join(" ")
                        .if_supports_color(Stream::Stderr, |x| x.bold()),
                    with_code_str,
                );
            }
            Self::TestRunFailed => {
                log::error!("test run failed");
            }
        }
    }
}

impl fmt::Display for ExpectedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This should generally not be called, but provide a stub implementation it is
        match self {
            Self::CargoMetadataFailed => writeln!(f, "cargo metadata failed"),
            Self::BuildFailed { .. } => writeln!(f, "build failed"),
            Self::TestRunFailed => writeln!(f, "test run failed"),
        }
    }
}

impl error::Error for ExpectedError {}
