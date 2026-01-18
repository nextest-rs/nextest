// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Debug command implementation.

use crate::{
    ExpectedError, Result,
    dispatch::helpers::{detect_build_platforms, display_output_slice, extract_slice_from_output},
    output::OutputContext,
};
use camino::Utf8PathBuf;
use clap::{Subcommand, ValueEnum};
use nextest_runner::cargo_config::CargoConfigs;
use std::fmt;

/// Debug subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum DebugCommand {
    /// Show the data that nextest would extract from standard output or standard error.
    ///
    /// Text extraction is a heuristic process driven by a bunch of regexes and other similar logic.
    /// This command shows what nextest would extract from a given input.
    Extract {
        /// The path to the standard output produced by the test process.
        #[arg(long, required_unless_present_any = ["stderr", "combined"])]
        stdout: Option<Utf8PathBuf>,

        /// The path to the standard error produced by the test process.
        #[arg(long, required_unless_present_any = ["stdout", "combined"])]
        stderr: Option<Utf8PathBuf>,

        /// The combined output produced by the test process.
        #[arg(long, conflicts_with_all = ["stdout", "stderr"])]
        combined: Option<Utf8PathBuf>,

        /// The kind of output to produce.
        #[arg(value_enum)]
        output_format: ExtractOutputFormat,
    },

    /// Print the current executable path.
    CurrentExe,

    /// Show the build platforms that nextest would use.
    BuildPlatforms {
        /// The target triple to use.
        #[arg(long)]
        target: Option<String>,

        /// Override a Cargo Configuration value.
        #[arg(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        output_format: BuildPlatformsOutputFormat,
    },
}

impl DebugCommand {
    pub(crate) fn exec(self, output: OutputContext) -> Result<i32> {
        let _ = output;

        match self {
            DebugCommand::Extract {
                stdout,
                stderr,
                combined,
                output_format,
            } => {
                if let Some(combined) = combined {
                    let combined = std::fs::read(&combined).map_err(|err| {
                        ExpectedError::DebugExtractReadError {
                            kind: "combined",
                            path: combined,
                            err,
                        }
                    })?;

                    let description_kind = extract_slice_from_output(&combined, &combined);
                    display_output_slice(description_kind, output_format)?;
                } else {
                    let stdout = stdout
                        .map(|path| {
                            std::fs::read(&path).map_err(|err| {
                                ExpectedError::DebugExtractReadError {
                                    kind: "stdout",
                                    path,
                                    err,
                                }
                            })
                        })
                        .transpose()?
                        .unwrap_or_default();
                    let stderr = stderr
                        .map(|path| {
                            std::fs::read(&path).map_err(|err| {
                                ExpectedError::DebugExtractReadError {
                                    kind: "stderr",
                                    path,
                                    err,
                                }
                            })
                        })
                        .transpose()?
                        .unwrap_or_default();

                    let output_slice = extract_slice_from_output(&stdout, &stderr);
                    display_output_slice(output_slice, output_format)?;
                }
            }
            DebugCommand::CurrentExe => {
                let exe = std::env::current_exe()
                    .map_err(|err| ExpectedError::GetCurrentExeFailed { err })?;
                println!("{}", exe.display());
            }
            DebugCommand::BuildPlatforms {
                target,
                config,
                output_format,
            } => {
                let cargo_configs = CargoConfigs::new(&config).map_err(Box::new)?;
                let build_platforms = detect_build_platforms(&cargo_configs, target.as_deref())?;
                match output_format {
                    BuildPlatformsOutputFormat::Debug => {
                        println!("{build_platforms:#?}");
                    }
                    BuildPlatformsOutputFormat::Triple => {
                        println!(
                            "host triple: {}",
                            build_platforms.host.platform.triple().as_str()
                        );
                        if let Some(target) = &build_platforms.target {
                            println!(
                                "target triple: {}",
                                target.triple.platform.triple().as_str()
                            );
                        } else {
                            println!("target triple: (none)");
                        }
                    }
                }
            }
        }

        Ok(0)
    }
}

/// Output format for `nextest debug extract`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExtractOutputFormat {
    /// Show the raw text extracted.
    Raw,

    /// Show what would be put in the description field of JUnit reports.
    ///
    /// This is similar to `Raw`, but is valid Unicode, and strips out ANSI escape codes and other
    /// invalid XML characters.
    JunitDescription,

    /// Show what would be highlighted in nextest's output.
    Highlight,
}

impl fmt::Display for ExtractOutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::JunitDescription => write!(f, "junit-description"),
            Self::Highlight => write!(f, "highlight"),
        }
    }
}

/// Output format for `nextest debug build-platforms`.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum BuildPlatformsOutputFormat {
    /// Show Debug output.
    #[default]
    Debug,

    /// Show just the triple.
    Triple,
}
