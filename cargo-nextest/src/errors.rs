// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::ExtractOutputFormat;
use camino::Utf8PathBuf;
use itertools::Itertools;
use nextest_filtering::errors::FiltersetParseErrors;
use nextest_metadata::NextestExitCode;
use nextest_runner::{errors::*, redact::Redactor};
use owo_colors::{OwoColorize, Stream};
use semver::Version;
use std::{error::Error, string::FromUtf8Error};
use thiserror::Error;

pub(crate) type Result<T, E = ExpectedError> = std::result::Result<T, E>;

#[derive(Debug)]
#[doc(hidden)]
pub enum ReuseBuildKind {
    Normal,
    ReuseWithWorkspaceRemap { workspace_root: Utf8PathBuf },
    Reuse,
}

// Note that the #[error()] strings are mostly placeholder messages -- the expected way to print out
// errors is with the display_to_stderr method, which colorizes errors.

/// An error occurred in a program that nextest ran, not in nextest itself.
#[derive(Debug, Error)]
#[doc(hidden)]
pub enum ExpectedError {
    #[error("could not change to requested directory")]
    SetCurrentDirFailed { error: std::io::Error },
    #[error("cargo metadata exec failed")]
    CargoMetadataExecFailed {
        command: String,
        err: std::io::Error,
    },
    #[error("cargo metadata failed")]
    CargoMetadataFailed { command: String },
    #[error("cargo locate-project exec failed")]
    CargoLocateProjectExecFailed {
        command: String,
        err: std::io::Error,
    },
    #[error("cargo locate-project failed")]
    CargoLocateProjectFailed { command: String },
    #[error("workspace root is not valid UTF-8")]
    WorkspaceRootInvalidUtf8 {
        #[source]
        err: FromUtf8Error,
    },
    #[error("workspace root is invalid")]
    WorkspaceRootInvalid { workspace_root: Utf8PathBuf },
    #[error("root manifest not found at {path}")]
    RootManifestNotFound {
        path: Utf8PathBuf,
        reuse_build_kind: ReuseBuildKind,
    },
    #[error("profile not found")]
    ProfileNotFound {
        #[from]
        err: ProfileNotFound,
    },
    #[error("failed to create store directory")]
    StoreDirCreateError {
        store_dir: Utf8PathBuf,
        #[source]
        err: std::io::Error,
    },
    #[error("cargo config error")]
    CargoConfigError {
        #[from]
        err: Box<CargoConfigError>,
    },
    #[error("config parse error")]
    ConfigParseError {
        #[from]
        err: ConfigParseError,
    },
    #[error("test filter build error")]
    TestFilterBuilderError {
        #[from]
        err: TestFilterBuilderError,
    },
    #[error("unknown host platform")]
    UnknownHostPlatform {
        #[from]
        err: UnknownHostPlatform,
    },
    #[error("target triple error")]
    TargetTripleError {
        #[from]
        err: TargetTripleError,
    },
    #[error("metadata materialize error")]
    MetadataMaterializeError {
        arg_name: &'static str,
        #[source]
        err: Box<MetadataMaterializeError>,
    },
    #[error("unknown archive format")]
    UnknownArchiveFormat {
        archive_file: Utf8PathBuf,
        #[source]
        err: UnknownArchiveFormat,
    },
    #[error("archive create error")]
    ArchiveCreateError {
        archive_file: Utf8PathBuf,
        #[source]
        err: ArchiveCreateError,
        redactor: Redactor,
    },
    #[error("archive extract error")]
    ArchiveExtractError {
        archive_file: Utf8PathBuf,
        #[source]
        err: Box<ArchiveExtractError>,
    },
    #[error("path mapper construct error")]
    PathMapperConstructError {
        arg_name: &'static str,
        #[source]
        err: PathMapperConstructError,
    },
    #[error("cargo metadata parse error")]
    CargoMetadataParseError {
        file_name: Option<Utf8PathBuf>,
        #[source]
        err: guppy::Error,
    },
    #[error("rust build meta parse error")]
    RustBuildMetaParseError {
        #[from]
        err: RustBuildMetaParseError,
    },
    #[error("error parsing Cargo messages")]
    FromMessagesError {
        #[from]
        err: FromMessagesError,
    },
    #[error("create test list error")]
    CreateTestListError {
        #[source]
        err: CreateTestListError,
    },
    #[error("failed to execute build command")]
    BuildExecFailed {
        command: String,
        #[source]
        err: std::io::Error,
    },
    #[error("build failed")]
    BuildFailed {
        command: String,
        exit_code: Option<i32>,
    },
    #[error("building test runner failed")]
    TestRunnerBuildError {
        #[from]
        err: TestRunnerBuildError,
    },
    #[error("writing test list to output failed")]
    WriteTestListError {
        #[from]
        err: WriteTestListError,
    },
    #[error("writing event failed")]
    WriteEventError {
        #[from]
        err: WriteEventError,
    },
    #[error(transparent)]
    ConfigureHandleInheritanceError {
        #[from]
        err: ConfigureHandleInheritanceError,
    },
    #[error("show test groups error")]
    ShowTestGroupsError {
        #[from]
        err: ShowTestGroupsError,
    },
    #[error("setup script failed")]
    SetupScriptFailed,
    #[error("test run failed")]
    TestRunFailed,
    #[error("no tests to run")]
    NoTestsRun {
        /// The no-tests-run error was chosen because it was the default (we show a hint in this
        /// case)
        is_default: bool,
    },
    #[cfg(feature = "self-update")]
    #[error("failed to parse --version")]
    UpdateVersionParseError {
        #[from]
        err: UpdateVersionParseError,
    },
    #[cfg(feature = "self-update")]
    #[error("failed to update")]
    UpdateError {
        #[from]
        err: UpdateError,
    },
    #[error("error reading prompt")]
    DialoguerError {
        #[source]
        err: dialoguer::Error,
    },
    #[error("failed to set up Ctrl-C handler")]
    SignalHandlerSetupError {
        #[from]
        err: SignalHandlerSetupError,
    },
    #[error("required version not met")]
    RequiredVersionNotMet {
        required: Version,
        current: Version,
        tool: Option<String>,
    },
    #[error("experimental feature not enabled")]
    ExperimentalFeatureNotEnabled {
        name: &'static str,
        var_name: &'static str,
    },
    #[error("filterset parse error")]
    FiltersetParseError {
        all_errors: Vec<FiltersetParseErrors>,
    },
    #[error("test binary args parse error")]
    TestBinaryArgsParseError {
        reason: &'static str,
        args: Vec<String>,
    },
    #[error("double-spawn parse error")]
    DoubleSpawnParseArgsError {
        args: String,
        #[source]
        err: shell_words::ParseError,
    },
    #[error("double-spawn execution error")]
    DoubleSpawnExecError {
        command: std::process::Command,
        #[source]
        err: std::io::Error,
    },
    #[error("message format version is not valid")]
    InvalidMessageFormatVersion {
        #[from]
        err: FormatVersionError,
    },
    #[error("extract read error")]
    DebugExtractReadError {
        kind: &'static str,
        path: Utf8PathBuf,
        #[source]
        err: std::io::Error,
    },
    #[error("extract write error")]
    DebugExtractWriteError {
        format: ExtractOutputFormat,
        #[source]
        err: std::io::Error,
    },
}

impl ExpectedError {
    pub(crate) fn cargo_metadata_exec_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
        err: std::io::Error,
    ) -> Self {
        Self::CargoMetadataExecFailed {
            command: shell_words::join(command),
            err,
        }
    }

    pub(crate) fn cargo_metadata_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self::CargoMetadataFailed {
            command: shell_words::join(command),
        }
    }

    pub(crate) fn cargo_locate_project_exec_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
        err: std::io::Error,
    ) -> Self {
        Self::CargoLocateProjectExecFailed {
            command: shell_words::join(command),
            err,
        }
    }

    pub(crate) fn cargo_locate_project_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self::CargoLocateProjectFailed {
            command: shell_words::join(command),
        }
    }

    pub(crate) fn profile_not_found(err: ProfileNotFound) -> Self {
        Self::ProfileNotFound { err }
    }

    pub(crate) fn config_parse_error(err: ConfigParseError) -> Self {
        Self::ConfigParseError { err }
    }

    pub(crate) fn metadata_materialize_error(
        arg_name: &'static str,
        err: MetadataMaterializeError,
    ) -> Self {
        Self::MetadataMaterializeError {
            arg_name,
            err: Box::new(err),
        }
    }

    pub(crate) fn cargo_metadata_parse_error(
        file_name: impl Into<Option<Utf8PathBuf>>,
        err: guppy::Error,
    ) -> Self {
        Self::CargoMetadataParseError {
            file_name: file_name.into(),
            err,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn experimental_feature_error(name: &'static str, var_name: &'static str) -> Self {
        Self::ExperimentalFeatureNotEnabled { name, var_name }
    }

    pub(crate) fn build_exec_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
        err: std::io::Error,
    ) -> Self {
        Self::BuildExecFailed {
            command: shell_words::join(command),
            err,
        }
    }

    pub(crate) fn build_failed(
        command: impl IntoIterator<Item = impl AsRef<str>>,
        exit_code: Option<i32>,
    ) -> Self {
        Self::BuildFailed {
            command: shell_words::join(command),
            exit_code,
        }
    }

    pub(crate) fn filter_expression_parse_error(all_errors: Vec<FiltersetParseErrors>) -> Self {
        Self::FiltersetParseError { all_errors }
    }

    pub(crate) fn setup_script_failed() -> Self {
        Self::SetupScriptFailed
    }

    pub(crate) fn test_run_failed() -> Self {
        Self::TestRunFailed
    }

    pub(crate) fn test_binary_args_parse_error(reason: &'static str, args: Vec<String>) -> Self {
        Self::TestBinaryArgsParseError { reason, args }
    }

    /// Returns the exit code for the process.
    pub fn process_exit_code(&self) -> i32 {
        match self {
            Self::CargoMetadataExecFailed { .. }
            | Self::CargoMetadataFailed { .. }
            | Self::CargoLocateProjectExecFailed { .. }
            | Self::CargoLocateProjectFailed { .. } => NextestExitCode::CARGO_METADATA_FAILED,
            Self::WorkspaceRootInvalidUtf8 { .. }
            | Self::WorkspaceRootInvalid { .. }
            | Self::SetCurrentDirFailed { .. }
            | Self::ProfileNotFound { .. }
            | Self::StoreDirCreateError { .. }
            | Self::RootManifestNotFound { .. }
            | Self::CargoConfigError { .. }
            | Self::TestFilterBuilderError { .. }
            | Self::UnknownHostPlatform { .. }
            | Self::TargetTripleError { .. }
            | Self::MetadataMaterializeError { .. }
            | Self::UnknownArchiveFormat { .. }
            | Self::ArchiveExtractError { .. }
            | Self::RustBuildMetaParseError { .. }
            | Self::PathMapperConstructError { .. }
            | Self::TestRunnerBuildError { .. }
            | Self::ConfigureHandleInheritanceError { .. }
            | Self::CargoMetadataParseError { .. }
            | Self::TestBinaryArgsParseError { .. }
            | Self::DialoguerError { .. }
            | Self::SignalHandlerSetupError { .. }
            | Self::ShowTestGroupsError { .. }
            | Self::InvalidMessageFormatVersion { .. }
            | Self::DebugExtractReadError { .. } => NextestExitCode::SETUP_ERROR,
            Self::ConfigParseError { err } => {
                // Experimental features not being enabled are their own error.
                match err.kind() {
                    ConfigParseErrorKind::ExperimentalFeatureNotEnabled { .. } => {
                        NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED
                    }
                    _ => NextestExitCode::SETUP_ERROR,
                }
            }
            Self::RequiredVersionNotMet { .. } => NextestExitCode::REQUIRED_VERSION_NOT_MET,
            #[cfg(feature = "self-update")]
            Self::UpdateVersionParseError { .. } => NextestExitCode::SETUP_ERROR,
            Self::DoubleSpawnParseArgsError { .. } | Self::DoubleSpawnExecError { .. } => {
                NextestExitCode::DOUBLE_SPAWN_ERROR
            }
            Self::FromMessagesError { .. } | Self::CreateTestListError { .. } => {
                NextestExitCode::TEST_LIST_CREATION_FAILED
            }
            Self::BuildExecFailed { .. } | Self::BuildFailed { .. } => {
                NextestExitCode::BUILD_FAILED
            }
            Self::SetupScriptFailed => NextestExitCode::SETUP_SCRIPT_FAILED,
            Self::TestRunFailed => NextestExitCode::TEST_RUN_FAILED,
            Self::NoTestsRun { .. } => NextestExitCode::NO_TESTS_RUN,
            Self::ArchiveCreateError { .. } => NextestExitCode::ARCHIVE_CREATION_FAILED,
            Self::WriteTestListError { .. }
            | Self::WriteEventError { .. }
            | Self::DebugExtractWriteError { .. } => NextestExitCode::WRITE_OUTPUT_ERROR,
            #[cfg(feature = "self-update")]
            Self::UpdateError { .. } => NextestExitCode::UPDATE_ERROR,
            Self::ExperimentalFeatureNotEnabled { .. } => {
                NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED
            }
            Self::FiltersetParseError { .. } => NextestExitCode::INVALID_FILTERSET,
        }
    }

    /// Displays this error to stderr.
    pub fn display_to_stderr(&self) {
        let mut next_error = match &self {
            Self::SetCurrentDirFailed { error } => {
                log::error!("could not change to requested directory");
                Some(error as &dyn Error)
            }
            Self::CargoMetadataExecFailed { command, err } => {
                log::error!(
                    "failed to execute `{}`",
                    command.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::CargoMetadataFailed { .. } => {
                // The error produced by `cargo metadata` is enough.
                None
            }
            Self::CargoLocateProjectExecFailed { command, err } => {
                log::error!(
                    "failed to execute `{}`",
                    command.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::CargoLocateProjectFailed { .. } => {
                // The error produced by `cargo locate-project` is enough.
                None
            }
            Self::WorkspaceRootInvalidUtf8 { err } => {
                log::error!("workspace root is not valid UTF-8");
                Some(err as &dyn Error)
            }
            Self::WorkspaceRootInvalid { workspace_root } => {
                log::error!(
                    "workspace root `{}` is invalid",
                    workspace_root.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                None
            }
            Self::ProfileNotFound { err } => {
                log::error!("{}", err);
                err.source()
            }
            Self::RootManifestNotFound {
                path,
                reuse_build_kind,
            } => {
                let hint_str = match reuse_build_kind {
                    ReuseBuildKind::ReuseWithWorkspaceRemap { workspace_root } => {
                        format!(
                            "\n(hint: ensure that project source is available at {})",
                            workspace_root.if_supports_color(Stream::Stderr, |x| x.bold())
                        )
                    }
                    ReuseBuildKind::Reuse => {
                        "\n(hint: ensure that project source is available for reused build, \
                          using --workspace-remap if necessary)"
                            .to_owned()
                    }
                    ReuseBuildKind::Normal => String::new(),
                };
                log::error!(
                    "workspace root manifest at {} does not exist{hint_str}",
                    path.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                None
            }
            Self::StoreDirCreateError { store_dir, err } => {
                log::error!(
                    "failed to create store dir at `{}`",
                    store_dir.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::CargoConfigError { err } => {
                log::error!("{}", err);
                err.source()
            }
            Self::ConfigParseError { err } => {
                match err.kind() {
                    ConfigParseErrorKind::FiltersetOrCfgParseError(errors) => {
                        // Override errors are printed out using miette.
                        for override_error in errors {
                            log::error!(
                                "for config file `{}`{}, failed to parse overrides for profile: {}",
                                err.config_file(),
                                provided_by_tool(err.tool()),
                                override_error
                                    .profile_name
                                    .if_supports_color(Stream::Stderr, |p| p.bold()),
                            );
                            for report in override_error.reports() {
                                log::error!(target: "cargo_nextest::no_heading", "{report:?}");
                            }
                        }
                        None
                    }
                    ConfigParseErrorKind::UnknownTestGroups {
                        errors,
                        known_groups,
                    } => {
                        let known_groups_str = known_groups
                            .iter()
                            .map(|group_name| {
                                group_name.if_supports_color(Stream::Stderr, |x| x.bold())
                            })
                            .join(", ");
                        let mut errors_str = String::new();
                        for error in errors {
                            errors_str.push_str(&format!(
                                " - group `{}` in overrides for profile `{}`\n",
                                error.name.if_supports_color(Stream::Stderr, |x| x.bold()),
                                error
                                    .profile_name
                                    .if_supports_color(Stream::Stderr, |x| x.bold())
                            ));
                        }

                        log::error!(
                            "for config file `{}`{}, unknown test groups defined \
                            (known groups: {known_groups_str}):\n{errors_str}",
                            err.config_file(),
                            provided_by_tool(err.tool()),
                        );
                        None
                    }
                    ConfigParseErrorKind::UnknownConfigScripts {
                        errors,
                        known_scripts,
                    } => {
                        let known_scripts_str = known_scripts
                            .iter()
                            .map(|group_name| {
                                group_name.if_supports_color(Stream::Stderr, |x| x.bold())
                            })
                            .join(", ");
                        let mut errors_str = String::new();
                        for error in errors {
                            errors_str.push_str(&format!(
                                " - script `{}` specified within profile `{}`\n",
                                error.name.if_supports_color(Stream::Stderr, |x| x.bold()),
                                error
                                    .profile_name
                                    .if_supports_color(Stream::Stderr, |x| x.bold())
                            ));
                        }

                        log::error!(
                            "for config file `{}`{}, unknown scripts defined \
                        (known scripts: {known_scripts_str}):\n{errors_str}",
                            err.config_file(),
                            provided_by_tool(err.tool()),
                        );
                        None
                    }
                    ConfigParseErrorKind::UnknownExperimentalFeatures { unknown, known } => {
                        let unknown_str = unknown
                            .iter()
                            .map(|feature_name| {
                                feature_name.if_supports_color(Stream::Stderr, |x| x.bold())
                            })
                            .join(", ");
                        let known_str = known
                            .iter()
                            .map(|feature_name| {
                                feature_name.if_supports_color(Stream::Stderr, |x| x.bold())
                            })
                            .join(", ");

                        log::error!(
                            "for config file `{}`{}, unknown experimental features defined:
                             {unknown_str} (known features: {known_str}):",
                            err.config_file(),
                            provided_by_tool(err.tool()),
                        );
                        None
                    }
                    _ => {
                        // These other errors are printed out normally.
                        log::error!("{}", err);
                        err.source()
                    }
                }
            }
            Self::TestFilterBuilderError { err } => {
                log::error!("{err}");
                err.source()
            }
            Self::UnknownHostPlatform { err } => {
                log::error!("the host platform was unknown to nextest");
                Some(err as &dyn Error)
            }
            Self::TargetTripleError { err } => {
                log::error!("{err}");
                err.source()
            }
            Self::MetadataMaterializeError { arg_name, err } => {
                log::error!(
                    "error reading metadata from argument {}",
                    format!("--{arg_name}").if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::UnknownArchiveFormat { archive_file, err } => {
                log::error!(
                    "failed to autodetect archive format for {}",
                    archive_file.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::ArchiveCreateError {
                archive_file,
                err,
                redactor,
            } => {
                log::error!(
                    "error creating archive `{}`",
                    redactor
                        .redact_path(archive_file)
                        .if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::ArchiveExtractError { archive_file, err } => {
                log::error!(
                    "error extracting archive `{}`",
                    archive_file.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::RustBuildMetaParseError { err } => {
                log::error!("error parsing Rust build metadata");
                Some(err as &dyn Error)
            }
            Self::PathMapperConstructError { arg_name, err } => {
                log::error!(
                    "argument {} specified `{}` that couldn't be read",
                    format!("--{arg_name}").if_supports_color(Stream::Stderr, |x| x.bold()),
                    err.input().if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::CargoMetadataParseError { file_name, err } => {
                let metadata_source = match file_name {
                    Some(path) => format!(
                        " from file `{}`",
                        path.if_supports_color(Stream::Stderr, |x| x.bold())
                    ),
                    None => "".to_owned(),
                };
                log::error!("error parsing Cargo metadata{}", metadata_source);
                Some(err as &dyn Error)
            }
            Self::FromMessagesError { err } => {
                log::error!("failed to parse messages generated by Cargo");
                Some(err as &dyn Error)
            }
            Self::CreateTestListError { err } => {
                log::error!("creating test list failed");
                Some(err as &dyn Error)
            }
            Self::BuildExecFailed { command, err } => {
                log::error!(
                    "failed to execute `{}`",
                    command.if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::BuildFailed { command, exit_code } => {
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
                    "command `{}` exited{}",
                    command.if_supports_color(Stream::Stderr, |x| x.bold()),
                    with_code_str,
                );

                None
            }
            Self::TestRunnerBuildError { err } => {
                log::error!("failed to build test runner");
                Some(err as &dyn Error)
            }
            Self::ConfigureHandleInheritanceError { err } => {
                log::error!("{err}");
                err.source()
            }
            Self::WriteTestListError { err } => {
                log::error!("failed to write test list to output");
                Some(err as &dyn Error)
            }
            Self::WriteEventError { err } => {
                log::error!("failed to write event to output");
                Some(err as &dyn Error)
            }
            Self::SetupScriptFailed => {
                log::error!("setup script failed");
                None
            }
            Self::TestRunFailed => {
                log::error!("test run failed");
                None
            }
            Self::NoTestsRun { is_default } => {
                let hint_str = if *is_default {
                    "\n(hint: use `--no-tests` to customize)"
                } else {
                    ""
                };
                log::error!("no tests to run{hint_str}");
                None
            }
            Self::ShowTestGroupsError { err } => {
                log::error!("{err}");
                err.source()
            }
            Self::RequiredVersionNotMet {
                required,
                current,
                tool,
            } => {
                log::error!(
                    "this repository requires nextest version {}, but the current version is {}",
                    required.if_supports_color(Stream::Stderr, |x| x.bold()),
                    current.if_supports_color(Stream::Stderr, |x| x.bold()),
                );
                if let Some(tool) = tool {
                    log::info!(
                        target: "cargo_nextest::no_heading",
                        "(required version specified by tool `{}`)",
                        tool,
                    );
                }

                crate::helpers::log_needs_update(
                    log::Level::Info,
                    crate::helpers::BYPASS_VERSION_TEXT,
                );
                None
            }
            #[cfg(feature = "self-update")]
            Self::UpdateVersionParseError { err } => {
                log::error!("failed to parse --version");
                Some(err as &dyn Error)
            }
            #[cfg(feature = "self-update")]
            Self::UpdateError { err } => {
                log::error!(
                    "failed to update nextest (please update manually by visiting <{}>)",
                    "https://get.nexte.st".if_supports_color(Stream::Stderr, |x| x.bold())
                );
                Some(err as &dyn Error)
            }
            Self::DialoguerError { err } => {
                log::error!("error reading input prompt");
                Some(err as &dyn Error)
            }
            Self::SignalHandlerSetupError { err } => {
                log::error!("error setting up signal handler");
                Some(err as &dyn Error)
            }
            Self::ExperimentalFeatureNotEnabled { name, var_name } => {
                log::error!(
                    "{} is an experimental feature and must be enabled with {}=1",
                    name,
                    var_name
                );
                None
            }
            Self::FiltersetParseError { all_errors } => {
                for errors in all_errors {
                    for single_error in &errors.errors {
                        let report = miette::Report::new(single_error.clone())
                            .with_source_code(errors.input.to_owned());
                        log::error!(target: "cargo_nextest::no_heading", "{:?}", report);
                    }
                }

                log::error!("failed to parse filterset");
                None
            }
            Self::TestBinaryArgsParseError { reason, args } => {
                log::error!(
                    "failed to parse test binary arguments `{}`: arguments are {reason}",
                    args.join(", "),
                );
                None
            }
            Self::DoubleSpawnParseArgsError { args, err } => {
                log::error!("[double-spawn] failed to parse arguments `{args}`");
                Some(err as &dyn Error)
            }
            Self::DoubleSpawnExecError { command, err } => {
                log::error!("[double-spawn] failed to exec `{command:?}`");
                Some(err as &dyn Error)
            }
            Self::InvalidMessageFormatVersion { err } => {
                log::error!("error parsing message format version");
                Some(err as &dyn Error)
            }
            Self::DebugExtractReadError { kind, path, err } => {
                log::error!(
                    "error reading {kind} file `{}`",
                    path.if_supports_color(Stream::Stderr, |x| x.bold()),
                );
                Some(err as &dyn Error)
            }
            Self::DebugExtractWriteError { format, err } => {
                log::error!("error writing {format} output");
                Some(err as &dyn Error)
            }
        };

        while let Some(err) = next_error {
            log::error!(target: "cargo_nextest::no_heading", "\nCaused by:\n  {}", err);
            next_error = err.source();
        }
    }
}
