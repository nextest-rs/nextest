// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Listing and running tests described by a Buck2 spec.
//!
//! This mirrors `cargo-nextest`'s run path, minus everything Cargo-specific:
//! there is no build step, no package graph, and no path remapping.

use crate::{
    convert::Buck2BinaryList,
    errors::{ExpectedError, Result},
};
use camino::Utf8PathBuf;
use nextest_filtering::{Filterset, FiltersetKind, KnownGroups, ParseContext};
use nextest_metadata::NextestExitCode;
use nextest_runner::{
    cargo_config::EnvironmentMap,
    config::core::{ConfigExperimental, EarlyProfile, EvaluatableProfile, NextestConfig},
    double_spawn::DoubleSpawnInfo,
    errors::WriteEventError,
    helpers::{ShowTerminalProgress, ThemeCharacters},
    input::InputHandlerKind,
    list::{ListProgressOptions, OutputFormat, RustTestArtifact, TestExecuteContext, TestList},
    reporter::{
        ReporterBuilder, ReporterOutput, ShowProgress,
        events::{FinalRunStats, ReporterEvent},
        structured::StructuredReporter,
    },
    reuse_build::PathMapper,
    run_mode::NextestRunMode,
    runner::{TestRunnerBuilder, VersionEnvVars, configure_handle_inheritance},
    signal::SignalHandlerKind,
    target_runner::TargetRunner,
    test_filter::{FilterBound, RunIgnored, TestFilter, TestFilterPatterns},
    write_str::WriteStr,
};
use quick_junit::ReportUuid;
use std::{
    convert::Infallible,
    fmt,
    io::{IsTerminal, Write},
    sync::Arc,
};
use thiserror::Error;

/// Where the displayer's output goes, and so who owns the terminal.
///
/// A plain flag rather than a [`ReporterOutput`], which borrows a writer and is
/// invariant over its lifetime, so it has to be built where it is used.
#[derive(Clone, Copy, Debug)]
enum OutputTo {
    /// The terminal, as `cargo-nextest` does.
    Terminal,

    /// Standard error, plainly, for when something else owns the terminal.
    ///
    /// Buck2 captures the executor's standard error and shows it only on
    /// request (`buck2 test --test-executor-stderr=-`), so this is a detail
    /// view rather than the primary one -- which is why it is written without
    /// a progress bar or any other cursor control.
    PlainStderr,
}

impl OutputTo {
    /// Returns how to handle signals and terminal input.
    ///
    /// Input handling is interactive: it reads standard input to offer nextest's
    /// info and pause features. That only makes sense when nextest is what the
    /// person is looking at, and under Buck2 standard input belongs to Buck2.
    ///
    /// Signals are the other way round. Whoever spawned the test processes has
    /// to shut them down, and that is nextest either way -- so `Ctrl-C` must
    /// still reach the graceful cancellation path rather than killing this
    /// process and orphaning the tests it started.
    fn handlers(self) -> (SignalHandlerKind, InputHandlerKind) {
        match self {
            Self::Terminal => (SignalHandlerKind::Standard, InputHandlerKind::Standard),
            Self::PlainStderr => (SignalHandlerKind::Standard, InputHandlerKind::Noop),
        }
    }
}

/// Why a run's event callback failed.
///
/// The two arms are kept apart so the message says whether the run stopped
/// because results could not be delivered or because they could not be
/// rendered. Implementing `Error` rather than only `Debug` is what lets
/// `TestRunnerExecuteErrors` render this properly.
#[derive(Debug, Error)]
enum RunError {
    /// Forwarding an event to the caller's sink failed.
    #[error("failed to forward results: {0}")]
    Sink(String),

    /// Writing an event to the reporter failed.
    #[error(transparent)]
    Report(WriteEventError),
}

/// Writes the displayer's output to standard error, without any cursor control.
struct PlainStderrWriter;

impl WriteStr for PlainStderrWriter {
    fn write_str(&mut self, s: &str) -> std::io::Result<()> {
        std::io::stderr().write_all(s.as_bytes())
    }

    fn write_str_flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

/// Everything needed to list or run the tests in a spec.
pub struct RunContext {
    /// The converted binary list and its packages.
    pub binaries: Buck2BinaryList,

    /// The Buck2 project root.
    pub project_root: Utf8PathBuf,

    /// The nextest profile to use.
    pub profile_name: Option<String>,

    /// A path to a nextest configuration file, if one was given.
    pub config_file: Option<Utf8PathBuf>,

    /// Filtersets from the command line.
    pub filtersets: Vec<String>,

    /// Substring filters from the command line.
    pub filter_patterns: Vec<String>,

    /// Whether to run ignored tests.
    pub run_ignored: RunIgnored,

    /// The number of threads to list tests with.
    pub list_threads: usize,
}

impl RunContext {
    /// Lists the tests in the spec, writing them in the requested format.
    pub fn list(&self, format: OutputFormat, writer: &mut dyn WriteStr) -> Result<()> {
        let config = self.load_config()?;
        let early_profile = self.load_profile(&config)?;
        let filter = self.build_filter(&early_profile.known_groups())?;
        let profile = early_profile
            .apply_build_platforms(&self.binaries.binary_list.rust_build_meta.build_platforms);
        let test_list = self.build_test_list(&profile, &filter)?;

        test_list
            .write(format, writer, false)
            .map_err(|error| ExpectedError::WriteEventError {
                error: std::io::Error::other(error),
            })
    }

    /// Runs the tests in the spec, returning the process exit code.
    pub fn run(&self, cli_args: Vec<String>) -> Result<i32> {
        self.run_inner(cli_args, OutputTo::Terminal, |_| Ok::<(), Infallible>(()))
    }

    /// Runs the tests, forwarding every event to `sink` as well.
    ///
    /// Used when something other than the terminal consumes results -- Buck2,
    /// which renders them itself. The displayer still runs, writing plainly to
    /// standard error, so its output is there for anyone who asks Buck2 for it.
    ///
    /// If `sink` returns an error, the run is cancelled gracefully: nextest
    /// keeps reporting until the tests it has already started finish.
    pub fn run_with_sink<E, F>(&self, cli_args: Vec<String>, sink: F) -> Result<i32>
    where
        F: FnMut(&ReporterEvent<'_>) -> std::result::Result<(), E> + Send,
        E: fmt::Debug + Send,
    {
        self.run_inner(cli_args, OutputTo::PlainStderr, sink)
    }

    fn run_inner<E, F>(&self, cli_args: Vec<String>, output: OutputTo, mut sink: F) -> Result<i32>
    where
        F: FnMut(&ReporterEvent<'_>) -> std::result::Result<(), E> + Send,
        E: fmt::Debug + Send,
    {
        let config = self.load_config()?;
        let early_profile = self.load_profile(&config)?;
        let filter = self.build_filter(&early_profile.known_groups())?;
        let profile = early_profile
            .apply_build_platforms(&self.binaries.binary_list.rust_build_meta.build_platforms);
        let test_list = self.build_test_list(&profile, &filter)?;

        let (signal_handler, input_handler) = output.handlers();
        let runner = TestRunnerBuilder::default()
            .build(
                ReportUuid::new_v4(),
                version_env_vars(),
                &test_list,
                &profile,
                cli_args,
                signal_handler,
                input_handler,
                DoubleSpawnInfo::disabled(),
                TargetRunner::empty(),
            )
            .map_err(|error| ExpectedError::TestRunnerBuildError { error })?;

        // The writer must be declared here, next to the borrows it shares a
        // lifetime with: `ReporterOutput` is invariant, so one built further
        // out cannot be narrowed to this scope.
        let mut plain_stderr = PlainStderrWriter;
        let output = match output {
            OutputTo::Terminal => ReporterOutput::Terminal,
            OutputTo::PlainStderr => ReporterOutput::Writer {
                writer: &mut plain_stderr,
                // Whatever is downstream of Buck2's capture is unknown, so
                // stick to ASCII.
                use_unicode: false,
            },
        };

        let mut reporter = ReporterBuilder::default().build(
            &test_list,
            &profile,
            ShowTerminalProgress::No,
            output,
            StructuredReporter::new(),
        );

        configure_handle_inheritance(false).map_err(|error| ExpectedError::WriteEventError {
            error: std::io::Error::other(error),
        })?;

        let run_stats = runner
            .try_execute(|event| {
                // The sink goes first: if it has failed, there is no point
                // rendering an event nobody will see, and returning its error
                // is what starts a graceful cancellation.
                sink(&event).map_err(|error| RunError::Sink(format!("{error:?}")))?;
                reporter.report_event(event).map_err(RunError::Report)
            })
            .map_err(|errors| ExpectedError::WriteEventError {
                error: std::io::Error::other(errors.to_string()),
            })?;
        reporter.finish();

        match run_stats.summarize_final() {
            FinalRunStats::Success => Ok(0),
            // A run with nothing in it is an error by default, matching
            // cargo-nextest's `--no-tests` default.
            FinalRunStats::NoTestsRun => Ok(NextestExitCode::NO_TESTS_RUN),
            FinalRunStats::Failed { .. } | FinalRunStats::Cancelled { .. } => {
                Ok(NextestExitCode::TEST_RUN_FAILED)
            }
        }
    }

    // ---
    // Helper methods
    // ---

    fn load_config(&self) -> Result<NextestConfig> {
        // Buck2 has no Cargo package graph, so package-graph filterset
        // predicates are unavailable. See `ParseContext::without_graph`.
        let pcx = ParseContext::without_graph();

        NextestConfig::from_sources(
            self.project_root.clone(),
            &pcx,
            self.config_file.as_deref(),
            &[],
            &ConfigExperimental::from_env(),
        )
        .map_err(|error| ExpectedError::ConfigParseError { error })
    }

    fn load_profile<'cfg>(&self, config: &'cfg NextestConfig) -> Result<EarlyProfile<'cfg>> {
        let name = self
            .profile_name
            .as_deref()
            .unwrap_or(NextestConfig::DEFAULT_PROFILE);
        config
            .profile(name)
            .map_err(|error| ExpectedError::ProfileNotFound { error })
    }

    /// Builds the test filter.
    ///
    /// `known_groups` comes from the profile: `group()` is legal in a test
    /// filterset, so the set of valid group names must be known before the
    /// filterset is compiled.
    fn build_filter(&self, known_groups: &KnownGroups) -> Result<TestFilter> {
        let pcx = ParseContext::without_graph();
        // Report every bad filterset at once rather than stopping at the first.
        let mut exprs = Vec::with_capacity(self.filtersets.len());
        let mut all_errors = Vec::new();
        for input in &self.filtersets {
            match Filterset::parse(input.clone(), &pcx, FiltersetKind::Test, known_groups) {
                Ok(expr) => exprs.push(expr),
                Err(errors) => all_errors.push(errors),
            }
        }
        if !all_errors.is_empty() {
            return Err(ExpectedError::FiltersetParseError { all_errors });
        }

        TestFilter::new(
            NextestRunMode::Test,
            self.run_ignored,
            TestFilterPatterns::new(self.filter_patterns.clone()),
            exprs,
        )
        .map_err(|error| ExpectedError::TestFilterBuildError { error })
    }

    fn build_test_list<'a>(
        &'a self,
        profile: &EvaluatableProfile<'_>,
        filter: &TestFilter,
    ) -> Result<TestList<'a>> {
        // No path remapping: the spec's paths are already where the binaries are.
        let path_mapper = PathMapper::noop();
        let rust_build_meta = self
            .binaries
            .binary_list
            .rust_build_meta
            .map_paths(&path_mapper);

        let artifacts = RustTestArtifact::from_binary_list(
            &self.binaries.packages,
            Arc::new(self.binaries.binary_list.clone()),
            &rust_build_meta,
            &path_mapper,
            None,
        )
        .map_err(|error| ExpectedError::FromMessagesError { error })?;

        let version_env_vars = version_env_vars();
        let double_spawn = DoubleSpawnInfo::disabled();
        let target_runner = TargetRunner::empty();
        let ctx = TestExecuteContext {
            run_id: ReportUuid::new_v4(),
            version_env_vars: &version_env_vars,
            profile_name: profile.name(),
            double_spawn: &double_spawn,
            target_runner: &target_runner,
        };

        TestList::new(
            &ctx,
            artifacts,
            rust_build_meta,
            filter,
            None,
            self.project_root.clone(),
            EnvironmentMap::empty(),
            profile,
            FilterBound::All,
            self.list_threads,
            ListProgressOptions::new(
                ShowProgress::default(),
                ShowTerminalProgress::No,
                ThemeCharacters::default(),
                std::io::stderr().is_terminal(),
            ),
        )
        .map_err(|error| ExpectedError::CreateTestListError { error })
    }
}

fn version_env_vars() -> VersionEnvVars {
    VersionEnvVars {
        current_version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("crate version is valid semver"),
        required_version: None,
        recommended_version: None,
    }
}
