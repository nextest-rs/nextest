// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    reporter::{StatusLevel, TestOutputDisplay},
    test_filter::RunIgnored,
    test_list::OutputFormat,
};
use camino::Utf8PathBuf;
use config::ConfigError;
use std::{borrow::Cow, error, fmt};

/// An error that occurred while parsing the config.
#[derive(Debug)]
#[non_exhaustive]
pub struct ConfigParseError {
    config_file: Utf8PathBuf,
    err: ConfigError,
}

impl ConfigParseError {
    pub(crate) fn new(config_file: impl Into<Utf8PathBuf>, err: ConfigError) -> Self {
        Self {
            config_file: config_file.into(),
            err,
        }
    }
}

impl fmt::Display for ConfigParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "failed to parse nextest config at `{}`",
            self.config_file
        )?;
        Ok(())
    }
}

impl error::Error for ConfigParseError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.err)
    }
}

#[derive(Clone, Debug)]
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

impl fmt::Display for ProfileNotFound {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "profile '{}' not found (known profiles: {})",
            self.profile,
            self.all_profiles.join(", ")
        )
    }
}

impl error::Error for ProfileNotFound {}

/// Error returned while parsing a [`TestOutputDisplay`] value from a string.
#[derive(Clone, Debug)]
pub struct TestOutputDisplayParseError {
    input: String,
}

impl TestOutputDisplayParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for TestOutputDisplayParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unrecognized value for test output display: {}\n(known values: {})",
            self.input,
            TestOutputDisplay::variants().join(", ")
        )
    }
}

impl error::Error for TestOutputDisplayParseError {}

/// Error returned while parsing a [`StatusLevel`] value from a string.
#[derive(Clone, Debug)]
pub struct StatusLevelParseError {
    input: String,
}

impl StatusLevelParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for StatusLevelParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unrecognized value for status-level: {}\n(known values: {})",
            self.input,
            StatusLevel::variants().join(", ")
        )
    }
}

impl error::Error for StatusLevelParseError {}

/// An error that occurs while parsing an [`OutputFormat`] value from a string.
#[derive(Clone, Debug)]
pub struct OutputFormatParseError {
    input: String,
}

impl OutputFormatParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for OutputFormatParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unrecognized value for output-format: {}\n(known values: {})",
            self.input,
            OutputFormat::variants().join(", ")
        )
    }
}

impl error::Error for OutputFormatParseError {}

/// An error that occurs while parsing a [`RunIgnored`] value from a string.
#[derive(Clone, Debug)]
pub struct RunIgnoredParseError {
    input: String,
}

impl RunIgnoredParseError {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for RunIgnoredParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unrecognized value for run-ignored: {}\n(known values: {})",
            self.input,
            RunIgnored::variants().join(", ")
        )
    }
}

impl error::Error for RunIgnoredParseError {}

/// An error that occurs while parsing a
/// [`PartitionerBuilder`](crate::partition::PartitionerBuilder) input.
#[derive(Clone, Debug)]
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

impl error::Error for PartitionerBuilderParseError {}

/// An error that occurs in [`TestBinary::from_messages`](crate::test_list::TestBinary::from_messages).
#[derive(Debug)]
#[non_exhaustive]
pub enum FromMessagesError {
    /// An error occurred while reading Cargo's JSON messages.
    ReadMessages(std::io::Error),

    /// An error occurred while querying the package graph.
    PackageGraph(guppy::Error),
}

impl fmt::Display for FromMessagesError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FromMessagesError::ReadMessages(_) => {
                write!(f, "error reading Cargo JSON messages")
            }
            FromMessagesError::PackageGraph(_) => {
                write!(f, "error querying package graph")
            }
        }
    }
}

impl error::Error for FromMessagesError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            FromMessagesError::ReadMessages(error) => Some(error),
            FromMessagesError::PackageGraph(error) => Some(error),
        }
    }
}

/// An error that occurs while parsing test list output.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParseTestListError {
    /// Running a command to gather the list of tests failed.
    Command {
        /// The command that was run.
        command: Cow<'static, str>,

        /// The underlying error.
        error: std::io::Error,
    },

    /// An error occurred while parsing a line in the test output.
    ParseLine {
        /// A descriptive message.
        message: Cow<'static, str>,

        /// The full output.
        full_output: String,
    },
}

impl ParseTestListError {
    pub(crate) fn command(command: impl Into<Cow<'static, str>>, error: std::io::Error) -> Self {
        ParseTestListError::Command {
            command: command.into(),
            error,
        }
    }

    pub(crate) fn parse_line(
        message: impl Into<Cow<'static, str>>,
        full_output: impl Into<String>,
    ) -> Self {
        ParseTestListError::ParseLine {
            message: message.into(),
            full_output: full_output.into(),
        }
    }
}

impl fmt::Display for ParseTestListError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseTestListError::Command { command, .. } => {
                write!(f, "running '{}' failed", command)
            }
            ParseTestListError::ParseLine {
                message,
                full_output,
            } => {
                write!(f, "{}\nfull output:\n{}", message, full_output)
            }
        }
    }
}

impl error::Error for ParseTestListError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ParseTestListError::Command { error, .. } => Some(error),
            ParseTestListError::ParseLine { .. } => None,
        }
    }
}

/// An error that occurs while writing list output.
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteTestListError {
    /// An error occurred while writing the list to the provided output.
    Io(std::io::Error),

    /// An error occurred while serializing JSON, or while writing it to the provided output.
    Json(serde_json::Error),
}

impl fmt::Display for WriteTestListError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WriteTestListError::Io(_) => {
                write!(f, "error writing to output")
            }
            WriteTestListError::Json(_) => {
                write!(f, "error serializing to JSON")
            }
        }
    }
}

impl error::Error for WriteTestListError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            WriteTestListError::Io(error) => Some(error),
            WriteTestListError::Json(error) => Some(error),
        }
    }
}

/// An error that occurs while writing an event.
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteEventError {
    /// An error occurred while writing the event to the provided output.
    Io(std::io::Error),

    /// An error occurred while operating on the file system.
    Fs {
        /// The file being operated on.
        file: Utf8PathBuf,

        /// The underlying IO error.
        error: std::io::Error,
    },

    /// An error occurred while producing JUnit XML.
    Junit {
        /// The output file.
        file: Utf8PathBuf,

        error: quick_junit::Error,
    },
}

impl fmt::Display for WriteEventError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WriteEventError::Io(_) => {
                write!(f, "error writing to output")
            }
            WriteEventError::Fs { file, .. } => {
                write!(f, "error operating on path {}", file)
            }
            WriteEventError::Junit { file, .. } => {
                write!(f, "error writing JUnit output to {}", file)
            }
        }
    }
}

impl error::Error for WriteEventError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            WriteEventError::Io(error) => Some(error),
            WriteEventError::Fs { error, .. } => Some(error),
            WriteEventError::Junit { error, .. } => Some(error),
        }
    }
}
