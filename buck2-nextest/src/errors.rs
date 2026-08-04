// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error types for `buck2-nextest`.

use camino::Utf8PathBuf;
use miette::Diagnostic;
use nextest_filtering::errors::FiltersetParseErrors;
use nextest_runner::errors::{
    ConfigParseError, CreateTestListError, FromMessagesError, ProfileNotFound,
    TestFilterBuildError, TestRunnerBuildError,
};
use thiserror::Error;

/// The result type used throughout `buck2-nextest`.
pub type Result<T, E = ExpectedError> = std::result::Result<T, E>;

/// An error that is expected to occur in normal operation, and that carries an
/// exit code.
///
/// This mirrors `cargo-nextest`'s error model: these are user or environment
/// errors, as opposed to internal errors which panic.
#[derive(Debug, Diagnostic, Error)]
#[non_exhaustive]
pub enum ExpectedError {
    /// The spec file could not be read.
    #[error("failed to read spec file `{path}`")]
    SpecReadError {
        /// The path that could not be read.
        path: Utf8PathBuf,
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// The spec could not be read from standard input.
    #[error("failed to read spec from standard input")]
    SpecStdinReadError {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// The spec was not valid JSON, or did not match the expected shape.
    #[error("failed to parse spec from {source_name}")]
    SpecParseError {
        /// Where the spec came from, for the error message.
        source_name: String,
        /// The underlying error, carrying the path to the offending field.
        #[source]
        error: serde_path_to_error::Error<serde_json::Error>,
    },

    /// A target's test type is not supported.
    #[error("target `{label}` has test type `{test_type}`, which is not supported")]
    #[diagnostic(help(
        "buck2-nextest currently runs Rust tests only, since it drives them over the \
         libtest protocol; filter the spec down to `rust` targets"
    ))]
    UnsupportedTestType {
        /// The target's label.
        label: String,
        /// The unsupported test type.
        test_type: String,
    },

    /// A target's command or environment contained a handle rather than a
    /// literal value.
    #[error("target `{label}` uses a {kind} in its {location}, which cannot be resolved")]
    #[diagnostic(help(
        "handles are resolved by calling back into the Buck2 orchestrator over gRPC, \
         which a file-based spec has no channel for; re-export the spec with values \
         inlined"
    ))]
    UnresolvedSpecValue {
        /// The target's label.
        label: String,
        /// The kind of handle, e.g. `arg_handle`.
        kind: &'static str,
        /// Where the handle appeared, e.g. "command" or "env var `FOO`".
        location: String,
    },

    /// A target had an empty command, so there is no binary to run.
    #[error("target `{label}` has an empty command")]
    #[diagnostic(help("a test target's command must start with the test binary to run"))]
    EmptyCommand {
        /// The target's label.
        label: String,
    },

    /// Two targets in the spec had the same label.
    #[error("target `{label}` appears more than once in the spec")]
    DuplicateTarget {
        /// The duplicated label.
        label: String,
    },

    /// The arguments after `--` on the `buck2 test` command line were invalid.
    ///
    /// Rendered by clap itself, which already produces a usage message.
    #[error("failed to parse the arguments after `--`")]
    PassthroughParseError {
        /// The underlying error.
        #[source]
        error: clap::Error,
    },

    /// An `--env` argument was not of the form `NAME=VALUE`.
    #[error("`--env {arg}` is not of the form `NAME=VALUE`")]
    MalformedEnvArg {
        /// The argument as it was given.
        arg: String,
    },

    /// The executor was invoked without a usable pair of sockets.
    #[error("no Buck2 sockets were given")]
    #[diagnostic(help(
        "this binary is a Buck2 test executor; point Buck2 at it with \
         `v2_test_executor` in the `[test]` section of `.buckconfig` and run \
         `buck2 test`, or use a subcommand to drive it directly"
    ))]
    NoExecutorSockets,

    /// Only one of the two sockets was named.
    #[error("Buck2 named the {given} socket but not the {missing} one")]
    IncompleteExecutorSockets {
        /// The socket that was given.
        given: &'static str,
        /// The socket that was not.
        missing: &'static str,
    },

    /// A file descriptor Buck2 passed could not be turned into a socket.
    #[error("failed to adopt the {which} socket")]
    SocketAdoptError {
        /// Which socket, for the error message.
        which: &'static str,
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// A socket address Buck2 named could not be connected to.
    #[error("failed to connect to the {which} socket at `{addr}`")]
    SocketConnectError {
        /// Which socket, for the error message.
        which: &'static str,
        /// The address that could not be reached.
        addr: String,
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// A gRPC channel could not be established over a socket.
    #[error("failed to set up a gRPC channel over the {which} socket")]
    ChannelBuildError {
        /// Which socket, for the error message.
        which: &'static str,
        /// The underlying error.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Serving the executor service failed.
    #[error("the connection to Buck2 failed")]
    ExecutorServeError {
        /// The underlying error.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Buck2 disconnected before saying it had sent every test target.
    #[error("Buck2 disconnected before sending all test targets")]
    #[diagnostic(help("check `buck2 test`'s own output for the failure that caused this"))]
    Buck2Disconnected,

    /// A message from Buck2 was missing a field the protocol requires.
    #[error("Buck2 sent a message for `{label}` with no `{field}`")]
    SpecMissingField {
        /// The field that was missing.
        field: &'static str,
        /// The target the message was about.
        label: String,
    },

    /// Buck2 could not say how to run a target locally.
    #[error("Buck2 could not prepare `{label}` for local execution")]
    PrepareForLocalExecutionError {
        /// The target that could not be prepared.
        label: String,
        /// The status Buck2 returned.
        #[source]
        status: Box<tonic::Status>,
    },

    /// Reporting results back to Buck2 failed.
    #[error("failed to report results to Buck2")]
    ReportResultsError {
        /// The status Buck2 returned.
        #[source]
        status: Box<tonic::Status>,
    },

    /// The tokio runtime the gRPC connections need could not be created.
    #[error("failed to create the async runtime for the Buck2 connection")]
    RuntimeCreateError {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// The host platform could not be determined.
    #[error("failed to detect the host platform")]
    HostPlatformDetectError {
        /// The underlying error.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Nextest's configuration could not be parsed.
    #[error("failed to parse nextest configuration")]
    ConfigParseError {
        /// The underlying error.
        #[source]
        error: ConfigParseError,
    },

    /// The requested profile was not found.
    #[error("profile not found")]
    ProfileNotFound {
        /// The underlying error.
        #[source]
        error: ProfileNotFound,
    },

    /// One or more filtersets could not be parsed.
    ///
    /// `FiltersetParseErrors` is not itself a `Diagnostic`; each individual
    /// error is, so they are rendered separately by
    /// [`ExpectedError::display_extra`].
    #[error("failed to parse filterset")]
    FiltersetParseError {
        /// The underlying errors, one entry per filterset that failed.
        all_errors: Vec<FiltersetParseErrors>,
    },

    /// The test filter could not be constructed.
    #[error("failed to construct test filter")]
    TestFilterBuildError {
        /// The underlying error.
        #[source]
        error: TestFilterBuildError,
    },

    /// The binary list could not be converted into test artifacts.
    #[error("failed to build the list of test binaries")]
    FromMessagesError {
        /// The underlying error.
        #[source]
        error: FromMessagesError,
    },

    /// The test list could not be created.
    #[error("failed to list tests")]
    CreateTestListError {
        /// The underlying error.
        #[source]
        error: CreateTestListError,
    },

    /// The test runner could not be built.
    #[error("failed to set up the test runner")]
    TestRunnerBuildError {
        /// The underlying error.
        #[source]
        error: TestRunnerBuildError,
    },

    /// Writing reporter output failed.
    #[error("failed to write test output")]
    WriteEventError {
        /// The underlying error.
        #[source]
        error: std::io::Error,
    },

    /// The test run itself reported failures.
    ///
    /// This carries no message: the reporter has already described what failed.
    #[error("test run failed")]
    TestRunFailed,

    /// No tests were run, and that was configured to be an error.
    ///
    /// Only reachable from the spec-file path. Under Buck2 an empty run is
    /// success, since Buck2 chose the targets.
    #[error("no tests to run")]
    NoTestsRun,
}

impl ExpectedError {
    /// Renders any diagnostics that cannot be attached as a `#[source]`.
    ///
    /// Filterset errors carry spans into the filterset text, so each is
    /// rendered as its own report with that text as the source code.
    pub fn display_extra(&self) {
        if let Self::FiltersetParseError { all_errors } = self {
            for errors in all_errors {
                for single_error in &errors.errors {
                    let report = miette::Report::new(single_error.clone())
                        .with_source_code(errors.input.clone());
                    eprintln!("{report:?}");
                }
            }
        }
    }

    /// Returns the process exit code for this error.
    ///
    /// These match `cargo-nextest`'s codes so that tooling can treat the two
    /// binaries alike.
    pub fn exit_code(&self) -> i32 {
        use nextest_metadata::NextestExitCode;

        match self {
            Self::SpecReadError { .. }
            | Self::SpecStdinReadError { .. }
            | Self::SpecParseError { .. }
            | Self::UnsupportedTestType { .. }
            | Self::UnresolvedSpecValue { .. }
            | Self::EmptyCommand { .. }
            | Self::DuplicateTarget { .. } => NextestExitCode::SETUP_ERROR,
            Self::PassthroughParseError { .. }
            | Self::MalformedEnvArg { .. }
            | Self::NoExecutorSockets
            | Self::IncompleteExecutorSockets { .. }
            | Self::SocketAdoptError { .. }
            | Self::SocketConnectError { .. }
            | Self::ChannelBuildError { .. }
            | Self::RuntimeCreateError { .. } => NextestExitCode::SETUP_ERROR,
            // Buck2 going away, or answering badly, is not something the user
            // did wrong here; treat it the way a failed listing is treated.
            Self::ExecutorServeError { .. }
            | Self::Buck2Disconnected
            | Self::SpecMissingField { .. }
            | Self::PrepareForLocalExecutionError { .. }
            | Self::ReportResultsError { .. } => NextestExitCode::TEST_LIST_CREATION_FAILED,
            Self::HostPlatformDetectError { .. } => NextestExitCode::SETUP_ERROR,
            Self::ConfigParseError { .. } | Self::ProfileNotFound { .. } => {
                NextestExitCode::SETUP_ERROR
            }
            Self::FiltersetParseError { .. } | Self::TestFilterBuildError { .. } => {
                NextestExitCode::SETUP_ERROR
            }
            Self::FromMessagesError { .. } | Self::CreateTestListError { .. } => {
                NextestExitCode::TEST_LIST_CREATION_FAILED
            }
            Self::TestRunnerBuildError { .. } => NextestExitCode::SETUP_ERROR,
            Self::WriteEventError { .. } => NextestExitCode::WRITE_OUTPUT_ERROR,
            Self::TestRunFailed => NextestExitCode::TEST_RUN_FAILED,
            Self::NoTestsRun => NextestExitCode::NO_TESTS_RUN,
        }
    }
}
