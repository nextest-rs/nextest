// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Debug command implementation.

use crate::{
    ExpectedError, Result,
    dispatch::helpers::{detect_build_platforms, display_output_slice, extract_slice_from_output},
    output::OutputContext,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Subcommand, ValueEnum};
use nextest_runner::{
    cargo_config::CargoConfigs,
    errors::{DisplayErrorChain, RecordReadError},
    record::{
        CARGO_METADATA_JSON_PATH, ExtractOuterFileResult, PORTABLE_MANIFEST_FILE_NAME,
        PortableRecording, RECORD_OPTS_JSON_PATH, RERUN_INFO_JSON_PATH, RUN_LOG_FILE_NAME,
        STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_ZIP_FILE_NAME, StoreReader, TEST_LIST_JSON_PATH,
    },
    redact::Redactor,
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
use std::{fmt, fs};
use tracing::{error, warn};

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

    /// Extract metadata files from a portable recording.
    ExtractPortableRecording(ExtractPortableRecordingOpts),
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
            DebugCommand::ExtractPortableRecording(opts) => {
                return opts.exec();
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

/// Options for `nextest debug extract-portable-recording`.
#[derive(Debug, Args)]
pub struct ExtractPortableRecordingOpts {
    /// Path to a portable recording (`.zip` file, or a pipe from process
    /// substitution such as `<(curl url)`).
    #[arg(value_name = "ARCHIVE")]
    archive: Utf8PathBuf,

    /// Output directory to extract files to.
    #[arg(value_name = "OUTPUT_DIR")]
    output_dir: Utf8PathBuf,
}

impl ExtractPortableRecordingOpts {
    fn exec(&self) -> Result<i32> {
        let redactor = if crate::output::should_redact() {
            Redactor::for_snapshot_testing()
        } else {
            Redactor::noop()
        };

        // Create the output directory if it doesn't exist.
        fs::create_dir_all(&self.output_dir).map_err(|err| {
            ExpectedError::DebugExtractRecordingError {
                message: format!(
                    "failed to create output directory: {}",
                    DisplayErrorChain::new(&err)
                ),
            }
        })?;

        let mut archive = PortableRecording::open(&self.archive)
            .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

        let mut has_errors = false;

        for file_name in [PORTABLE_MANIFEST_FILE_NAME, RUN_LOG_FILE_NAME] {
            let output_path = self.output_dir.join(file_name);
            match archive.extract_outer_file_to_path(file_name, &output_path, true) {
                Ok(result) => print_extraction_result(&output_path, &result, file_name, &redactor),
                Err(err) => {
                    error!(
                        "failed to extract {file_name}: {}",
                        DisplayErrorChain::new(&err)
                    );
                    has_errors = true;
                }
            }
        }

        let store_zip_path = self.output_dir.join(STORE_ZIP_FILE_NAME);
        match archive.extract_outer_file_to_path(
            STORE_ZIP_FILE_NAME,
            &store_zip_path,
            // store.zip does not have a limit check because it can be large.
            false,
        ) {
            Ok(result) => {
                print_extraction_result(&store_zip_path, &result, STORE_ZIP_FILE_NAME, &redactor)
            }
            Err(err) => {
                error!(
                    "failed to extract {STORE_ZIP_FILE_NAME}: {}",
                    DisplayErrorChain::new(&err)
                );
                has_errors = true;
            }
        }

        // Create the meta subdirectory for store contents.
        let meta_dir = self.output_dir.join("meta");
        fs::create_dir_all(&meta_dir).map_err(|err| ExpectedError::DebugExtractRecordingError {
            message: format!(
                "failed to create meta directory: {}",
                DisplayErrorChain::new(&err)
            ),
        })?;

        let mut store = archive
            .open_store()
            .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

        // Extract store metadata files (streaming).
        for store_path in [
            CARGO_METADATA_JSON_PATH,
            TEST_LIST_JSON_PATH,
            RECORD_OPTS_JSON_PATH,
            STDOUT_DICT_PATH,
            STDERR_DICT_PATH,
        ] {
            let output_path = self.output_dir.join(store_path);
            match store.extract_file_to_path(store_path, &output_path) {
                Ok(bytes_written) => {
                    println!(
                        "wrote {output_path} ({})",
                        redactor.redact_size(bytes_written)
                    );
                }
                Err(err) => {
                    error!(
                        "failed to extract {store_path}: {}",
                        DisplayErrorChain::new(&err)
                    );
                    has_errors = true;
                }
            }
        }

        // Extract rerun-info.json if it exists (only present for reruns).
        let rerun_info_path = self.output_dir.join(RERUN_INFO_JSON_PATH);
        match store.extract_file_to_path(RERUN_INFO_JSON_PATH, &rerun_info_path) {
            Ok(bytes_written) => {
                println!(
                    "wrote {rerun_info_path} ({})",
                    redactor.redact_size(bytes_written)
                );
            }
            Err(RecordReadError::FileNotFound { .. }) => {
                // File doesn't exist; this run is not a rerun.
            }
            Err(err) => {
                error!(
                    "failed to extract {RERUN_INFO_JSON_PATH}: {}",
                    DisplayErrorChain::new(&err)
                );
                has_errors = true;
            }
        }

        if has_errors {
            Err(ExpectedError::DebugExtractRecordingError {
                message: "one or more files failed to extract".to_string(),
            })
        } else {
            Ok(0)
        }
    }
}

/// Prints the result of extracting a file, including a warning if the file
/// exceeded the size limit.
fn print_extraction_result(
    output_path: &Utf8Path,
    result: &ExtractOuterFileResult,
    file_name: &str,
    redactor: &Redactor,
) {
    println!(
        "wrote {output_path} ({})",
        redactor.redact_size(result.bytes_written)
    );
    if let Some(claimed_size) = result.exceeded_limit {
        warn!(
            "{file_name} size ({}) exceeds limit ({})",
            redactor.redact_size(claimed_size),
            redactor.redact_size(MAX_MAX_OUTPUT_SIZE.as_u64()),
        );
    }
}
