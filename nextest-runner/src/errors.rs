// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors produced by nextest.

use crate::{
    cargo_config::{TargetTriple, TargetTripleSource},
    config::{
        core::{ConfigExperimental, ToolName},
        elements::{CustomTestGroup, TestGroup},
        scripts::{ProfileScriptType, ScriptId, ScriptType},
    },
    helpers::{display_exited_with, dylib_path_envvar},
    indenter::{DisplayIndented, indented},
    record::{RecordedRunInfo, RunIdIndex},
    redact::Redactor,
    reuse_build::{ArchiveFormat, ArchiveStep},
    target_runner::PlatformRunnerSource,
};
use camino::{FromPathBufError, Utf8Path, Utf8PathBuf};
use config::ConfigError;
use itertools::{Either, Itertools};
use nextest_filtering::errors::FiltersetParseErrors;
use nextest_metadata::RustBinaryId;
use quick_junit::ReportUuid;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    borrow::Cow,
    collections::BTreeSet,
    env::JoinPathsError,
    fmt::{self, Write as _},
    path::PathBuf,
    process::ExitStatus,
    sync::Arc,
};
use target_spec_miette::IntoMietteDiagnostic;
use thiserror::Error;

/// An error that occurred while parsing the config.
#[derive(Debug, Error)]
#[error(
    "failed to parse nextest config at `{config_file}`{}",
    provided_by_tool(tool.as_ref())
)]
#[non_exhaustive]
pub struct ConfigParseError {
    config_file: Utf8PathBuf,
    tool: Option<ToolName>,
    #[source]
    kind: ConfigParseErrorKind,
}

impl ConfigParseError {
    pub(crate) fn new(
        config_file: impl Into<Utf8PathBuf>,
        tool: Option<&ToolName>,
        kind: ConfigParseErrorKind,
    ) -> Self {
        Self {
            config_file: config_file.into(),
            tool: tool.cloned(),
            kind,
        }
    }

    /// Returns the config file for this error.
    pub fn config_file(&self) -> &Utf8Path {
        &self.config_file
    }

    /// Returns the tool name associated with this error.
    pub fn tool(&self) -> Option<&ToolName> {
        self.tool.as_ref()
    }

    /// Returns the kind of error this is.
    pub fn kind(&self) -> &ConfigParseErrorKind {
        &self.kind
    }
}

/// Returns the string ` provided by tool <tool>`, if `tool` is `Some`.
pub fn provided_by_tool(tool: Option<&ToolName>) -> String {
    match tool {
        Some(tool) => format!(" provided by tool `{tool}`"),
        None => String::new(),
    }
}

/// The kind of error that occurred while parsing a config.
///
/// Returned by [`ConfigParseError::kind`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigParseErrorKind {
    /// An error occurred while building the config.
    #[error(transparent)]
    BuildError(Box<ConfigError>),
    /// An error occurred while parsing the config into a table.
    #[error(transparent)]
    TomlParseError(Box<toml::de::Error>),
    #[error(transparent)]
    /// An error occurred while deserializing the config.
    DeserializeError(Box<serde_path_to_error::Error<ConfigError>>),
    /// An error occurred while reading the config file (version only).
    #[error(transparent)]
    VersionOnlyReadError(std::io::Error),
    /// An error occurred while deserializing the config (version only).
    #[error(transparent)]
    VersionOnlyDeserializeError(Box<serde_path_to_error::Error<toml::de::Error>>),
    /// Errors occurred while compiling configuration strings.
    #[error("error parsing compiled data (destructure this variant for more details)")]
    CompileErrors(Vec<ConfigCompileError>),
    /// An invalid set of test groups was defined by the user.
    #[error("invalid test groups defined: {}\n(test groups cannot start with '@tool:' unless specified by a tool)", .0.iter().join(", "))]
    InvalidTestGroupsDefined(BTreeSet<CustomTestGroup>),
    /// An invalid set of test groups was defined by a tool config file.
    #[error(
        "invalid test groups defined by tool: {}\n(test groups must start with '@tool:<tool-name>:')", .0.iter().join(", "))]
    InvalidTestGroupsDefinedByTool(BTreeSet<CustomTestGroup>),
    /// Some test groups were unknown.
    #[error("unknown test groups specified by config (destructure this variant for more details)")]
    UnknownTestGroups {
        /// The list of errors that occurred.
        errors: Vec<UnknownTestGroupError>,

        /// Known groups up to this point.
        known_groups: BTreeSet<TestGroup>,
    },
    /// Both `[script.*]` and `[scripts.*]` were defined.
    #[error(
        "both `[script.*]` and `[scripts.*]` defined\n\
         (hint: [script.*] will be removed in the future: switch to [scripts.setup.*])"
    )]
    BothScriptAndScriptsDefined,
    /// An invalid set of config scripts was defined by the user.
    #[error("invalid config scripts defined: {}\n(config scripts cannot start with '@tool:' unless specified by a tool)", .0.iter().join(", "))]
    InvalidConfigScriptsDefined(BTreeSet<ScriptId>),
    /// An invalid set of config scripts was defined by a tool config file.
    #[error(
        "invalid config scripts defined by tool: {}\n(config scripts must start with '@tool:<tool-name>:')", .0.iter().join(", "))]
    InvalidConfigScriptsDefinedByTool(BTreeSet<ScriptId>),
    /// The same config script name was used across config script types.
    #[error(
        "config script names used more than once: {}\n\
         (config script names must be unique across all script types)", .0.iter().join(", ")
    )]
    DuplicateConfigScriptNames(BTreeSet<ScriptId>),
    /// Errors occurred while parsing `[[profile.<profile-name>.scripts]]`.
    #[error(
        "errors in profile-specific config scripts (destructure this variant for more details)"
    )]
    ProfileScriptErrors {
        /// The errors that occurred.
        errors: Box<ProfileScriptErrors>,

        /// Known scripts up to this point.
        known_scripts: BTreeSet<ScriptId>,
    },
    /// An unknown experimental feature or features were defined.
    #[error("unknown experimental features defined (destructure this variant for more details)")]
    UnknownExperimentalFeatures {
        /// The set of unknown features.
        unknown: BTreeSet<String>,

        /// The set of known features.
        known: BTreeSet<ConfigExperimental>,
    },
    /// A tool specified an experimental feature.
    ///
    /// Tools are not allowed to specify experimental features.
    #[error(
        "tool config file specifies experimental features `{}` \
         -- only repository config files can do so",
        .features.iter().join(", "),
    )]
    ExperimentalFeaturesInToolConfig {
        /// The name of the experimental feature.
        features: BTreeSet<String>,
    },
    /// Experimental features were used but not enabled.
    #[error("experimental features used but not enabled: {}", .missing_features.iter().join(", "))]
    ExperimentalFeaturesNotEnabled {
        /// The features that were not enabled.
        missing_features: BTreeSet<ConfigExperimental>,
    },
    /// An inheritance cycle was detected in the profile configuration.
    #[error("inheritance error(s) detected: {}", .0.iter().join(", "))]
    InheritanceErrors(Vec<InheritsError>),
}

/// An error that occurred while compiling overrides or scripts specified in
/// configuration.
#[derive(Debug)]
#[non_exhaustive]
pub struct ConfigCompileError {
    /// The name of the profile under which the data was found.
    pub profile_name: String,

    /// The section within the profile where the error occurred.
    pub section: ConfigCompileSection,

    /// The kind of error that occurred.
    pub kind: ConfigCompileErrorKind,
}

/// For a [`ConfigCompileError`], the section within the profile where the error
/// occurred.
#[derive(Debug)]
pub enum ConfigCompileSection {
    /// `profile.<profile-name>.default-filter`.
    DefaultFilter,

    /// `[[profile.<profile-name>.overrides]]` at the corresponding index.
    Override(usize),

    /// `[[profile.<profile-name>.scripts]]` at the corresponding index.
    Script(usize),
}

/// The kind of error that occurred while parsing config overrides.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigCompileErrorKind {
    /// Neither `platform` nor `filter` were specified.
    ConstraintsNotSpecified {
        /// Whether `default-filter` was specified.
        ///
        /// If default-filter is specified, then specifying `filter` is not
        /// allowed -- so we show a different message in that case.
        default_filter_specified: bool,
    },

    /// Both `filter` and `default-filter` were specified.
    ///
    /// It only makes sense to specify one of the two.
    FilterAndDefaultFilterSpecified,

    /// One or more errors occured while parsing expressions.
    Parse {
        /// A potential error that occurred while parsing the host platform expression.
        host_parse_error: Option<target_spec::Error>,

        /// A potential error that occurred while parsing the target platform expression.
        target_parse_error: Option<target_spec::Error>,

        /// Filterset or default filter parse errors.
        filter_parse_errors: Vec<FiltersetParseErrors>,
    },
}

impl ConfigCompileErrorKind {
    /// Returns [`miette::Report`]s for each error recorded by self.
    pub fn reports(&self) -> impl Iterator<Item = miette::Report> + '_ {
        match self {
            Self::ConstraintsNotSpecified {
                default_filter_specified,
            } => {
                let message = if *default_filter_specified {
                    "for override with `default-filter`, `platform` must also be specified"
                } else {
                    "at least one of `platform` and `filter` must be specified"
                };
                Either::Left(std::iter::once(miette::Report::msg(message)))
            }
            Self::FilterAndDefaultFilterSpecified => {
                Either::Left(std::iter::once(miette::Report::msg(
                    "at most one of `filter` and `default-filter` must be specified",
                )))
            }
            Self::Parse {
                host_parse_error,
                target_parse_error,
                filter_parse_errors,
            } => {
                let host_parse_report = host_parse_error
                    .as_ref()
                    .map(|error| miette::Report::new_boxed(error.clone().into_diagnostic()));
                let target_parse_report = target_parse_error
                    .as_ref()
                    .map(|error| miette::Report::new_boxed(error.clone().into_diagnostic()));
                let filter_parse_reports =
                    filter_parse_errors.iter().flat_map(|filter_parse_errors| {
                        filter_parse_errors.errors.iter().map(|single_error| {
                            miette::Report::new(single_error.clone())
                                .with_source_code(filter_parse_errors.input.to_owned())
                        })
                    });

                Either::Right(
                    host_parse_report
                        .into_iter()
                        .chain(target_parse_report)
                        .chain(filter_parse_reports),
                )
            }
        }
    }
}

/// A test priority specified was out of range.
#[derive(Clone, Debug, Error)]
#[error("test priority ({priority}) out of range: must be between -100 and 100, both inclusive")]
pub struct TestPriorityOutOfRange {
    /// The priority that was out of range.
    pub priority: i8,
}

/// An execution error occurred while attempting to start a test.
#[derive(Clone, Debug, Error)]
pub enum ChildStartError {
    /// An error occurred while creating a temporary path for a setup script.
    #[error("error creating temporary path for setup script")]
    TempPath(#[source] Arc<std::io::Error>),

    /// An error occurred while spawning the child process.
    #[error("error spawning child process")]
    Spawn(#[source] Arc<std::io::Error>),
}

/// An error that occurred while reading the output of a setup script.
#[derive(Clone, Debug, Error)]
pub enum SetupScriptOutputError {
    /// An error occurred while opening the setup script environment file.
    #[error("error opening environment file `{path}`")]
    EnvFileOpen {
        /// The path to the environment file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: Arc<std::io::Error>,
    },

    /// An error occurred while reading the setup script environment file.
    #[error("error reading environment file `{path}`")]
    EnvFileRead {
        /// The path to the environment file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: Arc<std::io::Error>,
    },

    /// An error occurred while parsing the setup script environment file.
    #[error("line `{line}` in environment file `{path}` not in KEY=VALUE format")]
    EnvFileParse {
        /// The path to the environment file.
        path: Utf8PathBuf,
        /// The line at issue.
        line: String,
    },

    /// An environment variable key was reserved.
    #[error("key `{key}` begins with `NEXTEST`, which is reserved for internal use")]
    EnvFileReservedKey {
        /// The environment variable name.
        key: String,
    },
}

/// A list of errors that implements `Error`.
///
/// In the future, we'll likely want to replace this with a `miette::Diagnostic`-based error, since
/// that supports multiple causes via "related".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorList<T> {
    // A description of what the errors are.
    description: Cow<'static, str>,
    // Invariant: this list is non-empty.
    inner: Vec<T>,
}

impl<T: std::error::Error> ErrorList<T> {
    pub(crate) fn new<U>(description: &'static str, errors: Vec<U>) -> Option<Self>
    where
        T: From<U>,
    {
        if errors.is_empty() {
            None
        } else {
            Some(Self {
                description: Cow::Borrowed(description),
                inner: errors.into_iter().map(T::from).collect(),
            })
        }
    }

    /// Returns a short summary of the error list.
    pub(crate) fn short_message(&self) -> String {
        let string = self.to_string();
        match string.lines().next() {
            // Remove a trailing colon if it exists for a better UX.
            Some(first_line) => first_line.trim_end_matches(':').to_string(),
            None => String::new(),
        }
    }

    /// Returns the description of what the errors are.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Iterates over the errors in this list.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inner.iter()
    }

    /// Transforms the errors in this list using the given function.
    pub fn map<U, F>(self, f: F) -> ErrorList<U>
    where
        U: std::error::Error,
        F: FnMut(T) -> U,
    {
        ErrorList {
            description: self.description,
            inner: self.inner.into_iter().map(f).collect(),
        }
    }
}

impl<T: std::error::Error> IntoIterator for ErrorList<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<T: std::error::Error> fmt::Display for ErrorList<T> {
    fn fmt(&self, mut f: &mut fmt::Formatter) -> fmt::Result {
        // If a single error occurred, pretend that this is just that.
        if self.inner.len() == 1 {
            return write!(f, "{}", self.inner[0]);
        }

        // Otherwise, list all errors.
        writeln!(
            f,
            "{} errors occurred {}:",
            self.inner.len(),
            self.description,
        )?;
        for error in &self.inner {
            let mut indent = indented(f).with_str("  ").skip_initial();
            writeln!(indent, "* {}", DisplayErrorChain::new(error))?;
            f = indent.into_inner();
        }
        Ok(())
    }
}

#[cfg(test)]
impl<T: proptest::arbitrary::Arbitrary + std::fmt::Debug + 'static> proptest::arbitrary::Arbitrary
    for ErrorList<T>
{
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;

        // Generate 1-5 errors (non-empty).
        proptest::collection::vec(any::<T>(), 1..=5)
            .prop_map(|inner| ErrorList {
                description: Cow::Borrowed("test errors"),
                inner,
            })
            .boxed()
    }
}

impl<T: std::error::Error> std::error::Error for ErrorList<T> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if self.inner.len() == 1 {
            self.inner[0].source()
        } else {
            // More than one error occurred, so we can't return a single error here. Instead, we
            // return `None` and display the chain of causes in `fmt::Display`.
            None
        }
    }
}

/// A wrapper type to display a chain of errors with internal indentation.
///
/// This is similar to the display-error-chain crate, but uses an indenter
/// internally to ensure that subsequent lines are also nested.
pub struct DisplayErrorChain<E> {
    error: E,
    initial_indent: &'static str,
}

impl<E: std::error::Error> DisplayErrorChain<E> {
    /// Creates a new `DisplayErrorChain` with the given error.
    pub fn new(error: E) -> Self {
        Self {
            error,
            initial_indent: "",
        }
    }

    /// Creates a new `DisplayErrorChain` with the given error and initial indentation.
    pub fn new_with_initial_indent(initial_indent: &'static str, error: E) -> Self {
        Self {
            error,
            initial_indent,
        }
    }
}

impl<E> fmt::Display for DisplayErrorChain<E>
where
    E: std::error::Error,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut writer = indented(f).with_str(self.initial_indent);
        write!(writer, "{}", self.error)?;

        let Some(mut cause) = self.error.source() else {
            return Ok(());
        };

        write!(writer, "\n  caused by:")?;

        loop {
            writeln!(writer)?;
            // Wrap the existing writer to accumulate indentation.
            let mut indent = indented(&mut writer).with_str("    ").skip_initial();
            write!(indent, "  - {cause}")?;

            let Some(next_cause) = cause.source() else {
                break Ok(());
            };

            cause = next_cause;
        }
    }
}

/// An error was returned while managing a child process or reading its output.
#[derive(Clone, Debug, Error)]
pub enum ChildError {
    /// An error occurred while reading from a child file descriptor.
    #[error(transparent)]
    Fd(#[from] ChildFdError),

    /// An error occurred while reading the output of a setup script.
    #[error(transparent)]
    SetupScriptOutput(#[from] SetupScriptOutputError),
}

/// An error was returned while reading from child a file descriptor.
#[derive(Clone, Debug, Error)]
pub enum ChildFdError {
    /// An error occurred while reading standard output.
    #[error("error reading standard output")]
    ReadStdout(#[source] Arc<std::io::Error>),

    /// An error occurred while reading standard error.
    #[error("error reading standard error")]
    ReadStderr(#[source] Arc<std::io::Error>),

    /// An error occurred while reading a combined stream.
    #[error("error reading combined stream")]
    ReadCombined(#[source] Arc<std::io::Error>),

    /// An error occurred while waiting for the child process to exit.
    #[error("error waiting for child process to exit")]
    Wait(#[source] Arc<std::io::Error>),
}

/// An unknown test group was specified in the config.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct UnknownTestGroupError {
    /// The name of the profile under which the unknown test group was found.
    pub profile_name: String,

    /// The name of the unknown test group.
    pub name: TestGroup,
}

/// While parsing profile-specific config scripts, an unknown script was
/// encountered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileUnknownScriptError {
    /// The name of the profile under which the errors occurred.
    pub profile_name: String,

    /// The name of the unknown script.
    pub name: ScriptId,
}

/// While parsing profile-specific config scripts, a script of the wrong type
/// was encountered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileWrongConfigScriptTypeError {
    /// The name of the profile under which the errors occurred.
    pub profile_name: String,

    /// The name of the config script.
    pub name: ScriptId,

    /// The script type that the user attempted to use the script as.
    pub attempted: ProfileScriptType,

    /// The script type that the script actually is.
    pub actual: ScriptType,
}

/// While parsing profile-specific config scripts, a list-time-enabled script
/// used a filter that can only be used at test run time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileListScriptUsesRunFiltersError {
    /// The name of the profile under which the errors occurred.
    pub profile_name: String,

    /// The name of the config script.
    pub name: ScriptId,

    /// The script type.
    pub script_type: ProfileScriptType,

    /// The filters that were used.
    pub filters: BTreeSet<String>,
}

/// Errors that occurred while parsing `[[profile.*.scripts]]`.
#[derive(Clone, Debug, Default)]
pub struct ProfileScriptErrors {
    /// The list of unknown script errors.
    pub unknown_scripts: Vec<ProfileUnknownScriptError>,

    /// The list of wrong script type errors.
    pub wrong_script_types: Vec<ProfileWrongConfigScriptTypeError>,

    /// The list of list-time-enabled scripts that used a run-time filter.
    pub list_scripts_using_run_filters: Vec<ProfileListScriptUsesRunFiltersError>,
}

impl ProfileScriptErrors {
    /// Returns true if there are no errors recorded.
    pub fn is_empty(&self) -> bool {
        self.unknown_scripts.is_empty()
            && self.wrong_script_types.is_empty()
            && self.list_scripts_using_run_filters.is_empty()
    }
}

/// An error which indicates that a profile was requested but not known to nextest.
#[derive(Clone, Debug, Error)]
#[error("profile `{profile}` not found (known profiles: {})", .all_profiles.join(", "))]
pub struct ProfileNotFound {
    profile: String,
    all_profiles: Vec<String>,
}

impl ProfileNotFound {
    pub(crate) fn new(
        profile: impl Into<String>,
        all_profiles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut all_profiles: Vec<_> = all_profiles.into_iter().map(|s| s.into()).collect();
        all_profiles.sort_unstable();
        Self {
            profile: profile.into(),
            all_profiles,
        }
    }
}

/// An identifier is invalid.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InvalidIdentifier {
    /// The identifier is empty.
    #[error("identifier is empty")]
    Empty,

    /// The identifier is not in the correct Unicode format.
    #[error("invalid identifier `{0}`")]
    InvalidXid(SmolStr),

    /// This tool identifier doesn't match the expected pattern.
    #[error("tool identifier not of the form \"@tool:tool-name:identifier\": `{0}`")]
    ToolIdentifierInvalidFormat(SmolStr),

    /// One of the components of this tool identifier is empty.
    #[error("tool identifier has empty component: `{0}`")]
    ToolComponentEmpty(SmolStr),

    /// The tool identifier is not in the correct Unicode format.
    #[error("invalid tool identifier `{0}`")]
    ToolIdentifierInvalidXid(SmolStr),
}

/// A tool name is invalid.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InvalidToolName {
    /// The tool name is empty.
    #[error("tool name is empty")]
    Empty,

    /// The tool name is not in the correct Unicode format.
    #[error("invalid tool name `{0}`")]
    InvalidXid(SmolStr),

    /// The tool name starts with "@tool", which is reserved for tool identifiers.
    #[error("tool name cannot start with \"@tool\": `{0}`")]
    StartsWithToolPrefix(SmolStr),
}

/// The name of a test group is invalid (not a valid identifier).
#[derive(Clone, Debug, Error)]
#[error("invalid custom test group name: {0}")]
pub struct InvalidCustomTestGroupName(pub InvalidIdentifier);

/// The name of a configuration script is invalid (not a valid identifier).
#[derive(Clone, Debug, Error)]
#[error("invalid configuration script name: {0}")]
pub struct InvalidConfigScriptName(pub InvalidIdentifier);

/// Error returned while parsing a [`ToolConfigFile`](crate::config::core::ToolConfigFile) value.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ToolConfigFileParseError {
    #[error(
        "tool-config-file has invalid format: {input}\n(hint: tool configs must be in the format <tool-name>:<path>)"
    )]
    /// The input was not in the format "tool:path".
    InvalidFormat {
        /// The input that failed to parse.
        input: String,
    },

    /// The tool name was invalid.
    #[error("tool-config-file has invalid tool name: {input}")]
    InvalidToolName {
        /// The input that failed to parse.
        input: String,

        /// The error that occurred.
        #[source]
        error: InvalidToolName,
    },

    /// The config file path was empty.
    #[error("tool-config-file has empty config file path: {input}")]
    EmptyConfigFile {
        /// The input that failed to parse.
        input: String,
    },

    /// The config file was not an absolute path.
    #[error("tool-config-file is not an absolute path: {config_file}")]
    ConfigFileNotAbsolute {
        /// The file name that wasn't absolute.
        config_file: Utf8PathBuf,
    },
}

/// Errors that can occur while loading user config.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UserConfigError {
    /// The user config file specified via `--user-config-file` or
    /// `NEXTEST_USER_CONFIG_FILE` does not exist.
    #[error("user config file not found at {path}")]
    FileNotFound {
        /// The path that was specified.
        path: Utf8PathBuf,
    },

    /// Failed to read the user config file.
    #[error("failed to read user config at {path}")]
    Read {
        /// The path to the config file.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        error: std::io::Error,
    },

    /// Failed to parse the user config file.
    #[error("failed to parse user config at {path}")]
    Parse {
        /// The path to the config file.
        path: Utf8PathBuf,
        /// The underlying TOML parse error.
        #[source]
        error: toml::de::Error,
    },

    /// The user config path contains non-UTF-8 characters.
    #[error("user config path contains non-UTF-8 characters")]
    NonUtf8Path {
        /// The underlying error from path conversion.
        #[source]
        error: FromPathBufError,
    },

    /// Failed to compile a platform spec in an override.
    #[error(
        "for user config at {path}, failed to compile platform spec in [[overrides]] at index {index}"
    )]
    OverridePlatformSpec {
        /// The path to the config file.
        path: Utf8PathBuf,
        /// The index of the override in the array.
        index: usize,
        /// The underlying target-spec error.
        #[source]
        error: target_spec::Error,
    },
}

/// Error returned while parsing a [`MaxFail`](crate::config::elements::MaxFail) input.
#[derive(Clone, Debug, Error)]
#[error("unrecognized value for max-fail: {reason}")]
pub struct MaxFailParseError {
    /// The reason parsing failed.
    pub reason: String,
}

impl MaxFailParseError {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Error returned while parsing a [`StressCount`](crate::runner::StressCount) input.
#[derive(Clone, Debug, Error)]
#[error(
    "unrecognized value for stress-count: {input}\n\
     (hint: expected either a positive integer or \"infinite\")"
)]
pub struct StressCountParseError {
    /// The input that failed to parse.
    pub input: String,
}

impl StressCountParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

/// An error that occurred while parsing a debugger command.
#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum DebuggerCommandParseError {
    /// The command string could not be parsed as shell words.
    #[error(transparent)]
    ShellWordsParse(shell_words::ParseError),

    /// The command was empty.
    #[error("debugger command cannot be empty")]
    EmptyCommand,
}

/// An error that occurred while parsing a tracer command.
#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum TracerCommandParseError {
    /// The command string could not be parsed as shell words.
    #[error(transparent)]
    ShellWordsParse(shell_words::ParseError),

    /// The command was empty.
    #[error("tracer command cannot be empty")]
    EmptyCommand,
}

/// Error returned while parsing a [`TestThreads`](crate::config::elements::TestThreads) value.
#[derive(Clone, Debug, Error)]
#[error(
    "unrecognized value for test-threads: {input}\n(hint: expected either an integer or \"num-cpus\")"
)]
pub struct TestThreadsParseError {
    /// The input that failed to parse.
    pub input: String,
}

impl TestThreadsParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

/// An error that occurs while parsing a
/// [`PartitionerBuilder`](crate::partition::PartitionerBuilder) input.
#[derive(Clone, Debug, Error)]
pub struct PartitionerBuilderParseError {
    expected_format: Option<&'static str>,
    message: Cow<'static, str>,
}

impl PartitionerBuilderParseError {
    pub(crate) fn new(
        expected_format: Option<&'static str>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            expected_format,
            message: message.into(),
        }
    }
}

impl fmt::Display for PartitionerBuilderParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.expected_format {
            Some(format) => {
                write!(
                    f,
                    "partition must be in the format \"{}\":\n{}",
                    format, self.message
                )
            }
            None => write!(f, "{}", self.message),
        }
    }
}

/// An error that occurs while operating on a
/// [`TestFilterBuilder`](crate::test_filter::TestFilterBuilder).
#[derive(Clone, Debug, Error)]
pub enum TestFilterBuilderError {
    /// An error that occurred while constructing test filters.
    #[error("error constructing test filters")]
    Construct {
        /// The underlying error.
        #[from]
        error: aho_corasick::BuildError,
    },
}

/// An error occurred in [`PathMapper::new`](crate::reuse_build::PathMapper::new).
#[derive(Debug, Error)]
pub enum PathMapperConstructError {
    /// An error occurred while canonicalizing a directory.
    #[error("{kind} `{input}` failed to canonicalize")]
    Canonicalization {
        /// The directory that failed to be canonicalized.
        kind: PathMapperConstructKind,

        /// The input provided.
        input: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        err: std::io::Error,
    },
    /// The canonicalized path isn't valid UTF-8.
    #[error("{kind} `{input}` canonicalized to a non-UTF-8 path")]
    NonUtf8Path {
        /// The directory that failed to be canonicalized.
        kind: PathMapperConstructKind,

        /// The input provided.
        input: Utf8PathBuf,

        /// The underlying error.
        #[source]
        err: FromPathBufError,
    },
    /// A provided input is not a directory.
    #[error("{kind} `{canonicalized_path}` is not a directory")]
    NotADirectory {
        /// The directory that failed to be canonicalized.
        kind: PathMapperConstructKind,

        /// The input provided.
        input: Utf8PathBuf,

        /// The canonicalized path that wasn't a directory.
        canonicalized_path: Utf8PathBuf,
    },
}

impl PathMapperConstructError {
    /// The kind of directory.
    pub fn kind(&self) -> PathMapperConstructKind {
        match self {
            Self::Canonicalization { kind, .. }
            | Self::NonUtf8Path { kind, .. }
            | Self::NotADirectory { kind, .. } => *kind,
        }
    }

    /// The input path that failed.
    pub fn input(&self) -> &Utf8Path {
        match self {
            Self::Canonicalization { input, .. }
            | Self::NonUtf8Path { input, .. }
            | Self::NotADirectory { input, .. } => input,
        }
    }
}

/// The kind of directory that failed to be read in
/// [`PathMapper::new`](crate::reuse_build::PathMapper::new).
///
/// Returned as part of [`PathMapperConstructError`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PathMapperConstructKind {
    /// The workspace root.
    WorkspaceRoot,

    /// The target directory.
    TargetDir,
}

impl fmt::Display for PathMapperConstructKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::WorkspaceRoot => write!(f, "remapped workspace root"),
            Self::TargetDir => write!(f, "remapped target directory"),
        }
    }
}

/// An error that occurs while parsing Rust build metadata from a summary.
#[derive(Debug, Error)]
pub enum RustBuildMetaParseError {
    /// An error occurred while deserializing the platform.
    #[error("error deserializing platform from build metadata")]
    PlatformDeserializeError(#[from] target_spec::Error),

    /// The host platform could not be determined.
    #[error("the host platform could not be determined")]
    DetectBuildTargetError(#[source] target_spec::Error),

    /// The build metadata includes features unsupported.
    #[error("unsupported features in the build metadata: {message}")]
    Unsupported {
        /// The detailed error message.
        message: String,
    },
}

/// Error returned when a user-supplied format version fails to be parsed to a
/// valid and supported version.
#[derive(Clone, Debug, thiserror::Error)]
#[error("invalid format version: {input}")]
pub struct FormatVersionError {
    /// The input that failed to parse.
    pub input: String,
    /// The underlying error.
    #[source]
    pub error: FormatVersionErrorInner,
}

/// The different errors that can occur when parsing and validating a format version.
#[derive(Clone, Debug, thiserror::Error)]
pub enum FormatVersionErrorInner {
    /// The input did not have a valid syntax.
    #[error("expected format version in form of `{expected}`")]
    InvalidFormat {
        /// The expected pseudo format.
        expected: &'static str,
    },
    /// A decimal integer was expected but could not be parsed.
    #[error("version component `{which}` could not be parsed as an integer")]
    InvalidInteger {
        /// Which component was invalid.
        which: &'static str,
        /// The parse failure.
        #[source]
        err: std::num::ParseIntError,
    },
    /// The version component was not within the expected range.
    #[error("version component `{which}` value {value} is out of range {range:?}")]
    InvalidValue {
        /// The component which was out of range.
        which: &'static str,
        /// The value that was parsed.
        value: u8,
        /// The range of valid values for the component.
        range: std::ops::Range<u8>,
    },
}

/// An error that occurs in [`BinaryList::from_messages`](crate::list::BinaryList::from_messages) or
/// [`RustTestArtifact::from_binary_list`](crate::list::RustTestArtifact::from_binary_list).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FromMessagesError {
    /// An error occurred while reading Cargo's JSON messages.
    #[error("error reading Cargo JSON messages")]
    ReadMessages(#[source] std::io::Error),

    /// An error occurred while querying the package graph.
    #[error("error querying package graph")]
    PackageGraph(#[source] guppy::Error),

    /// A target in the package graph was missing `kind` information.
    #[error("missing kind for target {binary_name} in package {package_name}")]
    MissingTargetKind {
        /// The name of the malformed package.
        package_name: String,
        /// The name of the malformed target within the package.
        binary_name: String,
    },
}

/// An error that occurs while parsing test list output.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CreateTestListError {
    /// The proposed cwd for a process is not a directory.
    #[error(
        "for `{binary_id}`, current directory `{cwd}` is not a directory\n\
         (hint: ensure project source is available at this location)"
    )]
    CwdIsNotDir {
        /// The binary ID for which the current directory wasn't found.
        binary_id: RustBinaryId,

        /// The current directory that wasn't found.
        cwd: Utf8PathBuf,
    },

    /// Running a command to gather the list of tests failed to execute.
    #[error(
        "for `{binary_id}`, running command `{}` failed to execute",
        shell_words::join(command)
    )]
    CommandExecFail {
        /// The binary ID for which gathering the list of tests failed.
        binary_id: RustBinaryId,

        /// The command that was run.
        command: Vec<String>,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Running a command to gather the list of tests failed failed with a non-zero exit code.
    #[error(
        "for `{binary_id}`, command `{}` {}\n--- stdout:\n{}\n--- stderr:\n{}\n---",
        shell_words::join(command),
        display_exited_with(*exit_status),
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr),
    )]
    CommandFail {
        /// The binary ID for which gathering the list of tests failed.
        binary_id: RustBinaryId,

        /// The command that was run.
        command: Vec<String>,

        /// The exit status with which the command failed.
        exit_status: ExitStatus,

        /// Standard output for the command.
        stdout: Vec<u8>,

        /// Standard error for the command.
        stderr: Vec<u8>,
    },

    /// Running a command to gather the list of tests produced a non-UTF-8 standard output.
    #[error(
        "for `{binary_id}`, command `{}` produced non-UTF-8 output:\n--- stdout:\n{}\n--- stderr:\n{}\n---",
        shell_words::join(command),
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )]
    CommandNonUtf8 {
        /// The binary ID for which gathering the list of tests failed.
        binary_id: RustBinaryId,

        /// The command that was run.
        command: Vec<String>,

        /// Standard output for the command.
        stdout: Vec<u8>,

        /// Standard error for the command.
        stderr: Vec<u8>,
    },

    /// An error occurred while parsing a line in the test output.
    #[error("for `{binary_id}`, {message}\nfull output:\n{full_output}")]
    ParseLine {
        /// The binary ID for which parsing the list of tests failed.
        binary_id: RustBinaryId,

        /// A descriptive message.
        message: Cow<'static, str>,

        /// The full output.
        full_output: String,
    },

    /// An error occurred while joining paths for dynamic libraries.
    #[error(
        "error joining dynamic library paths for {}: [{}]",
        dylib_path_envvar(),
        itertools::join(.new_paths, ", ")
    )]
    DylibJoinPaths {
        /// New paths attempted to be added to the dynamic library environment variable.
        new_paths: Vec<Utf8PathBuf>,

        /// The underlying error.
        #[source]
        error: JoinPathsError,
    },

    /// Creating a Tokio runtime failed.
    #[error("error creating Tokio runtime")]
    TokioRuntimeCreate(#[source] std::io::Error),
}

impl CreateTestListError {
    pub(crate) fn parse_line(
        binary_id: RustBinaryId,
        message: impl Into<Cow<'static, str>>,
        full_output: impl Into<String>,
    ) -> Self {
        Self::ParseLine {
            binary_id,
            message: message.into(),
            full_output: full_output.into(),
        }
    }

    pub(crate) fn dylib_join_paths(new_paths: Vec<Utf8PathBuf>, error: JoinPathsError) -> Self {
        Self::DylibJoinPaths { new_paths, error }
    }
}

/// An error that occurs while writing list output.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteTestListError {
    /// An error occurred while writing the list to the provided output.
    #[error("error writing to output")]
    Io(#[source] std::io::Error),

    /// An error occurred while serializing JSON, or while writing it to the provided output.
    #[error("error serializing to JSON")]
    Json(#[source] serde_json::Error),
}

/// An error occurred while configuring handles.
///
/// Only relevant on Windows.
#[derive(Debug, Error)]
pub enum ConfigureHandleInheritanceError {
    /// An error occurred. This can only happen on Windows.
    #[cfg(windows)]
    #[error("error configuring handle inheritance")]
    WindowsError(#[from] std::io::Error),
}

/// An error that occurs while building the test runner.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TestRunnerBuildError {
    /// An error occurred while creating the Tokio runtime.
    #[error("error creating Tokio runtime")]
    TokioRuntimeCreate(#[source] std::io::Error),

    /// An error occurred while setting up signals.
    #[error("error setting up signals")]
    SignalHandlerSetupError(#[from] SignalHandlerSetupError),
}

/// Errors that occurred while managing test runner Tokio tasks.
#[derive(Debug, Error)]
pub struct TestRunnerExecuteErrors<E> {
    /// An error that occurred while reporting results to the reporter callback.
    pub report_error: Option<E>,

    /// Join errors (typically panics) that occurred while running the test
    /// runner.
    pub join_errors: Vec<tokio::task::JoinError>,
}

impl<E: std::error::Error> fmt::Display for TestRunnerExecuteErrors<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(report_error) = &self.report_error {
            write!(f, "error reporting results: {report_error}")?;
        }

        if !self.join_errors.is_empty() {
            if self.report_error.is_some() {
                write!(f, "; ")?;
            }

            write!(f, "errors joining tasks: ")?;

            for (i, join_error) in self.join_errors.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }

                write!(f, "{join_error}")?;
            }
        }

        Ok(())
    }
}

/// Represents an unknown archive format.
///
/// Returned by [`ArchiveFormat::autodetect`].
#[derive(Debug, Error)]
#[error(
    "could not detect archive format from file name `{file_name}` (supported extensions: {})",
    supported_extensions()
)]
pub struct UnknownArchiveFormat {
    /// The name of the archive file without any leading components.
    pub file_name: String,
}

fn supported_extensions() -> String {
    ArchiveFormat::SUPPORTED_FORMATS
        .iter()
        .map(|(extension, _)| *extension)
        .join(", ")
}

/// An error that occurs while archiving data.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArchiveCreateError {
    /// An error occurred while creating the binary list to be written.
    #[error("error creating binary list")]
    CreateBinaryList(#[source] WriteTestListError),

    /// An extra path was missing.
    #[error("extra path `{}` not found", .redactor.redact_path(path))]
    MissingExtraPath {
        /// The path that was missing.
        path: Utf8PathBuf,

        /// A redactor for the path.
        ///
        /// (This should eventually move to being a field for a wrapper struct, but it's okay for
        /// now.)
        redactor: Redactor,
    },

    /// An error occurred while reading data from a file on disk.
    #[error("while archiving {step}, error writing {} `{path}` to archive", kind_str(*.is_dir))]
    InputFileRead {
        /// The step that the archive errored at.
        step: ArchiveStep,

        /// The name of the file that could not be read.
        path: Utf8PathBuf,

        /// Whether this is a directory. `None` means the status was unknown.
        is_dir: Option<bool>,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while reading entries from a directory on disk.
    #[error("error reading directory entry from `{path}")]
    DirEntryRead {
        /// The name of the directory from which entries couldn't be read.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while writing data to the output file.
    #[error("error writing to archive")]
    OutputArchiveIo(#[source] std::io::Error),

    /// An error occurred in the reporter.
    #[error("error reporting archive status")]
    ReporterIo(#[source] std::io::Error),
}

fn kind_str(is_dir: Option<bool>) -> &'static str {
    match is_dir {
        Some(true) => "directory",
        Some(false) => "file",
        None => "path",
    }
}

/// An error occurred while materializing a metadata path.
#[derive(Debug, Error)]
pub enum MetadataMaterializeError {
    /// An I/O error occurred while reading the metadata file.
    #[error("I/O error reading metadata file `{path}`")]
    Read {
        /// The path that was being read.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// A JSON deserialization error occurred while reading the metadata file.
    #[error("error deserializing metadata file `{path}`")]
    Deserialize {
        /// The path that was being read.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while parsing Rust build metadata.
    #[error("error parsing Rust build metadata from `{path}`")]
    RustBuildMeta {
        /// The path that was deserialized.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: RustBuildMetaParseError,
    },

    /// An error occurred converting data into a `PackageGraph`.
    #[error("error building package graph from `{path}`")]
    PackageGraphConstruct {
        /// The path that was deserialized.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: guppy::Error,
    },
}

/// An error occurred while reading a file.
///
/// Returned as part of both [`ArchiveCreateError`] and [`ArchiveExtractError`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArchiveReadError {
    /// An I/O error occurred while reading the archive.
    #[error("I/O error reading archive")]
    Io(#[source] std::io::Error),

    /// A path wasn't valid UTF-8.
    #[error("path in archive `{}` wasn't valid UTF-8", String::from_utf8_lossy(.0))]
    NonUtf8Path(Vec<u8>),

    /// A file path within the archive didn't begin with "target/".
    #[error("path in archive `{0}` doesn't start with `target/`")]
    NoTargetPrefix(Utf8PathBuf),

    /// A file path within the archive had an invalid component within it.
    #[error("path in archive `{path}` contains an invalid component `{component}`")]
    InvalidComponent {
        /// The path that had an invalid component.
        path: Utf8PathBuf,

        /// The invalid component.
        component: String,
    },

    /// An error occurred while reading a checksum.
    #[error("corrupted archive: checksum read error for path `{path}`")]
    ChecksumRead {
        /// The path for which there was a checksum read error.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// An entry had an invalid checksum.
    #[error("corrupted archive: invalid checksum for path `{path}`")]
    InvalidChecksum {
        /// The path that had an invalid checksum.
        path: Utf8PathBuf,

        /// The expected checksum.
        expected: u32,

        /// The actual checksum.
        actual: u32,
    },

    /// A metadata file wasn't found.
    #[error("metadata file `{0}` not found in archive")]
    MetadataFileNotFound(&'static Utf8Path),

    /// An error occurred while deserializing a metadata file.
    #[error("error deserializing metadata file `{path}` in archive")]
    MetadataDeserializeError {
        /// The name of the metadata file.
        path: &'static Utf8Path,

        /// The deserialize error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while building a `PackageGraph`.
    #[error("error building package graph from `{path}` in archive")]
    PackageGraphConstructError {
        /// The name of the metadata file.
        path: &'static Utf8Path,

        /// The error.
        #[source]
        error: guppy::Error,
    },
}

/// An error occurred while extracting a file.
///
/// Returned by [`extract_archive`](crate::reuse_build::ReuseBuildInfo::extract_archive).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArchiveExtractError {
    /// An error occurred while creating a temporary directory.
    #[error("error creating temporary directory")]
    TempDirCreate(#[source] std::io::Error),

    /// An error occurred while canonicalizing the destination directory.
    #[error("error canonicalizing destination directory `{dir}`")]
    DestDirCanonicalization {
        /// The directory that failed to canonicalize.
        dir: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// The destination already exists and `--overwrite` was not passed in.
    #[error("destination `{0}` already exists")]
    DestinationExists(Utf8PathBuf),

    /// An error occurred while reading the archive.
    #[error("error reading archive")]
    Read(#[source] ArchiveReadError),

    /// An error occurred while deserializing Rust build metadata.
    #[error("error deserializing Rust build metadata")]
    RustBuildMeta(#[from] RustBuildMetaParseError),

    /// An error occurred while writing out a file to the destination directory.
    #[error("error writing file `{path}` to disk")]
    WriteFile {
        /// The path that we couldn't write out.
        path: Utf8PathBuf,

        /// The error that occurred.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while reporting the extraction status.
    #[error("error reporting extract status")]
    ReporterIo(std::io::Error),
}

/// An error that occurs while writing an event.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteEventError {
    /// An error occurred while writing the event to the provided output.
    #[error("error writing to output")]
    Io(#[source] std::io::Error),

    /// An error occurred while operating on the file system.
    #[error("error operating on path {file}")]
    Fs {
        /// The file being operated on.
        file: Utf8PathBuf,

        /// The underlying IO error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while producing JUnit XML.
    #[error("error writing JUnit output to {file}")]
    Junit {
        /// The output file.
        file: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: quick_junit::SerializeError,
    },
}

/// An error occurred while constructing a [`CargoConfigs`](crate::cargo_config::CargoConfigs)
/// instance.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CargoConfigError {
    /// Failed to retrieve the current directory.
    #[error("failed to retrieve current directory")]
    GetCurrentDir(#[source] std::io::Error),

    /// The current directory was invalid UTF-8.
    #[error("current directory is invalid UTF-8")]
    CurrentDirInvalidUtf8(#[source] FromPathBufError),

    /// Parsing a CLI config option failed.
    #[error("failed to parse --config argument `{config_str}` as TOML")]
    CliConfigParseError {
        /// The CLI config option.
        config_str: String,

        /// The error that occurred trying to parse the config.
        #[source]
        error: toml_edit::TomlError,
    },

    /// Deserializing a CLI config option into domain types failed.
    #[error("failed to deserialize --config argument `{config_str}` as TOML")]
    CliConfigDeError {
        /// The CLI config option.
        config_str: String,

        /// The error that occurred trying to deserialize the config.
        #[source]
        error: toml_edit::de::Error,
    },

    /// A CLI config option is not in the dotted key format.
    #[error(
        "invalid format for --config argument `{config_str}` (should be a dotted key expression)"
    )]
    InvalidCliConfig {
        /// The CLI config option.
        config_str: String,

        /// The reason why this Cargo CLI config is invalid.
        #[source]
        reason: InvalidCargoCliConfigReason,
    },

    /// A non-UTF-8 path was encountered.
    #[error("non-UTF-8 path encountered")]
    NonUtf8Path(#[source] FromPathBufError),

    /// Failed to retrieve the Cargo home directory.
    #[error("failed to retrieve the Cargo home directory")]
    GetCargoHome(#[source] std::io::Error),

    /// Failed to canonicalize a path
    #[error("failed to canonicalize path `{path}")]
    FailedPathCanonicalization {
        /// The path that failed to canonicalize
        path: Utf8PathBuf,

        /// The error the occurred during canonicalization
        #[source]
        error: std::io::Error,
    },

    /// Failed to read config file
    #[error("failed to read config at `{path}`")]
    ConfigReadError {
        /// The path of the config file
        path: Utf8PathBuf,

        /// The error that occurred trying to read the config file
        #[source]
        error: std::io::Error,
    },

    /// Failed to deserialize config file
    #[error(transparent)]
    ConfigParseError(#[from] Box<CargoConfigParseError>),
}

/// Failed to deserialize config file
///
/// We introduce this extra indirection, because of the `clippy::result_large_err` rule on Windows.
#[derive(Debug, Error)]
#[error("failed to parse config at `{path}`")]
pub struct CargoConfigParseError {
    /// The path of the config file
    pub path: Utf8PathBuf,

    /// The error that occurred trying to deserialize the config file
    #[source]
    pub error: toml::de::Error,
}

/// The reason an invalid CLI config failed.
///
/// Part of [`CargoConfigError::InvalidCliConfig`].
#[derive(Copy, Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum InvalidCargoCliConfigReason {
    /// The argument is not a TOML dotted key expression.
    #[error("was not a TOML dotted key expression (such as `build.jobs = 2`)")]
    NotDottedKv,

    /// The argument includes non-whitespace decoration.
    #[error("includes non-whitespace decoration")]
    IncludesNonWhitespaceDecoration,

    /// The argument sets a value to an inline table.
    #[error("sets a value to an inline table, which is not accepted")]
    SetsValueToInlineTable,

    /// The argument sets a value to an array of tables.
    #[error("sets a value to an array of tables, which is not accepted")]
    SetsValueToArrayOfTables,

    /// The argument doesn't provide a value.
    #[error("doesn't provide a value")]
    DoesntProvideValue,
}

/// The host platform couldn't be detected.
#[derive(Debug, Error)]
pub enum HostPlatformDetectError {
    /// Spawning `rustc -vV` failed, and detecting the build target failed as
    /// well.
    #[error(
        "error spawning `rustc -vV`, and detecting the build \
         target failed as well\n\
         - rustc spawn error: {}\n\
         - build target error: {}\n",
        DisplayErrorChain::new_with_initial_indent("  ", error),
        DisplayErrorChain::new_with_initial_indent("  ", build_target_error)
    )]
    RustcVvSpawnError {
        /// The error.
        error: std::io::Error,

        /// The error that occurred while detecting the build target.
        build_target_error: Box<target_spec::Error>,
    },

    /// `rustc -vV` exited with a non-zero code, and detecting the build target
    /// failed as well.
    #[error(
        "`rustc -vV` failed with {}, and detecting the \
         build target failed as well\n\
         - `rustc -vV` stdout:\n{}\n\
         - `rustc -vV` stderr:\n{}\n\
         - build target error:\n{}\n",
        status,
        DisplayIndented { item: String::from_utf8_lossy(stdout), indent: "  " },
        DisplayIndented { item: String::from_utf8_lossy(stderr), indent: "  " },
        DisplayErrorChain::new_with_initial_indent("  ", build_target_error)
    )]
    RustcVvFailed {
        /// The status.
        status: ExitStatus,

        /// The standard output from `rustc -vV`.
        stdout: Vec<u8>,

        /// The standard error from `rustc -vV`.
        stderr: Vec<u8>,

        /// The error that occurred while detecting the build target.
        build_target_error: Box<target_spec::Error>,
    },

    /// Parsing the host platform failed, and detecting the build target failed
    /// as well.
    #[error(
        "parsing `rustc -vV` output failed, and detecting the build target \
         failed as well\n\
         - host platform error:\n{}\n\
         - build target error:\n{}\n",
        DisplayErrorChain::new_with_initial_indent("  ", host_platform_error),
        DisplayErrorChain::new_with_initial_indent("  ", build_target_error)
    )]
    HostPlatformParseError {
        /// The error that occurred while parsing the host platform.
        host_platform_error: Box<target_spec::Error>,

        /// The error that occurred while detecting the build target.
        build_target_error: Box<target_spec::Error>,
    },

    /// Test-only code: `rustc -vV` was not queried, and detecting the build
    /// target failed as well.
    #[error("test-only code, so `rustc -vV` was not called; failed to detect build target")]
    BuildTargetError {
        /// The error that occurred while detecting the build target.
        #[source]
        build_target_error: Box<target_spec::Error>,
    },
}

/// An error occurred while determining the cross-compiling target triple.
#[derive(Debug, Error)]
pub enum TargetTripleError {
    /// The environment variable contained non-utf8 content
    #[error(
        "environment variable '{}' contained non-UTF-8 data",
        TargetTriple::CARGO_BUILD_TARGET_ENV
    )]
    InvalidEnvironmentVar,

    /// An error occurred while deserializing the platform.
    #[error("error deserializing target triple from {source}")]
    TargetSpecError {
        /// The source from which the triple couldn't be parsed.
        source: TargetTripleSource,

        /// The error that occurred parsing the triple.
        #[source]
        error: target_spec::Error,
    },

    /// For a custom platform, reading the target path failed.
    #[error("target path `{path}` is not a valid file")]
    TargetPathReadError {
        /// The source from which the triple couldn't be parsed.
        source: TargetTripleSource,

        /// The path that we tried to read.
        path: Utf8PathBuf,

        /// The error that occurred parsing the triple.
        #[source]
        error: std::io::Error,
    },

    /// Failed to create a temporary directory for a custom platform.
    #[error(
        "for custom platform obtained from {source}, \
         failed to create temporary directory for custom platform"
    )]
    CustomPlatformTempDirError {
        /// The source of the target triple.
        source: TargetTripleSource,

        /// The error that occurred during the create.
        #[source]
        error: std::io::Error,
    },

    /// Failed to write a custom platform to disk.
    #[error(
        "for custom platform obtained from {source}, \
         failed to write JSON to temporary path `{path}`"
    )]
    CustomPlatformWriteError {
        /// The source of the target triple.
        source: TargetTripleSource,

        /// The path that we tried to write to.
        path: Utf8PathBuf,

        /// The error that occurred during the write.
        #[source]
        error: std::io::Error,
    },

    /// Failed to close a temporary directory for an extracted custom platform.
    #[error(
        "for custom platform obtained from {source}, \
         failed to close temporary directory `{dir_path}`"
    )]
    CustomPlatformCloseError {
        /// The source of the target triple.
        source: TargetTripleSource,

        /// The directory that we tried to delete.
        dir_path: Utf8PathBuf,

        /// The error that occurred during the close.
        #[source]
        error: std::io::Error,
    },
}

impl TargetTripleError {
    /// Returns a [`miette::Report`] for the source, if available.
    ///
    /// This should be preferred over [`std::error::Error::source`] if
    /// available.
    pub fn source_report(&self) -> Option<miette::Report> {
        match self {
            Self::TargetSpecError { error, .. } => {
                Some(miette::Report::new_boxed(error.clone().into_diagnostic()))
            }
            // The remaining types are covered via the error source path.
            TargetTripleError::InvalidEnvironmentVar
            | TargetTripleError::TargetPathReadError { .. }
            | TargetTripleError::CustomPlatformTempDirError { .. }
            | TargetTripleError::CustomPlatformWriteError { .. }
            | TargetTripleError::CustomPlatformCloseError { .. } => None,
        }
    }
}

/// An error occurred determining the target runner
#[derive(Debug, Error)]
pub enum TargetRunnerError {
    /// An environment variable contained non-utf8 content
    #[error("environment variable '{0}' contained non-UTF-8 data")]
    InvalidEnvironmentVar(String),

    /// An environment variable or config key was found that matches the target
    /// triple, but it didn't actually contain a binary
    #[error("runner '{key}' = '{value}' did not contain a runner binary")]
    BinaryNotSpecified {
        /// The source under consideration.
        key: PlatformRunnerSource,

        /// The value that was read from the key
        value: String,
    },
}

/// An error that occurred while setting up the signal handler.
#[derive(Debug, Error)]
#[error("error setting up signal handler")]
pub struct SignalHandlerSetupError(#[from] std::io::Error);

/// An error occurred while showing test groups.
#[derive(Debug, Error)]
pub enum ShowTestGroupsError {
    /// Unknown test groups were specified.
    #[error(
        "unknown test groups specified: {}\n(known groups: {})",
        unknown_groups.iter().join(", "),
        known_groups.iter().join(", "),
    )]
    UnknownGroups {
        /// The unknown test groups.
        unknown_groups: BTreeSet<TestGroup>,

        /// All known test groups.
        known_groups: BTreeSet<TestGroup>,
    },
}

/// An error occurred while processing profile's inherits setting
#[derive(Debug, Error, PartialEq, Eq, Hash)]
pub enum InheritsError {
    /// The default profile should not be able to inherit from other profiles
    #[error("the {} profile should not inherit from other profiles", .0)]
    DefaultProfileInheritance(String),
    /// An unknown/unfound profile was detected to inherit from in profile configuration
    #[error("profile {} inherits from an unknown profile {}", .0, .1)]
    UnknownInheritance(String, String),
    /// A self referential inheritance is detected from this profile
    #[error("a self referential inheritance is detected from profile: {}", .0)]
    SelfReferentialInheritance(String),
    /// An inheritance cycle was detected in the profile configuration.
    #[error("inheritance cycle detected in profile configuration from: {}", .0.iter().map(|scc| {
        format!("[{}]", scc.iter().join(", "))
    }).join(", "))]
    InheritanceCycle(Vec<Vec<String>>),
}

// ---
// Record and replay errors
// ---

/// An error that occurred while managing the run store.
#[derive(Debug, Error)]
pub enum RunStoreError {
    /// An error occurred while creating the run directory.
    #[error("error creating run directory `{run_dir}`")]
    RunDirCreate {
        /// The run directory that could not be created.
        run_dir: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while acquiring a file lock.
    #[error("error acquiring lock on `{path}`")]
    FileLock {
        /// The path to the lock file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Timed out waiting to acquire a file lock.
    #[error(
        "timed out acquiring lock on `{path}` after {timeout_secs}s (is the cache directory \
         on a networked filesystem?)"
    )]
    FileLockTimeout {
        /// The path to the lock file.
        path: Utf8PathBuf,

        /// The timeout duration in seconds.
        timeout_secs: u64,
    },

    /// An error occurred while reading the run list.
    #[error("error reading run list from `{path}`")]
    RunListRead {
        /// The path to the run list file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while deserializing the run list.
    #[error("error deserializing run list from `{path}`")]
    RunListDeserialize {
        /// The path to the run list file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while serializing the run list.
    #[error("error serializing run list to `{path}`")]
    RunListSerialize {
        /// The path to the run list file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while serializing rerun info.
    #[error("error serializing rerun info")]
    RerunInfoSerialize {
        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while serializing the test list.
    #[error("error serializing test list")]
    TestListSerialize {
        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while serializing the record options.
    #[error("error serializing record options")]
    RecordOptionsSerialize {
        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while serializing a test event.
    #[error("error serializing test event")]
    TestEventSerialize {
        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// An error occurred while writing the run list.
    #[error("error writing run list to `{path}`")]
    RunListWrite {
        /// The path to the run list file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: atomicwrites::Error<std::io::Error>,
    },

    /// An error occurred while writing to the store.
    #[error("error writing to store at `{store_path}`")]
    StoreWrite {
        /// The path to the store file.
        store_path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: StoreWriterError,
    },

    /// An error occurred while creating the run log.
    #[error("error creating run log at `{path}`")]
    RunLogCreate {
        /// The path to the run log file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while writing to the run log.
    #[error("error writing to run log at `{path}`")]
    RunLogWrite {
        /// The path to the run log file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while flushing the run log.
    #[error("error flushing run log at `{path}`")]
    RunLogFlush {
        /// The path to the run log file.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Cannot write to runs.json.zst because it has a newer format version.
    #[error(
        "cannot write to record store: runs.json.zst format version {file_version} is newer than \
         supported version {max_supported_version}"
    )]
    FormatVersionTooNew {
        /// The format version in the file.
        file_version: u32,
        /// The maximum version this nextest can write.
        max_supported_version: u32,
    },
}

/// An error that occurred while writing to a zip store.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreWriterError {
    /// An error occurred while creating the store file.
    #[error("error creating store")]
    Create {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while starting a new file in the store.
    #[error("error creating path `{path}` in store")]
    StartFile {
        /// The path within the store.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: zip::result::ZipError,
    },

    /// An error occurred while writing to a file in the store.
    #[error("error writing to path `{path}` in store")]
    Write {
        /// The path within the store.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while compressing data with a zstd dictionary.
    #[error("error compressing data")]
    Compress {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while finalizing the store.
    #[error("error finalizing store")]
    Finish {
        /// The underlying error.
        #[source]
        error: zip::result::ZipError,
    },

    /// An error occurred while flushing the store.
    #[error("error flushing store")]
    Flush {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },
}

/// An error that occurred in the record reporter.
#[derive(Debug, Error)]
pub enum RecordReporterError {
    /// An error occurred while writing to the run store.
    #[error(transparent)]
    RunStore(RunStoreError),

    /// The writer thread panicked.
    #[error("record writer thread panicked: {message}")]
    WriterPanic {
        /// A message extracted from the panic payload, if available.
        message: String,
    },
}

/// An error determining the cache directory for recordings.
#[derive(Debug, Error)]
pub enum CacheDirError {
    /// The platform base strategy could not be determined.
    ///
    /// This typically means the platform doesn't support standard directory layouts.
    #[error("could not determine platform base directory strategy")]
    BaseDirStrategy,

    /// The platform cache directory path is not valid UTF-8.
    #[error("platform cache directory is not valid UTF-8: {path:?}")]
    CacheDirNotUtf8 {
        /// The path that was not valid UTF-8.
        path: PathBuf,
    },

    /// The workspace path could not be canonicalized.
    #[error("could not canonicalize workspace path `{workspace_root}`")]
    Canonicalize {
        /// The workspace root that could not be canonicalized.
        workspace_root: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        error: std::io::Error,
    },
}

/// An error during recording session setup.
#[derive(Debug, Error)]
pub enum RecordSetupError {
    /// The platform cache directory could not be determined.
    #[error("could not determine platform cache directory for recording")]
    CacheDirNotFound(#[source] CacheDirError),

    /// Failed to create the run store.
    #[error("failed to create run store")]
    StoreCreate(#[source] RunStoreError),

    /// Failed to acquire exclusive lock on the run store.
    #[error("failed to lock run store")]
    StoreLock(#[source] RunStoreError),

    /// Failed to create the run recorder.
    #[error("failed to create run recorder")]
    RecorderCreate(#[source] RunStoreError),
}

impl RecordSetupError {
    /// If this error indicates that recording should be disabled (but the test
    /// run should continue), returns the underlying error.
    ///
    /// This is the case when the runs.json.zst file has a newer format version than
    /// this nextest supports. Recording is disabled to avoid data loss, but the
    /// test run can proceed normally.
    pub fn disabled_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RecordSetupError::RecorderCreate(err @ RunStoreError::FormatVersionTooNew { .. }) => {
                Some(err)
            }
            _ => None,
        }
    }
}

/// An error that occurred while pruning recorded runs.
#[derive(Debug, Error)]
pub enum RecordPruneError {
    /// An error occurred while deleting a run directory.
    #[error("error deleting run `{run_id}` at `{path}`")]
    DeleteRun {
        /// The run ID that could not be deleted.
        run_id: ReportUuid,

        /// The path to the run directory.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while calculating the size of a path.
    #[error("error calculating size of `{path}`")]
    CalculateSize {
        /// The path whose size could not be calculated.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while deleting an orphaned directory.
    #[error("error deleting orphaned directory `{path}`")]
    DeleteOrphan {
        /// The path to the orphaned directory.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while reading the runs directory.
    #[error("error reading runs directory `{path}`")]
    ReadRunsDir {
        /// The path to the runs directory.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while reading a directory entry.
    #[error("error reading directory entry in `{dir}`")]
    ReadDirEntry {
        /// The path to the directory being read.
        dir: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// An error occurred while reading file type.
    #[error("error reading file type for `{path}`")]
    ReadFileType {
        /// The path whose file type could not be read.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },
}

/// Error returned when parsing an invalid run ID selector.
///
/// A valid selector is either "latest" or a string containing only hex digits
/// and dashes (for UUID format).
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("invalid run ID selector `{input}`: expected `latest` or hex digits")]
pub struct InvalidRunIdSelector {
    /// The invalid input string.
    pub input: String,
}

/// An error resolving a run ID prefix.
#[derive(Debug, Error)]
pub enum RunIdResolutionError {
    /// No run found matching the prefix.
    #[error("no recorded run found matching `{prefix}`")]
    NotFound {
        /// The prefix that was searched for.
        prefix: String,
    },

    /// Multiple runs match the prefix.
    #[error("prefix `{prefix}` is ambiguous, matches {count} runs")]
    Ambiguous {
        /// The prefix that was searched for.
        prefix: String,

        /// The number of matching runs.
        count: usize,

        /// The candidates that matched (up to a limit).
        candidates: Vec<RecordedRunInfo>,

        /// The run ID index for computing shortest unique prefixes.
        run_id_index: RunIdIndex,
    },

    /// The prefix contains invalid characters.
    #[error("prefix `{prefix}` contains invalid characters (expected hexadecimal)")]
    InvalidPrefix {
        /// The invalid prefix.
        prefix: String,
    },

    /// No recorded runs exist.
    #[error("no recorded runs exist")]
    NoRuns,
}

/// An error that occurred while reading a recorded run.
#[derive(Debug, Error)]
pub enum RecordReadError {
    /// The run was not found.
    #[error("run not found at `{path}`")]
    RunNotFound {
        /// The path where the run was expected.
        path: Utf8PathBuf,
    },

    /// Failed to open the archive.
    #[error("error opening archive at `{path}`")]
    OpenArchive {
        /// The path to the archive.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Failed to read an archive file.
    #[error("error reading `{file_name}` from archive")]
    ReadArchiveFile {
        /// The file name within the archive.
        file_name: String,

        /// The underlying error.
        #[source]
        error: zip::result::ZipError,
    },

    /// Failed to open the run log.
    #[error("error opening run log at `{path}`")]
    OpenRunLog {
        /// The path to the run log.
        path: Utf8PathBuf,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Failed to read a line from the run log.
    #[error("error reading line {line_number} from run log")]
    ReadRunLog {
        /// The line number that failed.
        line_number: usize,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Failed to parse an event from the run log.
    #[error("error parsing event at line {line_number}")]
    ParseEvent {
        /// The line number that failed.
        line_number: usize,

        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },

    /// Required file not found in archive.
    #[error("required file `{file_name}` not found in archive")]
    FileNotFound {
        /// The file name that was not found.
        file_name: String,
    },

    /// Failed to decompress archive data.
    #[error("error decompressing data from `{file_name}`")]
    Decompress {
        /// The file name being decompressed.
        file_name: String,

        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// Unknown output file type in archive.
    ///
    /// This indicates the archive was created by a newer version of nextest
    /// with additional output types that this version doesn't recognize.
    #[error(
        "unknown output file type `{file_name}` in archive \
         (archive may have been created by a newer version of nextest)"
    )]
    UnknownOutputType {
        /// The file name with the unknown type.
        file_name: String,
    },

    /// Archive file exceeds maximum allowed size.
    #[error(
        "file `{file_name}` in archive exceeds maximum size ({size} bytes, limit is {limit} bytes)"
    )]
    FileTooLarge {
        /// The file name in the archive.
        file_name: String,

        /// The size of the file.
        size: u64,

        /// The maximum allowed size.
        limit: u64,
    },

    /// Archive file size doesn't match the header.
    ///
    /// This indicates a corrupt or tampered archive since nextest controls
    /// archive creation.
    #[error(
        "file `{file_name}` size mismatch: header claims {claimed_size} bytes, \
         but read {actual_size} bytes (archive may be corrupt or tampered)"
    )]
    SizeMismatch {
        /// The file name in the archive.
        file_name: String,

        /// The size claimed in the ZIP header.
        claimed_size: u64,

        /// The actual size read.
        actual_size: u64,
    },

    /// Failed to deserialize metadata.
    #[error("error deserializing `{file_name}`")]
    DeserializeMetadata {
        /// The file name being deserialized.
        file_name: String,

        /// The underlying error.
        #[source]
        error: serde_json::Error,
    },
}

/// An error that occurred while reconstructing a TestList from a summary.
///
/// Returned by [`TestList::from_summary`](crate::list::TestList::from_summary).
#[derive(Debug, Error)]
pub enum TestListFromSummaryError {
    /// Package not found in the package graph.
    #[error("package `{name}` (id: `{package_id}`) not found in cargo metadata")]
    PackageNotFound {
        /// The package name.
        name: String,

        /// The package ID that was looked up.
        package_id: String,
    },

    /// Error parsing rust build metadata.
    #[error("error parsing rust build metadata")]
    RustBuildMeta(#[source] RustBuildMetaParseError),
}

#[cfg(feature = "self-update")]
mod self_update_errors {
    use super::*;
    use crate::update::PrereleaseKind;
    use mukti_metadata::ReleaseStatus;
    use semver::{Version, VersionReq};

    /// An error that occurs while performing a self-update.
    ///
    /// Returned by methods in the [`update`](crate::update) module.
    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum UpdateError {
        /// Failed to read release metadata from a local path on disk.
        #[error("failed to read release metadata from `{path}`")]
        ReadLocalMetadata {
            /// The path that was read.
            path: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// An error was generated by `self_update`.
        #[error("self-update failed")]
        SelfUpdate(#[source] self_update::errors::Error),

        /// Deserializing release metadata failed.
        #[error("deserializing release metadata failed")]
        ReleaseMetadataDe(#[source] serde_json::Error),

        /// This version was not found.
        #[error("version `{version}` not found (known versions: {})", known_versions(.known))]
        VersionNotFound {
            /// The version that wasn't found.
            version: Version,

            /// A list of all known versions.
            known: Vec<(Version, ReleaseStatus)>,
        },

        /// No version was found matching a requirement.
        #[error("no version found matching requirement `{req}`")]
        NoMatchForVersionReq {
            /// The version requirement that had no matches.
            req: VersionReq,
        },

        /// No stable (non-prerelease) version was found.
        #[error("no stable version found")]
        NoStableVersion,

        /// No version matching the requested prerelease kind was found.
        #[error("no version found matching {} channel", kind.description())]
        NoVersionForPrereleaseKind {
            /// The kind of prerelease that was requested.
            kind: PrereleaseKind,
        },

        /// The specified mukti project was not found.
        #[error("project {not_found} not found in release metadata (known projects: {})", known.join(", "))]
        MuktiProjectNotFound {
            /// The project that was not found.
            not_found: String,

            /// Known projects.
            known: Vec<String>,
        },

        /// No release information was found for the given target triple.
        #[error(
            "for version {version}, no release information found for target `{triple}` \
            (known targets: {})",
            known_triples.iter().join(", ")
        )]
        NoTargetData {
            /// The version that was fetched.
            version: Version,

            /// The target triple.
            triple: String,

            /// The triples that were found.
            known_triples: BTreeSet<String>,
        },

        /// The current executable could not be determined.
        #[error("the current executable's path could not be determined")]
        CurrentExe(#[source] std::io::Error),

        /// A temporary directory could not be created.
        #[error("temporary directory could not be created at `{location}`")]
        TempDirCreate {
            /// The location where the temporary directory could not be created.
            location: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// The temporary archive could not be created.
        #[error("temporary archive could not be created at `{archive_path}`")]
        TempArchiveCreate {
            /// The archive file that couldn't be created.
            archive_path: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// An error occurred while writing to a temporary archive.
        #[error("error writing to temporary archive at `{archive_path}`")]
        TempArchiveWrite {
            /// The archive path for which there was an error.
            archive_path: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// An error occurred while reading from a temporary archive.
        #[error("error reading from temporary archive at `{archive_path}`")]
        TempArchiveRead {
            /// The archive path for which there was an error.
            archive_path: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// A checksum mismatch occurred. (Currently, the SHA-256 checksum is checked.)
        #[error("SHA-256 checksum mismatch: expected: {expected}, actual: {actual}")]
        ChecksumMismatch {
            /// The expected checksum.
            expected: String,

            /// The actual checksum.
            actual: String,
        },

        /// An error occurred while renaming a file.
        #[error("error renaming `{source}` to `{dest}`")]
        FsRename {
            /// The rename source.
            source: Utf8PathBuf,

            /// The rename destination.
            dest: Utf8PathBuf,

            /// The error that occurred.
            #[source]
            error: std::io::Error,
        },

        /// An error occurred while running `cargo nextest self setup`.
        #[error("cargo-nextest binary updated, but error running `cargo nextest self setup`")]
        SelfSetup(#[source] std::io::Error),
    }

    fn known_versions(versions: &[(Version, ReleaseStatus)]) -> String {
        use std::fmt::Write;

        // Take the first few versions here.
        const DISPLAY_COUNT: usize = 4;

        let display_versions: Vec<_> = versions
            .iter()
            .filter(|(v, status)| v.pre.is_empty() && *status == ReleaseStatus::Active)
            .map(|(v, _)| v.to_string())
            .take(DISPLAY_COUNT)
            .collect();
        let mut display_str = display_versions.join(", ");
        if versions.len() > display_versions.len() {
            write!(
                display_str,
                " and {} others",
                versions.len() - display_versions.len()
            )
            .unwrap();
        }

        display_str
    }

    /// An error occurred while parsing an [`UpdateVersion`](crate::update::UpdateVersion).
    #[derive(Debug, Error)]
    pub enum UpdateVersionParseError {
        /// The version string is empty.
        #[error("version string is empty")]
        EmptyString,

        /// The input is not a valid version requirement.
        #[error(
            "`{input}` is not a valid semver requirement\n\
                (hint: see https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html for the correct format)"
        )]
        InvalidVersionReq {
            /// The input that was provided.
            input: String,

            /// The error.
            #[source]
            error: semver::Error,
        },

        /// The version is not a valid semver.
        #[error("`{input}` is not a valid semver{}", extra_semver_output(.input))]
        InvalidVersion {
            /// The input that was provided.
            input: String,

            /// The error.
            #[source]
            error: semver::Error,
        },
    }

    fn extra_semver_output(input: &str) -> String {
        // If it is not a valid version but it is a valid version
        // requirement, add a note to the warning
        if input.parse::<VersionReq>().is_ok() {
            format!(
                "\n(if you want to specify a semver range, add an explicit qualifier, like ^{input})"
            )
        } else {
            "".to_owned()
        }
    }
}

#[cfg(feature = "self-update")]
pub use self_update_errors::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_error_chain() {
        let err1 = StringError::new("err1", None);

        insta::assert_snapshot!(format!("{}", DisplayErrorChain::new(&err1)), @"err1");

        let err2 = StringError::new("err2", Some(err1));
        let err3 = StringError::new("err3\nerr3 line 2", Some(err2));

        insta::assert_snapshot!(format!("{}", DisplayErrorChain::new(&err3)), @"
        err3
        err3 line 2
          caused by:
          - err2
          - err1
        ");
    }

    #[test]
    fn display_error_list() {
        let err1 = StringError::new("err1", None);

        let error_list =
            ErrorList::<StringError>::new("waiting on the water to boil", vec![err1.clone()])
                .expect(">= 1 error");
        insta::assert_snapshot!(format!("{}", error_list), @"err1");
        insta::assert_snapshot!(format!("{}", DisplayErrorChain::new(&error_list)), @"err1");

        let err2 = StringError::new("err2", Some(err1));
        let err3 = StringError::new("err3", Some(err2));

        let error_list =
            ErrorList::<StringError>::new("waiting on flowers to bloom", vec![err3.clone()])
                .expect(">= 1 error");
        insta::assert_snapshot!(format!("{}", error_list), @"err3");
        insta::assert_snapshot!(format!("{}", DisplayErrorChain::new(&error_list)), @"
        err3
          caused by:
          - err2
          - err1
        ");

        let err4 = StringError::new("err4", None);
        let err5 = StringError::new("err5", Some(err4));
        let err6 = StringError::new("err6\nerr6 line 2", Some(err5));

        let error_list = ErrorList::<StringError>::new(
            "waiting for the heat death of the universe",
            vec![err3, err6],
        )
        .expect(">= 1 error");

        insta::assert_snapshot!(format!("{}", error_list), @"
        2 errors occurred waiting for the heat death of the universe:
        * err3
            caused by:
            - err2
            - err1
        * err6
          err6 line 2
            caused by:
            - err5
            - err4
        ");
        insta::assert_snapshot!(format!("{}", DisplayErrorChain::new(&error_list)), @"
        2 errors occurred waiting for the heat death of the universe:
        * err3
            caused by:
            - err2
            - err1
        * err6
          err6 line 2
            caused by:
            - err5
            - err4
        ");
    }

    #[derive(Clone, Debug, Error)]
    struct StringError {
        message: String,
        #[source]
        source: Option<Box<StringError>>,
    }

    impl StringError {
        fn new(message: impl Into<String>, source: Option<StringError>) -> Self {
            Self {
                message: message.into(),
                source: source.map(Box::new),
            }
        }
    }

    impl fmt::Display for StringError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }
}
