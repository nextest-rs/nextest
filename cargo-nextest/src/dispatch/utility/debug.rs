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
use guppy::graph::PackageGraph;
use nextest_filtering::{FiltersetKind, KnownGroups};
use nextest_runner::{
    cargo_config::CargoConfigs,
    errors::{DisplayErrorChain, RecordReadError},
    list::SerializableFormat,
    platform::BuildPlatforms,
    record::{
        CARGO_METADATA_JSON_PATH, ExtractOuterFileResult, PORTABLE_MANIFEST_FILE_NAME,
        PortableRecording, RECORD_OPTS_JSON_PATH, RERUN_INFO_JSON_PATH, RUN_LOG_FILE_NAME,
        STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_ZIP_FILE_NAME, StoreReader, TEST_LIST_JSON_PATH,
    },
    redact::Redactor,
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
#[cfg(unix)]
use nextest_runner::{
    config::elements::CpuPriorityLevel,
    cpu_priority_probe::{
        CpuPriorityProbeReport, format_probe_report_human, run_cpu_priority_probe,
    },
};
use std::{collections::HashSet, fmt, fs};
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

    /// Parse and compile a filterset expression, displaying the result or errors.
    ///
    /// This is useful for testing how filterset expressions are validated in
    /// different contexts.
    ParseFilterset(ParseFiltersetOpts),

    // TODO-RAINCLAUDE: doc — probe whether CPU priority levels (the default) or specific --nice values can be applied on this system. Unix-only.
    #[cfg(unix)]
    ProbeNice(ProbeNiceOpts),
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
                    BuildPlatformsOutputFormat::Json => {
                        print_build_platforms_summary(&build_platforms, SerializableFormat::Json);
                    }
                    BuildPlatformsOutputFormat::JsonPretty => {
                        print_build_platforms_summary(
                            &build_platforms,
                            SerializableFormat::JsonPretty,
                        );
                    }
                }
            }
            DebugCommand::ExtractPortableRecording(opts) => {
                return opts.exec();
            }
            DebugCommand::ParseFilterset(opts) => {
                return opts.exec();
            }
            #[cfg(unix)]
            DebugCommand::ProbeNice(opts) => {
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

    /// Show the build platforms summary as JSON.
    Json,

    /// Show the build platforms summary as pretty-printed JSON.
    JsonPretty,
}

fn print_build_platforms_summary(build_platforms: &BuildPlatforms, format: SerializableFormat) {
    let summary = build_platforms.to_summary();
    let mut buf = String::new();
    format
        .to_writer(&summary, &mut buf)
        .expect("BuildPlatformsSummary serializes to JSON into a String");
    println!("{buf}");
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

/// The empty graph JSON used when no `--cargo-metadata` is provided.
const EMPTY_GRAPH: &str = r#"{
    "packages": [],
    "workspace_members": [],
    "workspace_root": "",
    "target_directory": "",
    "version": 1
}"#;

/// Options for `nextest debug parse-filterset`.
#[derive(Debug, Args)]
pub(crate) struct ParseFiltersetOpts {
    /// The filterset expression to parse.
    expression: String,

    /// The kind of filterset context to parse the expression in.
    #[arg(long, value_enum)]
    kind: FiltersetKindArg,

    /// Path to a cargo metadata JSON file.
    ///
    /// If not provided, an empty workspace graph is used. This is sufficient for
    /// checking banned predicates, but expressions referencing specific packages
    /// will report no-match warnings.
    #[arg(long)]
    cargo_metadata: Option<Utf8PathBuf>,
}

impl ParseFiltersetOpts {
    fn exec(self) -> Result<i32> {
        let json = match &self.cargo_metadata {
            Some(path) => {
                fs::read_to_string(path).map_err(|err| ExpectedError::CargoMetadataReadError {
                    path: path.clone(),
                    err,
                })?
            }
            None => EMPTY_GRAPH.to_string(),
        };

        let graph = PackageGraph::from_json(json).map_err(|err| {
            ExpectedError::CargoMetadataParseError {
                file_name: self.cargo_metadata.clone(),
                err: Box::new(err),
            }
        })?;

        let kind: FiltersetKind = self.kind.into();
        let cx = nextest_filtering::ParseContext::new(&graph);
        // The debug command doesn't load config, so no custom groups are
        // known. @global is always implicitly valid.
        let known_groups = KnownGroups::Known {
            custom_groups: HashSet::new(),
        };

        match nextest_filtering::Filterset::parse(self.expression, &cx, kind, &known_groups) {
            Ok(expr) => {
                println!("{expr:?}");
                Ok(0)
            }
            Err(errors) => {
                for single_error in &errors.errors {
                    let report = miette::Report::new(single_error.clone())
                        .with_source_code(errors.input.clone());
                    error!(target: "cargo_nextest::no_heading", "{report:?}");
                }
                error!("failed to parse filterset");
                Ok(1)
            }
        }
    }
}

/// The kind of filterset context, as a CLI argument.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum FiltersetKindArg {
    /// A test filterset (used with `nextest run -E` or `nextest list -E`).
    Test,
    /// An override filter in a config profile.
    OverrideFilter,
    /// A test archive filterset (used with `nextest archive -E`).
    ArchiveFilter,
    /// A default-filter filterset (used in profile configuration).
    DefaultFilter,
}

impl From<FiltersetKindArg> for FiltersetKind {
    fn from(arg: FiltersetKindArg) -> Self {
        match arg {
            FiltersetKindArg::Test => FiltersetKind::Test,
            FiltersetKindArg::OverrideFilter => FiltersetKind::OverrideFilter,
            FiltersetKindArg::ArchiveFilter => FiltersetKind::TestArchive,
            FiltersetKindArg::DefaultFilter => FiltersetKind::DefaultFilter,
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

// TODO-RAINCLAUDE: options for `nextest debug probe-nice`, which measures whether CPU priorities can be applied on this system.
#[cfg(unix)]
#[derive(Debug, Args)]
pub(crate) struct ProbeNiceOpts {
    // TODO-RAINCLAUDE: a specific nice value to probe; may be repeated. When omitted, every CPU priority level is probed.
    #[arg(long = "nice", value_name = "NICE")]
    nice_values: Vec<i32>,

    // TODO-RAINCLAUDE: output format.
    #[arg(long, value_enum, default_value_t)]
    output_format: ProbeNiceOutputFormat,
}

#[cfg(unix)]
impl ProbeNiceOpts {
    fn exec(self) -> Result<i32> {
        // TODO-RAINCLAUDE: default to probing every CPU priority level when no explicit --nice was passed.
        let nice_values: Vec<i32> = if self.nice_values.is_empty() {
            CpuPriorityLevel::ALL
                .iter()
                .map(|&level| level.to_nice())
                .collect()
        } else {
            self.nice_values
        };

        let report = run_cpu_priority_probe(&nice_values);

        match self.output_format {
            ProbeNiceOutputFormat::Human => print!("{}", format_probe_report_human(&report)),
            ProbeNiceOutputFormat::Json => {
                print_probe_report_json(&report, SerializableFormat::Json);
            }
            ProbeNiceOutputFormat::JsonPretty => {
                print_probe_report_json(&report, SerializableFormat::JsonPretty);
            }
        }

        Ok(0)
    }
}

#[cfg(unix)]
fn print_probe_report_json(report: &CpuPriorityProbeReport, format: SerializableFormat) {
    let mut buf = String::new();
    format
        .to_writer(report, &mut buf)
        .expect("probe report serializes to JSON into a String");
    println!("{buf}");
}

// TODO-RAINCLAUDE: output format for `nextest debug probe-nice`.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ProbeNiceOutputFormat {
    // TODO-RAINCLAUDE: human-readable, level-labeled report.
    #[default]
    Human,

    // TODO-RAINCLAUDE: the raw probe report as a single JSON line (used internally by nextest itself).
    Json,

    // TODO-RAINCLAUDE: the raw probe report as pretty-printed JSON.
    JsonPretty,
}

#[cfg(all(test, unix))]
mod probe_nice_tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestApp {
        #[command(subcommand)]
        command: DebugCommand,
    }

    #[test]
    fn parses_repeated_negative_nice_values() {
        let app = TestApp::try_parse_from([
            "debug",
            "probe-nice",
            "--nice=-10",
            "--nice=-5",
            "--output-format",
            "json",
        ])
        .expect("repeated negative --nice values parse");
        let DebugCommand::ProbeNice(opts) = app.command else {
            panic!("expected the probe-nice subcommand");
        };
        assert_eq!(opts.nice_values, vec![-10, -5]);
        assert!(matches!(opts.output_format, ProbeNiceOutputFormat::Json));
    }

    #[test]
    fn nice_values_default_to_empty_and_human_format() {
        let app = TestApp::try_parse_from(["debug", "probe-nice"])
            .expect("probe-nice parses with no arguments");
        let DebugCommand::ProbeNice(opts) = app.command else {
            panic!("expected the probe-nice subcommand");
        };
        assert!(opts.nice_values.is_empty());
        assert!(matches!(opts.output_format, ProbeNiceOutputFormat::Human));
    }
}
