// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use nextest_metadata::NextestExitCode;
use nextest_runner::errors::{ConfigParseError, ProfileNotFound};
use owo_colors::{OwoColorize, Stream};
use std::{
    error::{self, Error},
    fmt,
};

/// An error occurred in a program that nextest ran, not in nextest itself.
#[derive(Debug)]
#[doc(hidden)]
pub enum ExpectedError {
    CargoMetadataFailed,
    ProfileNotFound {
        err: ProfileNotFound,
    },
    ConfigParseError {
        err: ConfigParseError,
    },
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

    pub(crate) fn profile_not_found(err: ProfileNotFound) -> Self {
        Self::ProfileNotFound { err }
    }

    pub(crate) fn config_parse_error(err: ConfigParseError) -> Self {
        Self::ConfigParseError { err }
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
            Self::CargoMetadataFailed => NextestExitCode::CARGO_METADATA_FAILED,
            Self::ProfileNotFound { .. } | Self::ConfigParseError { .. } => {
                NextestExitCode::SETUP_ERROR
            }
            Self::BuildFailed { .. } => NextestExitCode::BUILD_FAILED,
            Self::TestRunFailed => NextestExitCode::TEST_RUN_FAILED,
        }
    }

    pub fn display_to_stderr(&self) {
        let mut next_error = match &self {
            Self::CargoMetadataFailed => {
                // The error produced by `cargo metadata` is enough.
                None
            }
            Self::ProfileNotFound { err } => {
                log::error!("{}", err);
                err.source()
            }
            Self::ConfigParseError { err } => {
                log::error!("{}", err);
                err.source()
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

                None
            }
            Self::TestRunFailed => {
                log::error!("test run failed");
                None
            }
        };

        while let Some(err) = next_error {
            log::error!(target: "cargo_nextest::no_heading", "\nCaused by:\n  {}", err);
            next_error = err.source();
        }
    }
}

impl fmt::Display for ExpectedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This should generally not be called, but provide a stub implementation it is
        match self {
            Self::CargoMetadataFailed => writeln!(f, "cargo metadata failed"),
            Self::ProfileNotFound { .. } => writeln!(f, "profile not found"),
            Self::ConfigParseError { .. } => writeln!(f, "config read error"),
            Self::BuildFailed { .. } => writeln!(f, "build failed"),
            Self::TestRunFailed => writeln!(f, "test run failed"),
        }
    }
}

impl error::Error for ExpectedError {}
