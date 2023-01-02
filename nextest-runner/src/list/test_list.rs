// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::EnvironmentMap,
    double_spawn::DoubleSpawnInfo,
    errors::{CreateTestListError, FromMessagesError, WriteTestListError},
    helpers::{dylib_path, dylib_path_envvar, write_test_name},
    list::{BinaryList, OutputFormat, RustBuildMeta, Styles, TestListState},
    reuse_build::PathMapper,
    target_runner::{PlatformRunner, TargetRunner},
    test_command::{LocalExecuteContext, TestCommand},
    test_filter::TestFilterBuilder,
};
use camino::{Utf8Path, Utf8PathBuf};
use futures::prelude::*;
use guppy::{
    graph::{PackageGraph, PackageMetadata},
    PackageId,
};
use nextest_metadata::{
    BuildPlatform, RustBinaryId, RustNonTestBinaryKind, RustTestBinaryKind, RustTestBinarySummary,
    RustTestCaseSummary, RustTestSuiteStatusSummary, RustTestSuiteSummary, TestListSummary,
};
use once_cell::sync::{Lazy, OnceCell};
use owo_colors::OwoColorize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    io,
    io::Write,
    path::PathBuf,
    sync::Arc,
};
use tokio::runtime::Runtime;

/// A Rust test binary built by Cargo. This artifact hasn't been run yet so there's no information
/// about the tests within it.
///
/// Accepted as input to [`TestList::new`].
#[derive(Clone, Debug)]
pub struct RustTestArtifact<'g> {
    /// A unique identifier for this test artifact.
    pub binary_id: RustBinaryId,

    /// Metadata for the package this artifact is a part of. This is used to set the correct
    /// environment variables.
    pub package: PackageMetadata<'g>,

    /// The path to the binary artifact.
    pub binary_path: Utf8PathBuf,

    /// The unique binary name defined in `Cargo.toml` or inferred by the filename.
    pub binary_name: String,

    /// The kind of Rust test binary this is.
    pub kind: RustTestBinaryKind,

    /// Non-test binaries to be exposed to this artifact at runtime (name, path).
    pub non_test_binaries: BTreeSet<(String, Utf8PathBuf)>,

    /// The working directory that this test should be executed in.
    pub cwd: Utf8PathBuf,

    /// The platform for which this test artifact was built.
    pub build_platform: BuildPlatform,
}

impl<'g> RustTestArtifact<'g> {
    /// Constructs a list of test binaries from the list of built binaries.
    pub fn from_binary_list(
        graph: &'g PackageGraph,
        binary_list: Arc<BinaryList>,
        rust_build_meta: &RustBuildMeta<TestListState>,
        path_mapper: &PathMapper,
        platform_filter: Option<BuildPlatform>,
    ) -> Result<Vec<Self>, FromMessagesError> {
        let mut binaries = vec![];

        for binary in &binary_list.rust_binaries {
            if platform_filter.is_some() && platform_filter != Some(binary.build_platform) {
                continue;
            }

            // Look up the executable by package ID.
            let package_id = PackageId::new(binary.package_id.clone());
            let package = graph
                .metadata(&package_id)
                .map_err(FromMessagesError::PackageGraph)?;

            // Tests are run in the directory containing Cargo.toml
            let cwd = package
                .manifest_path()
                .parent()
                .unwrap_or_else(|| {
                    panic!(
                        "manifest path {} doesn't have a parent",
                        package.manifest_path()
                    )
                })
                .to_path_buf();

            let binary_path = path_mapper.map_binary(binary.path.clone());
            let cwd = path_mapper.map_cwd(cwd);

            // Non-test binaries are only exposed to integration tests and benchmarks.
            let non_test_binaries = if binary.kind == RustTestBinaryKind::TEST
                || binary.kind == RustTestBinaryKind::BENCH
            {
                // Note we must use the TestListState rust_build_meta here to ensure we get remapped
                // paths.
                match rust_build_meta.non_test_binaries.get(package_id.repr()) {
                    Some(binaries) => binaries
                        .iter()
                        .filter_map(|binary| {
                            // Only expose BIN_EXE non-test files.
                            (binary.kind == RustNonTestBinaryKind::BIN_EXE).then(|| {
                                // Convert relative paths to absolute ones by joining with the target directory.
                                let abs_path = rust_build_meta.target_directory.join(&binary.path);
                                (binary.name.clone(), abs_path)
                            })
                        })
                        .collect(),
                    None => BTreeSet::new(),
                }
            } else {
                BTreeSet::new()
            };

            binaries.push(RustTestArtifact {
                binary_id: binary.id.clone(),
                package,
                binary_path,
                binary_name: binary.name.clone(),
                kind: binary.kind.clone(),
                cwd,
                non_test_binaries,
                build_platform: binary.build_platform,
            })
        }

        Ok(binaries)
    }

    // ---
    // Helper methods
    // ---
    fn into_test_suite(self, status: RustTestSuiteStatus) -> (Utf8PathBuf, RustTestSuite<'g>) {
        let Self {
            binary_id,
            package,
            binary_path,
            binary_name,
            kind,
            non_test_binaries,
            cwd,
            build_platform,
        } = self;
        (
            binary_path,
            RustTestSuite {
                binary_id,
                package,
                binary_name,
                kind,
                non_test_binaries,
                cwd,
                build_platform,
                status,
            },
        )
    }
}

/// List of test instances, obtained by querying the [`RustTestArtifact`] instances generated by Cargo.
#[derive(Clone, Debug)]
pub struct TestList<'g> {
    test_count: usize,
    rust_build_meta: RustBuildMeta<TestListState>,
    rust_suites: BTreeMap<Utf8PathBuf, RustTestSuite<'g>>,
    env: EnvironmentMap,
    updated_dylib_path: OsString,
    // Computed on first access.
    skip_count: OnceCell<usize>,
}

impl<'g> TestList<'g> {
    /// Creates a new test list by running the given command and applying the specified filter.
    pub fn new<I>(
        ctx: &TestExecuteContext<'_>,
        test_artifacts: I,
        rust_build_meta: RustBuildMeta<TestListState>,
        filter: &TestFilterBuilder,
        env: EnvironmentMap,
        list_threads: usize,
    ) -> Result<Self, CreateTestListError>
    where
        I: IntoIterator<Item = RustTestArtifact<'g>>,
        I::IntoIter: Send,
    {
        let updated_dylib_path = Self::create_dylib_path(&rust_build_meta)?;
        log::debug!(
            "updated {}: {}",
            dylib_path_envvar(),
            updated_dylib_path.to_string_lossy(),
        );
        let ctx = LocalExecuteContext {
            double_spawn: ctx.double_spawn,
            runner: ctx.target_runner,
            dylib_path: &updated_dylib_path,
            env: &env,
        };

        let runtime = Runtime::new().map_err(CreateTestListError::TokioRuntimeCreate)?;

        let stream = futures::stream::iter(test_artifacts.into_iter()).map(|test_binary| {
            async {
                if filter.should_obtain_test_list_from_binary(&test_binary) {
                    // Run the binary to obtain the test list.
                    let (non_ignored, ignored) = test_binary.exec(&ctx).await?;
                    let (bin, info) = Self::process_output(
                        test_binary,
                        filter,
                        non_ignored.as_str(),
                        ignored.as_str(),
                    )?;
                    Ok::<_, CreateTestListError>((bin, info))
                } else {
                    // Skipped means no tests, so test_count doesn't need to be modified.
                    Ok(Self::process_skipped(test_binary))
                }
            }
        });
        let fut = stream.buffer_unordered(list_threads).try_collect();

        let rust_suites: BTreeMap<_, _> = runtime.block_on(fut)?;

        // Ensure that the runtime doesn't stay hanging even if a custom test framework misbehaves
        // (can be an issue on Windows).
        runtime.shutdown_background();

        let test_count = rust_suites
            .values()
            .map(|suite| suite.status.test_count())
            .sum();

        Ok(Self {
            rust_suites,
            env,
            rust_build_meta,
            updated_dylib_path,
            test_count,
            skip_count: OnceCell::new(),
        })
    }

    /// Creates a new test list with the given binary names and outputs.
    #[cfg(test)]
    fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<
            Item = (RustTestArtifact<'g>, impl AsRef<str>, impl AsRef<str>),
        >,
        rust_build_meta: RustBuildMeta<TestListState>,
        filter: &TestFilterBuilder,
        env: EnvironmentMap,
    ) -> Result<Self, CreateTestListError> {
        let mut test_count = 0;

        let updated_dylib_path = Self::create_dylib_path(&rust_build_meta)?;

        let test_artifacts = test_bin_outputs
            .into_iter()
            .map(|(test_binary, non_ignored, ignored)| {
                if filter.should_obtain_test_list_from_binary(&test_binary) {
                    let (bin, info) = Self::process_output(
                        test_binary,
                        filter,
                        non_ignored.as_ref(),
                        ignored.as_ref(),
                    )?;
                    test_count += info.status.test_count();
                    Ok((bin, info))
                } else {
                    // Skipped means no tests, so test_count doesn't need to be modified.
                    Ok(Self::process_skipped(test_binary))
                }
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(Self {
            rust_suites: test_artifacts,
            env,
            rust_build_meta,
            updated_dylib_path,
            test_count,
            skip_count: OnceCell::new(),
        })
    }

    /// Returns the total number of tests across all binaries.
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Returns the Rust build-related metadata for this test list.
    pub fn rust_build_meta(&self) -> &RustBuildMeta<TestListState> {
        &self.rust_build_meta
    }

    /// Returns the total number of skipped tests.
    pub fn skip_count(&self) -> usize {
        *self.skip_count.get_or_init(|| {
            self.iter_tests()
                .filter(|instance| !instance.test_info.filter_match.is_match())
                .count()
        })
    }

    /// Returns the total number of tests that aren't skipped.
    ///
    /// It is always the case that `run_count + skip_count == test_count`.
    pub fn run_count(&self) -> usize {
        self.test_count - self.skip_count()
    }

    /// Returns the total number of binaries that contain tests.
    pub fn binary_count(&self) -> usize {
        self.rust_suites.len()
    }

    /// Returns the tests for a given binary, or `None` if the binary wasn't in the list.
    pub fn get(&self, test_bin: impl AsRef<Utf8Path>) -> Option<&RustTestSuite> {
        self.rust_suites.get(test_bin.as_ref())
    }

    /// Returns the updated dynamic library path used for tests.
    pub fn updated_dylib_path(&self) -> &OsStr {
        &self.updated_dylib_path
    }

    /// Constructs a serializble summary for this test list.
    pub fn to_summary(&self) -> TestListSummary {
        let rust_suites = self
            .rust_suites
            .iter()
            .map(|(binary_path, info)| {
                let (status, test_cases) = info.status.to_summary();
                let testsuite = RustTestSuiteSummary {
                    package_name: info.package.name().to_owned(),
                    binary: RustTestBinarySummary {
                        binary_name: info.binary_name.clone(),
                        package_id: info.package.id().repr().to_owned(),
                        kind: info.kind.clone(),
                        binary_path: binary_path.clone(),
                        binary_id: info.binary_id.clone(),
                        build_platform: info.build_platform,
                    },
                    cwd: info.cwd.clone(),
                    status,
                    test_cases,
                };
                (info.binary_id.clone(), testsuite)
            })
            .collect();
        let mut summary = TestListSummary::new(self.rust_build_meta.to_summary());
        summary.test_count = self.test_count;
        summary.rust_suites = rust_suites;
        summary
    }

    /// Outputs this list to the given writer.
    pub fn write(
        &self,
        output_format: OutputFormat,
        writer: impl Write,
        colorize: bool,
    ) -> Result<(), WriteTestListError> {
        match output_format {
            OutputFormat::Human { verbose } => self
                .write_human(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Serializable(format) => format
                .to_writer(&self.to_summary(), writer)
                .map_err(WriteTestListError::Json),
        }
    }

    /// Iterates over all the test binaries.
    pub fn iter(&self) -> impl Iterator<Item = (&Utf8Path, &RustTestSuite)> + '_ {
        self.rust_suites
            .iter()
            .map(|(path, info)| (path.as_path(), info))
    }

    /// Iterates over the list of tests, returning the path and test name.
    pub fn iter_tests(&self) -> impl Iterator<Item = TestInstance<'_>> + '_ {
        self.rust_suites.iter().flat_map(|(test_bin, test_suite)| {
            test_suite
                .status
                .test_cases()
                .map(move |(name, test_info)| {
                    TestInstance::new(name, test_bin, test_suite, test_info)
                })
        })
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String, WriteTestListError> {
        // Ugh this sucks. String really should have an io::Write impl that errors on non-UTF8 text.
        let mut buf = Vec::with_capacity(1024);
        self.write(output_format, &mut buf, false)?;
        Ok(String::from_utf8(buf).expect("buffer is valid UTF-8"))
    }

    // ---
    // Helper methods
    // ---

    // Empty list for tests.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            test_count: 0,
            rust_build_meta: RustBuildMeta::empty(),
            env: EnvironmentMap::empty(),
            updated_dylib_path: OsString::new(),
            rust_suites: BTreeMap::new(),
            skip_count: OnceCell::new(),
        }
    }

    pub(crate) fn create_dylib_path(
        rust_build_meta: &RustBuildMeta<TestListState>,
    ) -> Result<OsString, CreateTestListError> {
        let dylib_path = dylib_path();
        let dylib_path_is_empty = dylib_path.is_empty();
        let new_paths = rust_build_meta.dylib_paths();

        let mut updated_dylib_path: Vec<PathBuf> =
            Vec::with_capacity(dylib_path.len() + new_paths.len());
        updated_dylib_path.extend(
            new_paths
                .iter()
                .map(|path| path.clone().into_std_path_buf()),
        );
        updated_dylib_path.extend(dylib_path);

        // On macOS, these are the defaults when DYLD_FALLBACK_LIBRARY_PATH isn't set or set to an
        // empty string. (This is relevant if nextest is invoked as its own process and not
        // a Cargo subcommand.)
        //
        // This copies the logic from
        // https://cs.github.com/rust-lang/cargo/blob/7d289b171183578d45dcabc56db6db44b9accbff/src/cargo/core/compiler/compilation.rs#L292.
        if cfg!(target_os = "macos") && dylib_path_is_empty {
            if let Some(home) = home::home_dir() {
                updated_dylib_path.push(home.join("lib"));
            }
            updated_dylib_path.push("/usr/local/lib".into());
            updated_dylib_path.push("/usr/lib".into());
        }

        std::env::join_paths(updated_dylib_path)
            .map_err(move |error| CreateTestListError::dylib_join_paths(new_paths, error))
    }

    fn process_output(
        test_binary: RustTestArtifact<'g>,
        filter: &TestFilterBuilder,
        non_ignored: impl AsRef<str>,
        ignored: impl AsRef<str>,
    ) -> Result<(Utf8PathBuf, RustTestSuite<'g>), CreateTestListError> {
        let mut test_cases = BTreeMap::new();

        // Treat ignored and non-ignored as separate sets of single filters, so that partitioning
        // based on one doesn't affect the other.
        let mut non_ignored_filter = filter.build();
        for test_name in Self::parse(&test_binary.binary_id, non_ignored.as_ref())? {
            test_cases.insert(
                test_name.into(),
                RustTestCaseSummary {
                    ignored: false,
                    filter_match: non_ignored_filter.filter_match(&test_binary, test_name, false),
                },
            );
        }

        let mut ignored_filter = filter.build();
        for test_name in Self::parse(&test_binary.binary_id, ignored.as_ref())? {
            // Note that libtest prints out:
            // * just ignored tests if --ignored is passed in
            // * all tests, both ignored and non-ignored, if --ignored is not passed in
            // Adding ignored tests after non-ignored ones makes everything resolve correctly.
            test_cases.insert(
                test_name.into(),
                RustTestCaseSummary {
                    ignored: true,
                    filter_match: ignored_filter.filter_match(&test_binary, test_name, true),
                },
            );
        }

        Ok(test_binary.into_test_suite(RustTestSuiteStatus::Listed { test_cases }))
    }

    fn process_skipped(test_binary: RustTestArtifact<'g>) -> (Utf8PathBuf, RustTestSuite<'g>) {
        test_binary.into_test_suite(RustTestSuiteStatus::Skipped)
    }

    /// Parses the output of --list --format terse and returns a sorted list.
    fn parse<'a>(
        binary_id: &'a RustBinaryId,
        list_output: &'a str,
    ) -> Result<Vec<&'a str>, CreateTestListError> {
        let mut list = Self::parse_impl(binary_id, list_output).collect::<Result<Vec<_>, _>>()?;
        list.sort_unstable();
        Ok(list)
    }

    fn parse_impl<'a>(
        binary_id: &'a RustBinaryId,
        list_output: &'a str,
    ) -> impl Iterator<Item = Result<&'a str, CreateTestListError>> + 'a {
        // The output is in the form:
        // <test name>: test
        // <test name>: test
        // ...

        list_output.lines().map(move |line| {
            line.strip_suffix(": test")
                .or_else(|| line.strip_suffix(": benchmark"))
                .ok_or_else(|| {
                    CreateTestListError::parse_line(
                        binary_id.clone(),
                        format!(
                            "line '{line}' did not end with the string ': test' or ': benchmark'"
                        ),
                        list_output,
                    )
                })
        })
    }

    fn write_human(&self, mut writer: impl Write, verbose: bool, colorize: bool) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }

        for (test_bin, info) in &self.rust_suites {
            // Skip this binary if there are no tests within it that will be run, and this isn't
            // verbose output.
            if !verbose
                && info
                    .status
                    .test_cases()
                    .all(|(_, test_case)| !test_case.filter_match.is_match())
            {
                continue;
            }

            writeln!(writer, "{}:", info.binary_id.style(styles.binary_id))?;
            if verbose {
                writeln!(writer, "  {} {}", "bin:".style(styles.field), test_bin)?;
                writeln!(writer, "  {} {}", "cwd:".style(styles.field), info.cwd)?;
                writeln!(
                    writer,
                    "  {} {}",
                    "build platform:".style(styles.field),
                    info.build_platform,
                )?;
            }

            let mut indented = indent_write::io::IndentWriter::new("    ", &mut writer);

            match &info.status {
                RustTestSuiteStatus::Listed { test_cases } => {
                    if test_cases.is_empty() {
                        writeln!(indented, "(no tests)")?;
                    } else {
                        for (name, info) in test_cases {
                            match (verbose, info.filter_match.is_match()) {
                                (_, true) => {
                                    write_test_name(name, &styles, &mut indented)?;
                                    writeln!(indented)?;
                                }
                                (true, false) => {
                                    write_test_name(name, &styles, &mut indented)?;
                                    writeln!(indented, " (skipped)")?;
                                }
                                (false, false) => {
                                    // Skip printing this test entirely if it isn't a match.
                                }
                            }
                        }
                    }
                }
                RustTestSuiteStatus::Skipped => {
                    writeln!(
                        indented,
                        "(test binary did not match filter expressions, skipped)"
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// A suite of tests within a single Rust test binary.
///
/// This is a representation of [`nextest_metadata::RustTestSuiteSummary`] used internally by the runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustTestSuite<'g> {
    /// A unique identifier for this binary.
    pub binary_id: RustBinaryId,

    /// Package metadata.
    pub package: PackageMetadata<'g>,

    /// The unique binary name defined in `Cargo.toml` or inferred by the filename.
    pub binary_name: String,

    /// The kind of Rust test binary this is.
    pub kind: RustTestBinaryKind,

    /// The working directory that this test binary will be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Utf8PathBuf,

    /// The platform the test suite is for (host or target).
    pub build_platform: BuildPlatform,

    /// Non-test binaries corresponding to this test suite (name, path).
    pub non_test_binaries: BTreeSet<(String, Utf8PathBuf)>,

    /// Test suite status and test case names.
    pub status: RustTestSuiteStatus,
}

impl<'g> RustTestArtifact<'g> {
    /// Run this binary with and without --ignored and get the corresponding outputs.
    async fn exec(
        &self,
        ctx: &LocalExecuteContext<'_>,
    ) -> Result<(String, String), CreateTestListError> {
        // This error situation has been known to happen with reused builds. It produces
        // a really terrible and confusing "file not found" message if allowed to prceed.
        if !self.cwd.is_dir() {
            return Err(CreateTestListError::CwdIsNotDir {
                binary_id: self.binary_id.clone(),
                cwd: self.cwd.clone(),
            });
        }
        let platform_runner = ctx.runner.for_build_platform(self.build_platform);

        let non_ignored = self.exec_single(false, ctx, platform_runner);
        let ignored = self.exec_single(true, ctx, platform_runner);

        let (non_ignored_out, ignored_out) = futures::future::join(non_ignored, ignored).await;
        Ok((non_ignored_out?, ignored_out?))
    }

    async fn exec_single(
        &self,
        ignored: bool,
        ctx: &LocalExecuteContext<'_>,
        runner: Option<&PlatformRunner>,
    ) -> Result<String, CreateTestListError> {
        let mut argv = Vec::new();

        let program: String = if let Some(runner) = runner {
            argv.extend(runner.args());
            argv.push(self.binary_path.as_str());
            runner.binary().into()
        } else {
            debug_assert!(
                self.binary_path.is_absolute(),
                "binary path {} is absolute",
                self.binary_path
            );
            self.binary_path.clone().into()
        };

        argv.extend(["--list", "--format", "terse"]);
        if ignored {
            argv.push("--ignored");
        }

        let mut cmd = TestCommand::new(
            ctx,
            program.clone(),
            &argv,
            &self.cwd,
            &self.package,
            &self.non_test_binaries,
        );
        // Capture stdout and stderr, and close stdin.
        cmd.command_mut()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|error| CreateTestListError::CommandExecFail {
                binary_id: self.binary_id.clone(),
                command: std::iter::once(program.clone())
                    .chain(argv.iter().map(|&s| s.to_owned()))
                    .collect(),
                error,
            })?;

        let output = child.wait_with_output().await.map_err(|error| {
            CreateTestListError::CommandExecFail {
                binary_id: self.binary_id.clone(),
                command: std::iter::once(program.clone())
                    .chain(argv.iter().map(|&s| s.to_owned()))
                    .collect(),
                error,
            }
        })?;

        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|err| CreateTestListError::CommandNonUtf8 {
                binary_id: self.binary_id.clone(),
                command: std::iter::once(program)
                    .chain(argv.iter().map(|&s| s.to_owned()))
                    .collect(),
                stdout: err.into_bytes(),
                stderr: output.stderr,
            })
        } else {
            Err(CreateTestListError::CommandFail {
                binary_id: self.binary_id.clone(),
                command: std::iter::once(program)
                    .chain(argv.iter().map(|&s| s.to_owned()))
                    .collect(),
                exit_status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
    }
}

/// Serializable information about the status of and test cases within a test suite.
///
/// Part of a [`RustTestSuiteSummary`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RustTestSuiteStatus {
    /// The test suite was executed with `--list` and the list of test cases was obtained.
    Listed {
        /// The test cases contained within this test suite.
        test_cases: BTreeMap<String, RustTestCaseSummary>,
    },

    /// The test suite was not executed.
    Skipped,
}

static EMPTY_TEST_CASE_MAP: Lazy<BTreeMap<String, RustTestCaseSummary>> = Lazy::new(BTreeMap::new);

impl RustTestSuiteStatus {
    /// Returns the number of test cases within this suite.
    pub fn test_count(&self) -> usize {
        match self {
            RustTestSuiteStatus::Listed { test_cases } => test_cases.len(),
            RustTestSuiteStatus::Skipped => 0,
        }
    }

    /// Returns the list of test cases within this suite.
    pub fn test_cases(&self) -> impl Iterator<Item = (&str, &RustTestCaseSummary)> + '_ {
        match self {
            RustTestSuiteStatus::Listed { test_cases } => test_cases.iter(),
            RustTestSuiteStatus::Skipped => {
                // Return an empty test case.
                EMPTY_TEST_CASE_MAP.iter()
            }
        }
        .map(|(name, case)| (name.as_str(), case))
    }

    /// Converts this status to its serializable form.
    pub fn to_summary(
        &self,
    ) -> (
        RustTestSuiteStatusSummary,
        BTreeMap<String, RustTestCaseSummary>,
    ) {
        match self {
            Self::Listed { test_cases } => (RustTestSuiteStatusSummary::LISTED, test_cases.clone()),
            Self::Skipped => (RustTestSuiteStatusSummary::SKIPPED, BTreeMap::new()),
        }
    }
}

/// Represents a single test with its associated binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestInstance<'a> {
    /// The name of the test.
    pub name: &'a str,

    /// The test binary.
    pub binary: &'a Utf8Path,

    /// Information about the test suite.
    pub suite_info: &'a RustTestSuite<'a>,

    /// Information about the test.
    pub test_info: &'a RustTestCaseSummary,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub(crate) fn new(
        name: &'a (impl AsRef<str> + ?Sized),
        binary: &'a (impl AsRef<Utf8Path> + ?Sized),
        suite_info: &'a RustTestSuite,
        test_info: &'a RustTestCaseSummary,
    ) -> Self {
        Self {
            name: name.as_ref(),
            binary: binary.as_ref(),
            suite_info,
            test_info,
        }
    }

    /// Return a reasonable key for sorting. This is (binary ID, test name).
    #[inline]
    pub(crate) fn sort_key(&self) -> (&'a str, &'a str) {
        ((self.suite_info.binary_id.as_str()), self.name)
    }

    /// Creates the command for this test instance.
    pub(crate) fn make_command(
        &self,
        ctx: &TestExecuteContext<'_>,
        test_list: &TestList<'_>,
    ) -> TestCommand {
        let platform_runner = ctx
            .target_runner
            .for_build_platform(self.suite_info.build_platform);
        // TODO: non-rust tests

        let mut args = Vec::new();

        let program: String = match platform_runner {
            Some(runner) => {
                args.extend(runner.args());
                args.push(self.binary.as_str());
                runner.binary().into()
            }
            None => self.binary.to_owned().into(),
        };

        args.extend(["--exact", self.name, "--nocapture"]);
        if self.test_info.ignored {
            args.push("--ignored");
        }

        let ctx = LocalExecuteContext {
            double_spawn: ctx.double_spawn,
            runner: ctx.target_runner,
            dylib_path: test_list.updated_dylib_path(),
            env: &test_list.env,
        };

        TestCommand::new(
            &ctx,
            program,
            &args,
            &self.suite_info.cwd,
            &self.suite_info.package,
            &self.suite_info.non_test_binaries,
        )
    }
}

/// Context required for test execution.
#[derive(Clone, Debug)]
pub struct TestExecuteContext<'a> {
    /// Double-spawn info.
    pub double_spawn: &'a DoubleSpawnInfo,

    /// Target runner.
    pub target_runner: &'a TargetRunner,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cargo_config::{TargetTriple, TargetTripleSource},
        list::SerializableFormat,
        test_filter::RunIgnored,
    };
    use guppy::CargoMetadata;
    use indoc::indoc;
    use maplit::btreemap;
    use nextest_filtering::FilteringExpr;
    use nextest_metadata::{FilterMatch, MismatchReason};
    use once_cell::sync::Lazy;
    use pretty_assertions::assert_eq;
    use std::iter;
    use target_spec::Platform;

    #[test]
    fn test_parse_test_list() {
        // Lines ending in ': benchmark' (output by the default Rust bencher) should be skipped.
        let non_ignored_output = indoc! {"
            tests::foo::test_bar: test
            tests::baz::test_quux: test
            benches::bench_foo: benchmark
        "};
        let ignored_output = indoc! {"
            tests::ignored::test_bar: test
            tests::baz::test_ignored: test
            benches::ignored_bench_foo: benchmark
        "};

        let test_filter = TestFilterBuilder::new(
            RunIgnored::Default,
            None,
            iter::empty::<String>(),
            // Test against the platform() predicate because this is the most important one here.
            vec![FilteringExpr::parse("platform(target)", &PACKAGE_GRAPH_FIXTURE).unwrap()],
        );
        let fake_cwd: Utf8PathBuf = "/fake/cwd".into();
        let fake_binary_name = "fake-binary".to_owned();
        let fake_binary_id = RustBinaryId::new("fake-package::fake-binary");

        let test_binary = RustTestArtifact {
            binary_path: "/fake/binary".into(),
            cwd: fake_cwd.clone(),
            package: package_metadata(),
            binary_name: fake_binary_name.clone(),
            binary_id: fake_binary_id.clone(),
            kind: RustTestBinaryKind::LIB,
            non_test_binaries: BTreeSet::new(),
            build_platform: BuildPlatform::Target,
        };

        let skipped_binary_name = "skipped-binary".to_owned();
        let skipped_binary_id = RustBinaryId::new("fake-package::skipped-binary");
        let skipped_binary = RustTestArtifact {
            binary_path: "/fake/skipped-binary".into(),
            cwd: fake_cwd.clone(),
            package: package_metadata(),
            binary_name: skipped_binary_name.clone(),
            binary_id: skipped_binary_id.clone(),
            kind: RustTestBinaryKind::PROC_MACRO,
            non_test_binaries: BTreeSet::new(),
            build_platform: BuildPlatform::Host,
        };

        let fake_triple = TargetTriple {
            platform: Platform::new(
                "aarch64-unknown-linux-gnu",
                target_spec::TargetFeatures::Unknown,
            )
            .unwrap(),
            source: TargetTripleSource::CliOption,
        };
        let fake_env = EnvironmentMap::empty();
        let rust_build_meta =
            RustBuildMeta::new("/fake", Some(fake_triple)).map_paths(&PathMapper::noop());
        let test_list = TestList::new_with_outputs(
            [
                (test_binary, &non_ignored_output, &ignored_output),
                (
                    skipped_binary,
                    &"should-not-show-up-stdout",
                    &"should-not-show-up-stderr",
                ),
            ],
            rust_build_meta,
            &test_filter,
            fake_env,
        )
        .expect("valid output");
        assert_eq!(
            test_list.rust_suites,
            btreemap! {
                "/fake/binary".into() => RustTestSuite {
                    status: RustTestSuiteStatus::Listed {
                        test_cases: btreemap! {
                            "tests::foo::test_bar".to_owned() => RustTestCaseSummary {
                                ignored: false,
                                filter_match: FilterMatch::Matches,
                            },
                            "tests::baz::test_quux".to_owned() => RustTestCaseSummary {
                                ignored: false,
                                filter_match: FilterMatch::Matches,
                            },
                            "benches::bench_foo".to_owned() => RustTestCaseSummary {
                                ignored: false,
                                filter_match: FilterMatch::Matches,
                            },
                            "tests::ignored::test_bar".to_owned() => RustTestCaseSummary {
                                ignored: true,
                                filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                            },
                            "tests::baz::test_ignored".to_owned() => RustTestCaseSummary {
                                ignored: true,
                                filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                            },
                            "benches::ignored_bench_foo".to_owned() => RustTestCaseSummary {
                                ignored: true,
                                filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                            },
                        },
                    },
                    cwd: fake_cwd.clone(),
                    build_platform: BuildPlatform::Target,
                    package: package_metadata(),
                    binary_name: fake_binary_name,
                    binary_id: fake_binary_id,
                    kind: RustTestBinaryKind::LIB,
                    non_test_binaries: BTreeSet::new(),
                },
                "/fake/skipped-binary".into() => RustTestSuite {
                    status: RustTestSuiteStatus::Skipped,
                    cwd: fake_cwd,
                    build_platform: BuildPlatform::Host,
                    package: package_metadata(),
                    binary_name: skipped_binary_name,
                    binary_id: skipped_binary_id,
                    kind: RustTestBinaryKind::PROC_MACRO,
                    non_test_binaries: BTreeSet::new(),
                },
            }
        );

        // Check that the expected outputs are valid.
        static EXPECTED_HUMAN: &str = indoc! {"
        fake-package::fake-binary:
            benches::bench_foo
            tests::baz::test_quux
            tests::foo::test_bar
        "};
        static EXPECTED_HUMAN_VERBOSE: &str = indoc! {"
            fake-package::fake-binary:
              bin: /fake/binary
              cwd: /fake/cwd
              build platform: target
                benches::bench_foo
                benches::ignored_bench_foo (skipped)
                tests::baz::test_ignored (skipped)
                tests::baz::test_quux
                tests::foo::test_bar
                tests::ignored::test_bar (skipped)
            fake-package::skipped-binary:
              bin: /fake/skipped-binary
              cwd: /fake/cwd
              build platform: host
                (test binary did not match filter expressions, skipped)
        "};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "rust-build-meta": {
                "target-directory": "/fake",
                "base-output-directories": [],
                "non-test-binaries": {},
                "linked-paths": [],
                "target-platforms": [
                  {
                    "triple": "aarch64-unknown-linux-gnu",
                    "target-features": "unknown"
                  }
                ],
                "target-platform": "aarch64-unknown-linux-gnu"
              },
              "test-count": 6,
              "rust-suites": {
                "fake-package::fake-binary": {
                  "package-name": "metadata-helper",
                  "binary-id": "fake-package::fake-binary",
                  "binary-name": "fake-binary",
                  "package-id": "metadata-helper 0.1.0 (path+file:///Users/fakeuser/local/testcrates/metadata/metadata-helper)",
                  "kind": "lib",
                  "binary-path": "/fake/binary",
                  "build-platform": "target",
                  "cwd": "/fake/cwd",
                  "status": "listed",
                  "testcases": {
                    "benches::bench_foo": {
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "benches::ignored_bench_foo": {
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_ignored": {
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_quux": {
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::foo::test_bar": {
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::ignored::test_bar": {
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    }
                  }
                },
                "fake-package::skipped-binary": {
                  "package-name": "metadata-helper",
                  "binary-id": "fake-package::skipped-binary",
                  "binary-name": "skipped-binary",
                  "package-id": "metadata-helper 0.1.0 (path+file:///Users/fakeuser/local/testcrates/metadata/metadata-helper)",
                  "kind": "proc-macro",
                  "binary-path": "/fake/skipped-binary",
                  "build-platform": "host",
                  "cwd": "/fake/cwd",
                  "status": "skipped",
                  "testcases": {}
                }
              }
            }"#};

        assert_eq!(
            test_list
                .to_string(OutputFormat::Human { verbose: false })
                .expect("human succeeded"),
            EXPECTED_HUMAN
        );
        assert_eq!(
            test_list
                .to_string(OutputFormat::Human { verbose: true })
                .expect("human succeeded"),
            EXPECTED_HUMAN_VERBOSE
        );
        println!(
            "{}",
            test_list
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded")
        );
        assert_eq!(
            test_list
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded"),
            EXPECTED_JSON_PRETTY
        );
    }

    static PACKAGE_GRAPH_FIXTURE: Lazy<PackageGraph> = Lazy::new(|| {
        static FIXTURE_JSON: &str = include_str!("../../../fixtures/cargo-metadata.json");
        let metadata = CargoMetadata::parse_json(FIXTURE_JSON).expect("fixture is valid JSON");
        metadata
            .build_graph()
            .expect("fixture is valid PackageGraph")
    });

    static PACKAGE_METADATA_ID: &str = "metadata-helper 0.1.0 (path+file:///Users/fakeuser/local/testcrates/metadata/metadata-helper)";
    fn package_metadata() -> PackageMetadata<'static> {
        PACKAGE_GRAPH_FIXTURE
            .metadata(&PackageId::new(PACKAGE_METADATA_ID))
            .expect("package ID is valid")
    }
}
