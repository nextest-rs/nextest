// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{DisplayFilterMatcher, TestListDisplayFilter};
use crate::{
    cargo_config::EnvironmentMap,
    config::{
        core::EvaluatableProfile,
        overrides::{ListSettings, TestSettings},
        scripts::{WrapperScriptConfig, WrapperScriptTargetRunner},
    },
    double_spawn::DoubleSpawnInfo,
    errors::{
        CreateTestListError, FromMessagesError, TestListFromSummaryError, WriteTestListError,
    },
    helpers::{convert_build_platform, dylib_path, dylib_path_envvar, write_test_name},
    indenter::indented,
    list::{BinaryList, OutputFormat, RustBuildMeta, Styles, TestListState},
    reuse_build::PathMapper,
    run_mode::NextestRunMode,
    runner::Interceptor,
    target_runner::{PlatformRunner, TargetRunner},
    test_command::{LocalExecuteContext, TestCommand, TestCommandPhase},
    test_filter::{BinaryMismatchReason, FilterBinaryMatch, FilterBound, TestFilterBuilder},
    write_str::WriteStr,
};
use camino::{Utf8Path, Utf8PathBuf};
use debug_ignore::DebugIgnore;
use futures::prelude::*;
use guppy::{
    PackageId,
    graph::{PackageGraph, PackageMetadata},
};
use iddqd::{IdOrdItem, IdOrdMap, id_upcast};
use nextest_filtering::{BinaryQuery, EvalContext, TestQuery};
use nextest_metadata::{
    BuildPlatform, FilterMatch, MismatchReason, RustBinaryId, RustNonTestBinaryKind,
    RustTestBinaryKind, RustTestBinarySummary, RustTestCaseSummary, RustTestKind,
    RustTestSuiteStatusSummary, RustTestSuiteSummary, TestCaseName, TestListSummary,
};
use owo_colors::OwoColorize;
use quick_junit::ReportUuid;
use serde::{Deserialize, Serialize};
use std::{
    borrow::{Borrow, Cow},
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fmt,
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use swrite::{SWrite, swrite};
use tokio::runtime::Runtime;
use tracing::debug;

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
                        .filter(|binary| {
                            // Only expose BIN_EXE non-test files.
                            binary.kind == RustNonTestBinaryKind::BIN_EXE
                        })
                        .map(|binary| {
                            // Convert relative paths to absolute ones by joining with the target directory.
                            let abs_path = rust_build_meta.target_directory.join(&binary.path);
                            (binary.name.clone(), abs_path)
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

    /// Returns a [`BinaryQuery`] corresponding to this test artifact.
    pub fn to_binary_query(&self) -> BinaryQuery<'_> {
        BinaryQuery {
            package_id: self.package.id(),
            binary_id: &self.binary_id,
            kind: &self.kind,
            binary_name: &self.binary_name,
            platform: convert_build_platform(self.build_platform),
        }
    }

    // ---
    // Helper methods
    // ---
    fn into_test_suite(self, status: RustTestSuiteStatus) -> RustTestSuite<'g> {
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

        RustTestSuite {
            binary_id,
            binary_path,
            package,
            binary_name,
            kind,
            non_test_binaries,
            cwd,
            build_platform,
            status,
        }
    }
}

/// Information about skipped tests and binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkipCounts {
    /// The number of skipped tests.
    pub skipped_tests: usize,

    /// The number of skipped tests due to this being a rerun and the test was
    /// already passing.
    pub skipped_tests_rerun: usize,

    /// The number of tests skipped because they are not benchmarks.
    ///
    /// This is used when running in benchmark mode.
    pub skipped_tests_non_benchmark: usize,

    /// The number of tests skipped due to not being in the default set.
    pub skipped_tests_default_filter: usize,

    /// The number of skipped binaries.
    pub skipped_binaries: usize,

    /// The number of binaries skipped due to not being in the default set.
    pub skipped_binaries_default_filter: usize,
}

/// List of test instances, obtained by querying the [`RustTestArtifact`] instances generated by Cargo.
#[derive(Clone, Debug)]
pub struct TestList<'g> {
    test_count: usize,
    mode: NextestRunMode,
    rust_build_meta: RustBuildMeta<TestListState>,
    rust_suites: IdOrdMap<RustTestSuite<'g>>,
    workspace_root: Utf8PathBuf,
    env: EnvironmentMap,
    updated_dylib_path: OsString,
    // Computed on first access.
    skip_counts: OnceLock<SkipCounts>,
}

impl<'g> TestList<'g> {
    /// Creates a new test list by running the given command and applying the specified filter.
    #[expect(clippy::too_many_arguments)]
    pub fn new<I>(
        ctx: &TestExecuteContext<'_>,
        test_artifacts: I,
        rust_build_meta: RustBuildMeta<TestListState>,
        filter: &TestFilterBuilder,
        workspace_root: Utf8PathBuf,
        env: EnvironmentMap,
        profile: &impl ListProfile,
        bound: FilterBound,
        list_threads: usize,
    ) -> Result<Self, CreateTestListError>
    where
        I: IntoIterator<Item = RustTestArtifact<'g>>,
        I::IntoIter: Send,
    {
        let updated_dylib_path = Self::create_dylib_path(&rust_build_meta)?;
        debug!(
            "updated {}: {}",
            dylib_path_envvar(),
            updated_dylib_path.to_string_lossy(),
        );
        let lctx = LocalExecuteContext {
            phase: TestCommandPhase::List,
            // Note: this is the remapped workspace root, not the original one.
            // (We really should have newtypes for this.)
            workspace_root: &workspace_root,
            rust_build_meta: &rust_build_meta,
            double_spawn: ctx.double_spawn,
            dylib_path: &updated_dylib_path,
            profile_name: ctx.profile_name,
            env: &env,
        };

        let ecx = profile.filterset_ecx();

        let runtime = Runtime::new().map_err(CreateTestListError::TokioRuntimeCreate)?;

        let stream = futures::stream::iter(test_artifacts).map(|test_binary| {
            async {
                let binary_query = test_binary.to_binary_query();
                let binary_match = filter.filter_binary_match(&test_binary, &ecx, bound);
                match binary_match {
                    FilterBinaryMatch::Definite | FilterBinaryMatch::Possible => {
                        debug!(
                            "executing test binary to obtain test list \
                            (match result is {binary_match:?}): {}",
                            test_binary.binary_id,
                        );
                        // Run the binary to obtain the test list.
                        let list_settings = profile.list_settings_for(&binary_query);
                        let (non_ignored, ignored) = test_binary
                            .exec(&lctx, &list_settings, ctx.target_runner)
                            .await?;
                        let info = Self::process_output(
                            test_binary,
                            filter,
                            &ecx,
                            bound,
                            non_ignored.as_str(),
                            ignored.as_str(),
                        )?;
                        Ok::<_, CreateTestListError>(info)
                    }
                    FilterBinaryMatch::Mismatch { reason } => {
                        debug!("skipping test binary: {reason}: {}", test_binary.binary_id,);
                        Ok(Self::process_skipped(test_binary, reason))
                    }
                }
            }
        });
        let fut = stream.buffer_unordered(list_threads).try_collect();

        let rust_suites: IdOrdMap<_> = runtime.block_on(fut)?;

        // Ensure that the runtime doesn't stay hanging even if a custom test framework misbehaves
        // (can be an issue on Windows).
        runtime.shutdown_background();

        let test_count = rust_suites
            .iter()
            .map(|suite| suite.status.test_count())
            .sum();

        Ok(Self {
            rust_suites,
            mode: filter.mode(),
            workspace_root,
            env,
            rust_build_meta,
            updated_dylib_path,
            test_count,
            skip_counts: OnceLock::new(),
        })
    }

    /// Creates a new test list with the given binary names and outputs.
    #[cfg(test)]
    fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<
            Item = (RustTestArtifact<'g>, impl AsRef<str>, impl AsRef<str>),
        >,
        workspace_root: Utf8PathBuf,
        rust_build_meta: RustBuildMeta<TestListState>,
        filter: &TestFilterBuilder,
        env: EnvironmentMap,
        ecx: &EvalContext<'_>,
        bound: FilterBound,
    ) -> Result<Self, CreateTestListError> {
        let mut test_count = 0;

        let updated_dylib_path = Self::create_dylib_path(&rust_build_meta)?;

        let rust_suites = test_bin_outputs
            .into_iter()
            .map(|(test_binary, non_ignored, ignored)| {
                let binary_match = filter.filter_binary_match(&test_binary, ecx, bound);
                match binary_match {
                    FilterBinaryMatch::Definite | FilterBinaryMatch::Possible => {
                        debug!(
                            "processing output for binary \
                            (match result is {binary_match:?}): {}",
                            test_binary.binary_id,
                        );
                        let info = Self::process_output(
                            test_binary,
                            filter,
                            ecx,
                            bound,
                            non_ignored.as_ref(),
                            ignored.as_ref(),
                        )?;
                        test_count += info.status.test_count();
                        Ok(info)
                    }
                    FilterBinaryMatch::Mismatch { reason } => {
                        debug!("skipping test binary: {reason}: {}", test_binary.binary_id,);
                        Ok(Self::process_skipped(test_binary, reason))
                    }
                }
            })
            .collect::<Result<IdOrdMap<_>, _>>()?;

        Ok(Self {
            rust_suites,
            mode: filter.mode(),
            workspace_root,
            env,
            rust_build_meta,
            updated_dylib_path,
            test_count,
            skip_counts: OnceLock::new(),
        })
    }

    /// Reconstructs a TestList from archived summary data.
    ///
    /// This is used during replay to reconstruct a TestList without
    /// executing test binaries. The reconstructed TestList provides the
    /// data needed for display through the reporter infrastructure.
    pub fn from_summary(
        graph: &'g PackageGraph,
        summary: &TestListSummary,
        mode: NextestRunMode,
    ) -> Result<Self, TestListFromSummaryError> {
        // Build RustBuildMeta from summary.
        let rust_build_meta = RustBuildMeta::from_summary(summary.rust_build_meta.clone())
            .map_err(TestListFromSummaryError::RustBuildMeta)?;

        // Get the workspace root from the graph.
        let workspace_root = graph.workspace().root().to_path_buf();

        // Construct an empty environment map - we don't need it for replay.
        let env = EnvironmentMap::empty();

        // For replay, we don't need the actual dylib path since we're not executing tests.
        let updated_dylib_path = OsString::new();

        // Build test suites from the summary.
        let mut rust_suites = IdOrdMap::new();
        let mut test_count = 0;

        for (binary_id, suite_summary) in &summary.rust_suites {
            // Look up the package in the graph by package_id.
            let package_id = PackageId::new(suite_summary.binary.package_id.clone());
            let package = graph.metadata(&package_id).map_err(|_| {
                TestListFromSummaryError::PackageNotFound {
                    name: suite_summary.package_name.clone(),
                    package_id: suite_summary.binary.package_id.clone(),
                }
            })?;

            // Determine the status based on the summary.
            let status = if suite_summary.status == RustTestSuiteStatusSummary::SKIPPED {
                RustTestSuiteStatus::Skipped {
                    reason: BinaryMismatchReason::Expression,
                }
            } else if suite_summary.status == RustTestSuiteStatusSummary::SKIPPED_DEFAULT_FILTER {
                RustTestSuiteStatus::Skipped {
                    reason: BinaryMismatchReason::DefaultSet,
                }
            } else {
                // Build test cases from the summary (only for listed suites).
                let test_cases: IdOrdMap<RustTestCase> = suite_summary
                    .test_cases
                    .iter()
                    .map(|(name, info)| RustTestCase {
                        name: name.clone(),
                        test_info: info.clone(),
                    })
                    .collect();

                test_count += test_cases.len();

                // LISTED or any other status should be treated as listed.
                RustTestSuiteStatus::Listed {
                    test_cases: DebugIgnore(test_cases),
                }
            };

            let suite = RustTestSuite {
                binary_id: binary_id.clone(),
                binary_path: suite_summary.binary.binary_path.clone(),
                package,
                binary_name: suite_summary.binary.binary_name.clone(),
                kind: suite_summary.binary.kind.clone(),
                non_test_binaries: BTreeSet::new(), // Not stored in summary.
                cwd: suite_summary.cwd.clone(),
                build_platform: suite_summary.binary.build_platform,
                status,
            };

            let _ = rust_suites.insert_unique(suite);
        }

        Ok(Self {
            rust_suites,
            mode,
            workspace_root,
            env,
            rust_build_meta,
            updated_dylib_path,
            test_count,
            skip_counts: OnceLock::new(),
        })
    }

    /// Returns the total number of tests across all binaries.
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Returns the mode nextest is running in.
    pub fn mode(&self) -> NextestRunMode {
        self.mode
    }

    /// Returns the Rust build-related metadata for this test list.
    pub fn rust_build_meta(&self) -> &RustBuildMeta<TestListState> {
        &self.rust_build_meta
    }

    /// Returns the total number of skipped tests.
    pub fn skip_counts(&self) -> &SkipCounts {
        self.skip_counts.get_or_init(|| {
            let mut skipped_tests_rerun = 0;
            let mut skipped_tests_non_benchmark = 0;
            let mut skipped_tests_default_filter = 0;
            let skipped_tests = self
                .iter_tests()
                .filter(|instance| match instance.test_info.filter_match {
                    FilterMatch::Mismatch {
                        reason: MismatchReason::RerunAlreadyPassed,
                    } => {
                        skipped_tests_rerun += 1;
                        true
                    }
                    FilterMatch::Mismatch {
                        reason: MismatchReason::NotBenchmark,
                    } => {
                        skipped_tests_non_benchmark += 1;
                        true
                    }
                    FilterMatch::Mismatch {
                        reason: MismatchReason::DefaultFilter,
                    } => {
                        skipped_tests_default_filter += 1;
                        true
                    }
                    FilterMatch::Mismatch { .. } => true,
                    FilterMatch::Matches => false,
                })
                .count();

            let mut skipped_binaries_default_filter = 0;
            let skipped_binaries = self
                .rust_suites
                .iter()
                .filter(|suite| match suite.status {
                    RustTestSuiteStatus::Skipped {
                        reason: BinaryMismatchReason::DefaultSet,
                    } => {
                        skipped_binaries_default_filter += 1;
                        true
                    }
                    RustTestSuiteStatus::Skipped { .. } => true,
                    RustTestSuiteStatus::Listed { .. } => false,
                })
                .count();

            SkipCounts {
                skipped_tests,
                skipped_tests_rerun,
                skipped_tests_non_benchmark,
                skipped_tests_default_filter,
                skipped_binaries,
                skipped_binaries_default_filter,
            }
        })
    }

    /// Returns the total number of tests that aren't skipped.
    ///
    /// It is always the case that `run_count + skip_count == test_count`.
    pub fn run_count(&self) -> usize {
        self.test_count - self.skip_counts().skipped_tests
    }

    /// Returns the total number of binaries that contain tests.
    pub fn binary_count(&self) -> usize {
        self.rust_suites.len()
    }

    /// Returns the total number of binaries that were listed (not skipped).
    pub fn listed_binary_count(&self) -> usize {
        self.binary_count() - self.skip_counts().skipped_binaries
    }

    /// Returns the mapped workspace root.
    pub fn workspace_root(&self) -> &Utf8Path {
        &self.workspace_root
    }

    /// Returns the environment variables to be used when running tests.
    pub fn cargo_env(&self) -> &EnvironmentMap {
        &self.env
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
            .map(|test_suite| {
                let (status, test_cases) = test_suite.status.to_summary();
                let testsuite = RustTestSuiteSummary {
                    package_name: test_suite.package.name().to_owned(),
                    binary: RustTestBinarySummary {
                        binary_name: test_suite.binary_name.clone(),
                        package_id: test_suite.package.id().repr().to_owned(),
                        kind: test_suite.kind.clone(),
                        binary_path: test_suite.binary_path.clone(),
                        binary_id: test_suite.binary_id.clone(),
                        build_platform: test_suite.build_platform,
                    },
                    cwd: test_suite.cwd.clone(),
                    status,
                    test_cases,
                };
                (test_suite.binary_id.clone(), testsuite)
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
        writer: &mut dyn WriteStr,
        colorize: bool,
    ) -> Result<(), WriteTestListError> {
        match output_format {
            OutputFormat::Human { verbose } => self
                .write_human(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Oneline { verbose } => self
                .write_oneline(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Serializable(format) => format.to_writer(&self.to_summary(), writer),
        }
    }

    /// Iterates over all the test suites.
    pub fn iter(&self) -> impl Iterator<Item = &RustTestSuite<'_>> + '_ {
        self.rust_suites.iter()
    }

    /// Looks up a test suite by binary ID.
    pub fn get_suite(&self, binary_id: &RustBinaryId) -> Option<&RustTestSuite<'_>> {
        self.rust_suites.get(binary_id)
    }

    /// Iterates over the list of tests, returning the path and test name.
    pub fn iter_tests(&self) -> impl Iterator<Item = TestInstance<'_>> + '_ {
        self.rust_suites.iter().flat_map(|test_suite| {
            test_suite
                .status
                .test_cases()
                .map(move |case| TestInstance::new(case, test_suite))
        })
    }

    /// Produces a priority queue of tests based on the given profile.
    pub fn to_priority_queue(
        &'g self,
        profile: &'g EvaluatableProfile<'g>,
    ) -> TestPriorityQueue<'g> {
        TestPriorityQueue::new(self, profile)
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String, WriteTestListError> {
        let mut s = String::with_capacity(1024);
        self.write(output_format, &mut s, false)?;
        Ok(s)
    }

    // ---
    // Helper methods
    // ---

    /// Creates an empty test list.
    ///
    /// This is primarily for use in tests where a placeholder test list is needed.
    pub fn empty() -> Self {
        Self {
            test_count: 0,
            mode: NextestRunMode::Test,
            workspace_root: Utf8PathBuf::new(),
            rust_build_meta: RustBuildMeta::empty(),
            env: EnvironmentMap::empty(),
            updated_dylib_path: OsString::new(),
            rust_suites: IdOrdMap::new(),
            skip_counts: OnceLock::new(),
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
        ecx: &EvalContext<'_>,
        bound: FilterBound,
        non_ignored: impl AsRef<str>,
        ignored: impl AsRef<str>,
    ) -> Result<RustTestSuite<'g>, CreateTestListError> {
        let mut test_cases = IdOrdMap::new();

        // Treat ignored and non-ignored as separate sets of single filters, so that partitioning
        // based on one doesn't affect the other.
        let mut non_ignored_filter = filter.build();
        for (test_name, kind) in Self::parse(&test_binary.binary_id, non_ignored.as_ref())? {
            let name = TestCaseName::new(test_name);
            let filter_match =
                non_ignored_filter.filter_match(&test_binary, &name, &kind, ecx, bound, false);
            test_cases.insert_overwrite(RustTestCase {
                name,
                test_info: RustTestCaseSummary {
                    kind: Some(kind),
                    ignored: false,
                    filter_match,
                },
            });
        }

        let mut ignored_filter = filter.build();
        for (test_name, kind) in Self::parse(&test_binary.binary_id, ignored.as_ref())? {
            // Note that libtest prints out:
            // * just ignored tests if --ignored is passed in
            // * all tests, both ignored and non-ignored, if --ignored is not passed in
            // Adding ignored tests after non-ignored ones makes everything resolve correctly.
            let name = TestCaseName::new(test_name);
            let filter_match =
                ignored_filter.filter_match(&test_binary, &name, &kind, ecx, bound, true);
            test_cases.insert_overwrite(RustTestCase {
                name,
                test_info: RustTestCaseSummary {
                    kind: Some(kind),
                    ignored: true,
                    filter_match,
                },
            });
        }

        Ok(test_binary.into_test_suite(RustTestSuiteStatus::Listed {
            test_cases: test_cases.into(),
        }))
    }

    fn process_skipped(
        test_binary: RustTestArtifact<'g>,
        reason: BinaryMismatchReason,
    ) -> RustTestSuite<'g> {
        test_binary.into_test_suite(RustTestSuiteStatus::Skipped { reason })
    }

    /// Parses the output of --list --message-format terse and returns a sorted list.
    fn parse<'a>(
        binary_id: &'a RustBinaryId,
        list_output: &'a str,
    ) -> Result<Vec<(&'a str, RustTestKind)>, CreateTestListError> {
        let mut list = parse_list_lines(binary_id, list_output).collect::<Result<Vec<_>, _>>()?;
        list.sort_unstable();
        Ok(list)
    }

    /// Writes this test list out in a human-friendly format.
    pub fn write_human(
        &self,
        writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
        self.write_human_impl(None, writer, verbose, colorize)
    }

    /// Writes this test list out in a human-friendly format with the given filter.
    pub(crate) fn write_human_with_filter(
        &self,
        filter: &TestListDisplayFilter<'_>,
        writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
        self.write_human_impl(Some(filter), writer, verbose, colorize)
    }

    fn write_human_impl(
        &self,
        filter: Option<&TestListDisplayFilter<'_>>,
        mut writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }

        for info in &self.rust_suites {
            let matcher = match filter {
                Some(filter) => match filter.matcher_for(&info.binary_id) {
                    Some(matcher) => matcher,
                    None => continue,
                },
                None => DisplayFilterMatcher::All,
            };

            // Skip this binary if there are no tests within it that will be run, and this isn't
            // verbose output.
            if !verbose
                && info
                    .status
                    .test_cases()
                    .all(|case| !case.test_info.filter_match.is_match())
            {
                continue;
            }

            writeln!(writer, "{}:", info.binary_id.style(styles.binary_id))?;
            if verbose {
                writeln!(
                    writer,
                    "  {} {}",
                    "bin:".style(styles.field),
                    info.binary_path
                )?;
                writeln!(writer, "  {} {}", "cwd:".style(styles.field), info.cwd)?;
                writeln!(
                    writer,
                    "  {} {}",
                    "build platform:".style(styles.field),
                    info.build_platform,
                )?;
            }

            let mut indented = indented(writer).with_str("    ");

            match &info.status {
                RustTestSuiteStatus::Listed { test_cases } => {
                    let matching_tests: Vec<_> = test_cases
                        .iter()
                        .filter(|case| matcher.is_match(&case.name))
                        .collect();
                    if matching_tests.is_empty() {
                        writeln!(indented, "(no tests)")?;
                    } else {
                        for case in matching_tests {
                            match (verbose, case.test_info.filter_match.is_match()) {
                                (_, true) => {
                                    write_test_name(&case.name, &styles, &mut indented)?;
                                    writeln!(indented)?;
                                }
                                (true, false) => {
                                    write_test_name(&case.name, &styles, &mut indented)?;
                                    writeln!(indented, " (skipped)")?;
                                }
                                (false, false) => {
                                    // Skip printing this test entirely if it isn't a match.
                                }
                            }
                        }
                    }
                }
                RustTestSuiteStatus::Skipped { reason } => {
                    writeln!(indented, "(test binary {reason}, skipped)")?;
                }
            }

            writer = indented.into_inner();
        }
        Ok(())
    }

    /// Writes this test list out in a one-line-per-test format.
    pub fn write_oneline(
        &self,
        writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }

        for info in &self.rust_suites {
            match &info.status {
                RustTestSuiteStatus::Listed { test_cases } => {
                    for case in test_cases.iter() {
                        let is_match = case.test_info.filter_match.is_match();
                        // Skip tests that don't match the filter (unless verbose).
                        if !verbose && !is_match {
                            continue;
                        }

                        write!(writer, "{} ", info.binary_id.style(styles.binary_id))?;
                        write_test_name(&case.name, &styles, writer)?;

                        if verbose {
                            write!(
                                writer,
                                " [{}{}] [{}{}] [{}{}]{}",
                                "bin: ".style(styles.field),
                                info.binary_path,
                                "cwd: ".style(styles.field),
                                info.cwd,
                                "build platform: ".style(styles.field),
                                info.build_platform,
                                if is_match { "" } else { " (skipped)" },
                            )?;
                        }

                        writeln!(writer)?;
                    }
                }
                RustTestSuiteStatus::Skipped { .. } => {
                    // Skip binaries that were not listed.
                }
            }
        }

        Ok(())
    }
}

fn parse_list_lines<'a>(
    binary_id: &'a RustBinaryId,
    list_output: &'a str,
) -> impl Iterator<Item = Result<(&'a str, RustTestKind), CreateTestListError>> + 'a + use<'a> {
    // The output is in the form:
    // <test name>: test
    // <test name>: test
    // ...

    list_output
        .lines()
        .map(move |line| match line.strip_suffix(": test") {
            Some(test_name) => Ok((test_name, RustTestKind::TEST)),
            None => match line.strip_suffix(": benchmark") {
                Some(test_name) => Ok((test_name, RustTestKind::BENCH)),
                None => Err(CreateTestListError::parse_line(
                    binary_id.clone(),
                    format!(
                        "line {line:?} did not end with the string \": test\" or \": benchmark\""
                    ),
                    list_output,
                )),
            },
        })
}

/// Profile implementation for test lists.
pub trait ListProfile {
    /// Returns the evaluation context.
    fn filterset_ecx(&self) -> EvalContext<'_>;

    /// Returns list-time settings for a test binary.
    fn list_settings_for(&self, query: &BinaryQuery<'_>) -> ListSettings<'_>;
}

impl<'g> ListProfile for EvaluatableProfile<'g> {
    fn filterset_ecx(&self) -> EvalContext<'_> {
        self.filterset_ecx()
    }

    fn list_settings_for(&self, query: &BinaryQuery<'_>) -> ListSettings<'_> {
        self.list_settings_for(query)
    }
}

/// A test list that has been sorted and has had priorities applied to it.
pub struct TestPriorityQueue<'a> {
    tests: Vec<TestInstanceWithSettings<'a>>,
}

impl<'a> TestPriorityQueue<'a> {
    fn new(test_list: &'a TestList<'a>, profile: &'a EvaluatableProfile<'a>) -> Self {
        let mode = test_list.mode();
        let mut tests = test_list
            .iter_tests()
            .map(|instance| {
                let settings = profile.settings_for(mode, &instance.to_test_query());
                TestInstanceWithSettings { instance, settings }
            })
            .collect::<Vec<_>>();
        // Note: this is a stable sort so that tests with the same priority are
        // sorted by what `iter_tests` produced.
        tests.sort_by_key(|test| test.settings.priority());

        Self { tests }
    }
}

impl<'a> IntoIterator for TestPriorityQueue<'a> {
    type Item = TestInstanceWithSettings<'a>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.tests.into_iter()
    }
}

/// A test instance, along with computed settings from a profile.
///
/// Returned from [`TestPriorityQueue`].
#[derive(Debug)]
pub struct TestInstanceWithSettings<'a> {
    /// The test instance.
    pub instance: TestInstance<'a>,

    /// The settings for this test.
    pub settings: TestSettings<'a>,
}

/// A suite of tests within a single Rust test binary.
///
/// This is a representation of [`nextest_metadata::RustTestSuiteSummary`] used internally by the runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustTestSuite<'g> {
    /// A unique identifier for this binary.
    pub binary_id: RustBinaryId,

    /// The path to the binary.
    pub binary_path: Utf8PathBuf,

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

impl IdOrdItem for RustTestSuite<'_> {
    type Key<'a>
        = &'a RustBinaryId
    where
        Self: 'a;

    fn key(&self) -> Self::Key<'_> {
        &self.binary_id
    }

    id_upcast!();
}

impl RustTestArtifact<'_> {
    /// Run this binary with and without --ignored and get the corresponding outputs.
    async fn exec(
        &self,
        lctx: &LocalExecuteContext<'_>,
        list_settings: &ListSettings<'_>,
        target_runner: &TargetRunner,
    ) -> Result<(String, String), CreateTestListError> {
        // This error situation has been known to happen with reused builds. It produces
        // a really terrible and confusing "file not found" message if allowed to prceed.
        if !self.cwd.is_dir() {
            return Err(CreateTestListError::CwdIsNotDir {
                binary_id: self.binary_id.clone(),
                cwd: self.cwd.clone(),
            });
        }
        let platform_runner = target_runner.for_build_platform(self.build_platform);

        let non_ignored = self.exec_single(false, lctx, list_settings, platform_runner);
        let ignored = self.exec_single(true, lctx, list_settings, platform_runner);

        let (non_ignored_out, ignored_out) = futures::future::join(non_ignored, ignored).await;
        Ok((non_ignored_out?, ignored_out?))
    }

    async fn exec_single(
        &self,
        ignored: bool,
        lctx: &LocalExecuteContext<'_>,
        list_settings: &ListSettings<'_>,
        runner: Option<&PlatformRunner>,
    ) -> Result<String, CreateTestListError> {
        let mut cli = TestCommandCli::default();
        cli.apply_wrappers(
            list_settings.list_wrapper(),
            runner,
            lctx.workspace_root,
            &lctx.rust_build_meta.target_directory,
        );
        cli.push(self.binary_path.as_str());

        cli.extend(["--list", "--format", "terse"]);
        if ignored {
            cli.push("--ignored");
        }

        let cmd = TestCommand::new(
            lctx,
            cli.program
                .clone()
                .expect("at least one argument passed in")
                .into_owned(),
            &cli.args,
            &self.cwd,
            &self.package,
            &self.non_test_binaries,
            &Interceptor::None, // Interceptors are not used during the test list phase.
        );

        let output =
            cmd.wait_with_output()
                .await
                .map_err(|error| CreateTestListError::CommandExecFail {
                    binary_id: self.binary_id.clone(),
                    command: cli.to_owned_cli(),
                    error,
                })?;

        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|err| CreateTestListError::CommandNonUtf8 {
                binary_id: self.binary_id.clone(),
                command: cli.to_owned_cli(),
                stdout: err.into_bytes(),
                stderr: output.stderr,
            })
        } else {
            Err(CreateTestListError::CommandFail {
                binary_id: self.binary_id.clone(),
                command: cli.to_owned_cli(),
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
        test_cases: DebugIgnore<IdOrdMap<RustTestCase>>,
    },

    /// The test suite was not executed.
    Skipped {
        /// The reason why the test suite was skipped.
        reason: BinaryMismatchReason,
    },
}

static EMPTY_TEST_CASE_MAP: IdOrdMap<RustTestCase> = IdOrdMap::new();

impl RustTestSuiteStatus {
    /// Returns the number of test cases within this suite.
    pub fn test_count(&self) -> usize {
        match self {
            RustTestSuiteStatus::Listed { test_cases } => test_cases.len(),
            RustTestSuiteStatus::Skipped { .. } => 0,
        }
    }

    /// Returns the list of test cases within this suite.
    pub fn test_cases(&self) -> impl Iterator<Item = &RustTestCase> + '_ {
        match self {
            RustTestSuiteStatus::Listed { test_cases } => test_cases.iter(),
            RustTestSuiteStatus::Skipped { .. } => {
                // Return an empty test case.
                EMPTY_TEST_CASE_MAP.iter()
            }
        }
    }

    /// Converts this status to its serializable form.
    pub fn to_summary(
        &self,
    ) -> (
        RustTestSuiteStatusSummary,
        BTreeMap<TestCaseName, RustTestCaseSummary>,
    ) {
        match self {
            Self::Listed { test_cases } => (
                RustTestSuiteStatusSummary::LISTED,
                test_cases
                    .iter()
                    .cloned()
                    .map(|case| (case.name, case.test_info))
                    .collect(),
            ),
            Self::Skipped {
                reason: BinaryMismatchReason::Expression,
            } => (RustTestSuiteStatusSummary::SKIPPED, BTreeMap::new()),
            Self::Skipped {
                reason: BinaryMismatchReason::DefaultSet,
            } => (
                RustTestSuiteStatusSummary::SKIPPED_DEFAULT_FILTER,
                BTreeMap::new(),
            ),
        }
    }
}

/// A single test case within a test suite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustTestCase {
    /// The name of the test.
    pub name: TestCaseName,

    /// Information about the test.
    pub test_info: RustTestCaseSummary,
}

impl IdOrdItem for RustTestCase {
    type Key<'a> = &'a TestCaseName;
    fn key(&self) -> Self::Key<'_> {
        &self.name
    }
    id_upcast!();
}

/// Represents a single test with its associated binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestInstance<'a> {
    /// The name of the test.
    pub name: &'a TestCaseName,

    /// Information about the test suite.
    pub suite_info: &'a RustTestSuite<'a>,

    /// Information about the test.
    pub test_info: &'a RustTestCaseSummary,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub(crate) fn new(case: &'a RustTestCase, suite_info: &'a RustTestSuite) -> Self {
        Self {
            name: &case.name,
            suite_info,
            test_info: &case.test_info,
        }
    }

    /// Return an identifier for test instances, including being able to sort
    /// them.
    #[inline]
    pub fn id(&self) -> TestInstanceId<'a> {
        TestInstanceId {
            binary_id: &self.suite_info.binary_id,
            test_name: self.name,
        }
    }

    /// Returns the corresponding [`TestQuery`] for this `TestInstance`.
    pub fn to_test_query(&self) -> TestQuery<'a> {
        TestQuery {
            binary_query: BinaryQuery {
                package_id: self.suite_info.package.id(),
                binary_id: &self.suite_info.binary_id,
                kind: &self.suite_info.kind,
                binary_name: &self.suite_info.binary_name,
                platform: convert_build_platform(self.suite_info.build_platform),
            },
            test_name: self.name,
        }
    }

    /// Creates the command for this test instance.
    pub(crate) fn make_command(
        &self,
        ctx: &TestExecuteContext<'_>,
        test_list: &TestList<'_>,
        wrapper_script: Option<&WrapperScriptConfig>,
        extra_args: &[String],
        interceptor: &Interceptor,
    ) -> TestCommand {
        // TODO: non-rust tests
        let cli = self.compute_cli(ctx, test_list, wrapper_script, extra_args);

        let lctx = LocalExecuteContext {
            phase: TestCommandPhase::Run,
            workspace_root: test_list.workspace_root(),
            rust_build_meta: &test_list.rust_build_meta,
            double_spawn: ctx.double_spawn,
            dylib_path: test_list.updated_dylib_path(),
            profile_name: ctx.profile_name,
            env: &test_list.env,
        };

        TestCommand::new(
            &lctx,
            cli.program
                .expect("at least one argument is guaranteed")
                .into_owned(),
            &cli.args,
            &self.suite_info.cwd,
            &self.suite_info.package,
            &self.suite_info.non_test_binaries,
            interceptor,
        )
    }

    pub(crate) fn command_line(
        &self,
        ctx: &TestExecuteContext<'_>,
        test_list: &TestList<'_>,
        wrapper_script: Option<&WrapperScriptConfig>,
        extra_args: &[String],
    ) -> Vec<String> {
        self.compute_cli(ctx, test_list, wrapper_script, extra_args)
            .to_owned_cli()
    }

    fn compute_cli(
        &self,
        ctx: &'a TestExecuteContext<'_>,
        test_list: &TestList<'_>,
        wrapper_script: Option<&'a WrapperScriptConfig>,
        extra_args: &'a [String],
    ) -> TestCommandCli<'a> {
        let platform_runner = ctx
            .target_runner
            .for_build_platform(self.suite_info.build_platform);

        let mut cli = TestCommandCli::default();
        cli.apply_wrappers(
            wrapper_script,
            platform_runner,
            test_list.workspace_root(),
            &test_list.rust_build_meta().target_directory,
        );
        cli.push(self.suite_info.binary_path.as_str());

        cli.extend(["--exact", self.name.as_str(), "--nocapture"]);
        if self.test_info.ignored {
            cli.push("--ignored");
        }
        match test_list.mode() {
            NextestRunMode::Test => {}
            NextestRunMode::Benchmark => {
                cli.push("--bench");
            }
        }
        cli.extend(extra_args.iter().map(String::as_str));

        cli
    }
}

#[derive(Clone, Debug, Default)]
struct TestCommandCli<'a> {
    program: Option<Cow<'a, str>>,
    args: Vec<Cow<'a, str>>,
}

impl<'a> TestCommandCli<'a> {
    fn apply_wrappers(
        &mut self,
        wrapper_script: Option<&'a WrapperScriptConfig>,
        platform_runner: Option<&'a PlatformRunner>,
        workspace_root: &Utf8Path,
        target_dir: &Utf8Path,
    ) {
        // Apply the wrapper script if it's enabled.
        if let Some(wrapper) = wrapper_script {
            match wrapper.target_runner {
                WrapperScriptTargetRunner::Ignore => {
                    // Ignore the platform runner.
                    self.push(wrapper.command.program(workspace_root, target_dir));
                    self.extend(wrapper.command.args.iter().map(String::as_str));
                }
                WrapperScriptTargetRunner::AroundWrapper => {
                    // Platform runner goes first.
                    if let Some(runner) = platform_runner {
                        self.push(runner.binary());
                        self.extend(runner.args());
                    }
                    self.push(wrapper.command.program(workspace_root, target_dir));
                    self.extend(wrapper.command.args.iter().map(String::as_str));
                }
                WrapperScriptTargetRunner::WithinWrapper => {
                    // Wrapper script goes first.
                    self.push(wrapper.command.program(workspace_root, target_dir));
                    self.extend(wrapper.command.args.iter().map(String::as_str));
                    if let Some(runner) = platform_runner {
                        self.push(runner.binary());
                        self.extend(runner.args());
                    }
                }
                WrapperScriptTargetRunner::OverridesWrapper => {
                    // Target runner overrides wrapper.
                    if let Some(runner) = platform_runner {
                        self.push(runner.binary());
                        self.extend(runner.args());
                    }
                }
            }
        } else {
            // If no wrapper script is enabled, use the platform runner.
            if let Some(runner) = platform_runner {
                self.push(runner.binary());
                self.extend(runner.args());
            }
        }
    }

    fn push(&mut self, arg: impl Into<Cow<'a, str>>) {
        if self.program.is_none() {
            self.program = Some(arg.into());
        } else {
            self.args.push(arg.into());
        }
    }

    fn extend(&mut self, args: impl IntoIterator<Item = &'a str>) {
        for arg in args {
            self.push(arg);
        }
    }

    fn to_owned_cli(&self) -> Vec<String> {
        let mut owned_cli = Vec::new();
        if let Some(program) = &self.program {
            owned_cli.push(program.to_string());
        }
        owned_cli.extend(self.args.iter().map(|arg| arg.clone().into_owned()));
        owned_cli
    }
}

/// A key for identifying and sorting test instances.
///
/// Returned by [`TestInstance::id`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub struct TestInstanceId<'a> {
    /// The binary ID.
    pub binary_id: &'a RustBinaryId,

    /// The name of the test.
    pub test_name: &'a TestCaseName,
}

impl TestInstanceId<'_> {
    /// Return the attempt ID corresponding to this test instance.
    ///
    /// This string uniquely identifies a single test attempt.
    pub fn attempt_id(
        &self,
        run_id: ReportUuid,
        stress_index: Option<u32>,
        attempt: u32,
    ) -> String {
        let mut out = String::new();
        swrite!(out, "{run_id}:{}", self.binary_id);
        if let Some(stress_index) = stress_index {
            swrite!(out, "@stress-{}", stress_index);
        }
        swrite!(out, "${}", self.test_name);
        if attempt > 1 {
            swrite!(out, "#{attempt}");
        }

        out
    }
}

impl fmt::Display for TestInstanceId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.binary_id, self.test_name)
    }
}

/// An owned version of [`TestInstanceId`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OwnedTestInstanceId {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    #[serde(rename = "name")]
    pub test_name: TestCaseName,
}

impl OwnedTestInstanceId {
    /// Borrow this as a [`TestInstanceId`].
    pub fn as_ref(&self) -> TestInstanceId<'_> {
        TestInstanceId {
            binary_id: &self.binary_id,
            test_name: &self.test_name,
        }
    }
}

impl TestInstanceId<'_> {
    /// Convert this to an owned version.
    pub fn to_owned(&self) -> OwnedTestInstanceId {
        OwnedTestInstanceId {
            binary_id: self.binary_id.clone(),
            test_name: self.test_name.clone(),
        }
    }
}

/// Trait to allow retrieving data from a set of [`OwnedTestInstanceId`] using a
/// [`TestInstanceId`].
///
/// This is an implementation of the [borrow-complex-key-example
/// pattern](https://github.com/sunshowers-code/borrow-complex-key-example).
pub trait TestInstanceIdKey {
    /// Converts self to a [`TestInstanceId`].
    fn key<'k>(&'k self) -> TestInstanceId<'k>;
}

impl TestInstanceIdKey for OwnedTestInstanceId {
    fn key<'k>(&'k self) -> TestInstanceId<'k> {
        TestInstanceId {
            binary_id: &self.binary_id,
            test_name: &self.test_name,
        }
    }
}

impl<'a> TestInstanceIdKey for TestInstanceId<'a> {
    fn key<'k>(&'k self) -> TestInstanceId<'k> {
        *self
    }
}

impl<'a> Borrow<dyn TestInstanceIdKey + 'a> for OwnedTestInstanceId {
    fn borrow(&self) -> &(dyn TestInstanceIdKey + 'a) {
        self
    }
}

impl<'a> PartialEq for dyn TestInstanceIdKey + 'a {
    fn eq(&self, other: &(dyn TestInstanceIdKey + 'a)) -> bool {
        self.key() == other.key()
    }
}

impl<'a> Eq for dyn TestInstanceIdKey + 'a {}

impl<'a> PartialOrd for dyn TestInstanceIdKey + 'a {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for dyn TestInstanceIdKey + 'a {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl<'a> Hash for dyn TestInstanceIdKey + 'a {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

/// Context required for test execution.
#[derive(Clone, Debug)]
pub struct TestExecuteContext<'a> {
    /// The name of the profile.
    pub profile_name: &'a str,

    /// Double-spawn info.
    pub double_spawn: &'a DoubleSpawnInfo,

    /// Target runner.
    pub target_runner: &'a TargetRunner,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cargo_config::{TargetDefinitionLocation, TargetTriple, TargetTripleSource},
        config::scripts::{ScriptCommand, ScriptCommandRelativeTo},
        list::SerializableFormat,
        platform::{BuildPlatforms, HostPlatform, PlatformLibdir, TargetPlatform},
        target_runner::PlatformRunnerSource,
        test_filter::{RunIgnored, TestFilterPatterns},
    };
    use guppy::CargoMetadata;
    use iddqd::id_ord_map;
    use indoc::indoc;
    use nextest_filtering::{CompiledExpr, Filterset, FiltersetKind, ParseContext};
    use nextest_metadata::{FilterMatch, MismatchReason, PlatformLibdirUnavailable, RustTestKind};
    use pretty_assertions::assert_eq;
    use std::{hash::DefaultHasher, sync::LazyLock};
    use target_spec::Platform;
    use test_strategy::proptest;

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

        let cx = ParseContext::new(&PACKAGE_GRAPH_FIXTURE);

        let test_filter = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            TestFilterPatterns::default(),
            // Test against the platform() predicate because this is the most important one here.
            vec![
                Filterset::parse("platform(target)".to_owned(), &cx, FiltersetKind::Test).unwrap(),
            ],
        )
        .unwrap();
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
            location: TargetDefinitionLocation::Builtin,
        };
        let fake_host_libdir = "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib";
        let build_platforms = BuildPlatforms {
            host: HostPlatform {
                platform: TargetTriple::x86_64_unknown_linux_gnu().platform,
                libdir: PlatformLibdir::Available(fake_host_libdir.into()),
            },
            target: Some(TargetPlatform {
                triple: fake_triple,
                // Test an unavailable libdir.
                libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::new_const("test")),
            }),
        };

        let fake_env = EnvironmentMap::empty();
        let rust_build_meta =
            RustBuildMeta::new("/fake", build_platforms).map_paths(&PathMapper::noop());
        let ecx = EvalContext {
            default_filter: &CompiledExpr::ALL,
        };
        let test_list = TestList::new_with_outputs(
            [
                (test_binary, &non_ignored_output, &ignored_output),
                (
                    skipped_binary,
                    &"should-not-show-up-stdout",
                    &"should-not-show-up-stderr",
                ),
            ],
            Utf8PathBuf::from("/fake/path"),
            rust_build_meta,
            &test_filter,
            fake_env,
            &ecx,
            FilterBound::All,
        )
        .expect("valid output");
        assert_eq!(
            test_list.rust_suites,
            id_ord_map! {
                RustTestSuite {
                    status: RustTestSuiteStatus::Listed {
                        test_cases: id_ord_map! {
                            RustTestCase {
                                name: TestCaseName::new("tests::foo::test_bar"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::TEST),
                                    ignored: false,
                                    filter_match: FilterMatch::Matches,
                                },
                            },
                            RustTestCase {
                                name: TestCaseName::new("tests::baz::test_quux"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::TEST),
                                    ignored: false,
                                    filter_match: FilterMatch::Matches,
                                },
                            },
                            RustTestCase {
                                name: TestCaseName::new("benches::bench_foo"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::BENCH),
                                    ignored: false,
                                    filter_match: FilterMatch::Matches,
                                },
                            },
                            RustTestCase {
                                name: TestCaseName::new("tests::ignored::test_bar"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::TEST),
                                    ignored: true,
                                    filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                                },
                            },
                            RustTestCase {
                                name: TestCaseName::new("tests::baz::test_ignored"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::TEST),
                                    ignored: true,
                                    filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                                },
                            },
                            RustTestCase {
                                name: TestCaseName::new("benches::ignored_bench_foo"),
                                test_info: RustTestCaseSummary {
                                    kind: Some(RustTestKind::BENCH),
                                    ignored: true,
                                    filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                                },
                            },
                        }.into(),
                    },
                    cwd: fake_cwd.clone(),
                    build_platform: BuildPlatform::Target,
                    package: package_metadata(),
                    binary_name: fake_binary_name,
                    binary_id: fake_binary_id,
                    binary_path: "/fake/binary".into(),
                    kind: RustTestBinaryKind::LIB,
                    non_test_binaries: BTreeSet::new(),
                },
                RustTestSuite {
                    status: RustTestSuiteStatus::Skipped {
                        reason: BinaryMismatchReason::Expression,
                    },
                    cwd: fake_cwd,
                    build_platform: BuildPlatform::Host,
                    package: package_metadata(),
                    binary_name: skipped_binary_name,
                    binary_id: skipped_binary_id,
                    binary_path: "/fake/skipped-binary".into(),
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
                (test binary didn't match filtersets, skipped)
        "};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "rust-build-meta": {
                "target-directory": "/fake",
                "base-output-directories": [],
                "non-test-binaries": {},
                "build-script-out-dirs": {},
                "linked-paths": [],
                "platforms": {
                  "host": {
                    "platform": {
                      "triple": "x86_64-unknown-linux-gnu",
                      "target-features": "unknown"
                    },
                    "libdir": {
                      "status": "available",
                      "path": "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib"
                    }
                  },
                  "targets": [
                    {
                      "platform": {
                        "triple": "aarch64-unknown-linux-gnu",
                        "target-features": "unknown"
                      },
                      "libdir": {
                        "status": "unavailable",
                        "reason": "test"
                      }
                    }
                  ]
                },
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
                      "kind": "bench",
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "benches::ignored_bench_foo": {
                      "kind": "bench",
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_ignored": {
                      "kind": "test",
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_quux": {
                      "kind": "test",
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::foo::test_bar": {
                      "kind": "test",
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::ignored::test_bar": {
                      "kind": "test",
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
        static EXPECTED_ONELINE: &str = indoc! {"
            fake-package::fake-binary benches::bench_foo
            fake-package::fake-binary tests::baz::test_quux
            fake-package::fake-binary tests::foo::test_bar
        "};
        static EXPECTED_ONELINE_VERBOSE: &str = indoc! {"
            fake-package::fake-binary benches::bench_foo [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target]
            fake-package::fake-binary benches::ignored_bench_foo [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target] (skipped)
            fake-package::fake-binary tests::baz::test_ignored [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target] (skipped)
            fake-package::fake-binary tests::baz::test_quux [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target]
            fake-package::fake-binary tests::foo::test_bar [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target]
            fake-package::fake-binary tests::ignored::test_bar [bin: /fake/binary] [cwd: /fake/cwd] [build platform: target] (skipped)
        "};

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
        assert_eq!(
            test_list
                .to_string(OutputFormat::Oneline { verbose: false })
                .expect("oneline succeeded"),
            EXPECTED_ONELINE
        );
        assert_eq!(
            test_list
                .to_string(OutputFormat::Oneline { verbose: true })
                .expect("oneline verbose succeeded"),
            EXPECTED_ONELINE_VERBOSE
        );
    }

    #[test]
    fn apply_wrappers_examples() {
        cfg_if::cfg_if! {
            if #[cfg(windows)]
            {
                let workspace_root = Utf8Path::new("D:\\workspace\\root");
                let target_dir = Utf8Path::new("C:\\foo\\bar");
            } else {
                let workspace_root = Utf8Path::new("/workspace/root");
                let target_dir = Utf8Path::new("/foo/bar");
            }
        };

        // Test with no wrappers
        {
            let mut cli_no_wrappers = TestCommandCli::default();
            cli_no_wrappers.apply_wrappers(None, None, workspace_root, target_dir);
            cli_no_wrappers.extend(["binary", "arg"]);
            assert_eq!(cli_no_wrappers.to_owned_cli(), vec!["binary", "arg"]);
        }

        // Test with platform runner only
        {
            let runner = PlatformRunner::debug_new(
                "runner".into(),
                Vec::new(),
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let mut cli_runner_only = TestCommandCli::default();
            cli_runner_only.apply_wrappers(None, Some(&runner), workspace_root, target_dir);
            cli_runner_only.extend(["binary", "arg"]);
            assert_eq!(
                cli_runner_only.to_owned_cli(),
                vec!["runner", "binary", "arg"],
            );
        }

        // Test wrapper with ignore target runner
        {
            let runner = PlatformRunner::debug_new(
                "runner".into(),
                Vec::new(),
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let wrapper_ignore = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "wrapper".into(),
                    args: Vec::new(),
                    relative_to: ScriptCommandRelativeTo::None,
                },
                target_runner: WrapperScriptTargetRunner::Ignore,
            };
            let mut cli_wrapper_ignore = TestCommandCli::default();
            cli_wrapper_ignore.apply_wrappers(
                Some(&wrapper_ignore),
                Some(&runner),
                workspace_root,
                target_dir,
            );
            cli_wrapper_ignore.extend(["binary", "arg"]);
            assert_eq!(
                cli_wrapper_ignore.to_owned_cli(),
                vec!["wrapper", "binary", "arg"],
            );
        }

        // Test wrapper with around wrapper (runner first)
        {
            let runner = PlatformRunner::debug_new(
                "runner".into(),
                Vec::new(),
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let wrapper_around = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "wrapper".into(),
                    args: Vec::new(),
                    relative_to: ScriptCommandRelativeTo::None,
                },
                target_runner: WrapperScriptTargetRunner::AroundWrapper,
            };
            let mut cli_wrapper_around = TestCommandCli::default();
            cli_wrapper_around.apply_wrappers(
                Some(&wrapper_around),
                Some(&runner),
                workspace_root,
                target_dir,
            );
            cli_wrapper_around.extend(["binary", "arg"]);
            assert_eq!(
                cli_wrapper_around.to_owned_cli(),
                vec!["runner", "wrapper", "binary", "arg"],
            );
        }

        // Test wrapper with within wrapper (wrapper first)
        {
            let runner = PlatformRunner::debug_new(
                "runner".into(),
                Vec::new(),
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let wrapper_within = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "wrapper".into(),
                    args: Vec::new(),
                    relative_to: ScriptCommandRelativeTo::None,
                },
                target_runner: WrapperScriptTargetRunner::WithinWrapper,
            };
            let mut cli_wrapper_within = TestCommandCli::default();
            cli_wrapper_within.apply_wrappers(
                Some(&wrapper_within),
                Some(&runner),
                workspace_root,
                target_dir,
            );
            cli_wrapper_within.extend(["binary", "arg"]);
            assert_eq!(
                cli_wrapper_within.to_owned_cli(),
                vec!["wrapper", "runner", "binary", "arg"],
            );
        }

        // Test wrapper with overrides wrapper (runner only)
        {
            let runner = PlatformRunner::debug_new(
                "runner".into(),
                Vec::new(),
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let wrapper_overrides = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "wrapper".into(),
                    args: Vec::new(),
                    relative_to: ScriptCommandRelativeTo::None,
                },
                target_runner: WrapperScriptTargetRunner::OverridesWrapper,
            };
            let mut cli_wrapper_overrides = TestCommandCli::default();
            cli_wrapper_overrides.apply_wrappers(
                Some(&wrapper_overrides),
                Some(&runner),
                workspace_root,
                target_dir,
            );
            cli_wrapper_overrides.extend(["binary", "arg"]);
            assert_eq!(
                cli_wrapper_overrides.to_owned_cli(),
                vec!["runner", "binary", "arg"],
            );
        }

        // Test wrapper with args
        {
            let wrapper_with_args = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "wrapper".into(),
                    args: vec!["--flag".to_string(), "value".to_string()],
                    relative_to: ScriptCommandRelativeTo::None,
                },
                target_runner: WrapperScriptTargetRunner::Ignore,
            };
            let mut cli_wrapper_args = TestCommandCli::default();
            cli_wrapper_args.apply_wrappers(
                Some(&wrapper_with_args),
                None,
                workspace_root,
                target_dir,
            );
            cli_wrapper_args.extend(["binary", "arg"]);
            assert_eq!(
                cli_wrapper_args.to_owned_cli(),
                vec!["wrapper", "--flag", "value", "binary", "arg"],
            );
        }

        // Test platform runner with args
        {
            let runner_with_args = PlatformRunner::debug_new(
                "runner".into(),
                vec!["--runner-flag".into(), "value".into()],
                PlatformRunnerSource::Env("fake".to_owned()),
            );
            let mut cli_runner_args = TestCommandCli::default();
            cli_runner_args.apply_wrappers(
                None,
                Some(&runner_with_args),
                workspace_root,
                target_dir,
            );
            cli_runner_args.extend(["binary", "arg"]);
            assert_eq!(
                cli_runner_args.to_owned_cli(),
                vec!["runner", "--runner-flag", "value", "binary", "arg"],
            );
        }

        // Test wrapper with ScriptCommandRelativeTo::WorkspaceRoot
        {
            let wrapper_relative_to_workspace_root = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "abc/def/my-wrapper".into(),
                    args: vec!["--verbose".to_string()],
                    relative_to: ScriptCommandRelativeTo::WorkspaceRoot,
                },
                target_runner: WrapperScriptTargetRunner::Ignore,
            };
            let mut cli_wrapper_relative = TestCommandCli::default();
            cli_wrapper_relative.apply_wrappers(
                Some(&wrapper_relative_to_workspace_root),
                None,
                workspace_root,
                target_dir,
            );
            cli_wrapper_relative.extend(["binary", "arg"]);

            cfg_if::cfg_if! {
                if #[cfg(windows)] {
                    let wrapper_path = "D:\\workspace\\root\\abc\\def\\my-wrapper";
                } else {
                    let wrapper_path = "/workspace/root/abc/def/my-wrapper";
                }
            }
            assert_eq!(
                cli_wrapper_relative.to_owned_cli(),
                vec![wrapper_path, "--verbose", "binary", "arg"],
            );
        }

        // Test wrapper with ScriptCommandRelativeTo::Target
        {
            let wrapper_relative_to_target = WrapperScriptConfig {
                command: ScriptCommand {
                    program: "abc/def/my-wrapper".into(),
                    args: vec!["--verbose".to_string()],
                    relative_to: ScriptCommandRelativeTo::Target,
                },
                target_runner: WrapperScriptTargetRunner::Ignore,
            };
            let mut cli_wrapper_relative = TestCommandCli::default();
            cli_wrapper_relative.apply_wrappers(
                Some(&wrapper_relative_to_target),
                None,
                workspace_root,
                target_dir,
            );
            cli_wrapper_relative.extend(["binary", "arg"]);
            cfg_if::cfg_if! {
                if #[cfg(windows)] {
                    let wrapper_path = "C:\\foo\\bar\\abc\\def\\my-wrapper";
                } else {
                    let wrapper_path = "/foo/bar/abc/def/my-wrapper";
                }
            }
            assert_eq!(
                cli_wrapper_relative.to_owned_cli(),
                vec![wrapper_path, "--verbose", "binary", "arg"],
            );
        }
    }

    static PACKAGE_GRAPH_FIXTURE: LazyLock<PackageGraph> = LazyLock::new(|| {
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

    #[test]
    fn test_parse_list_lines() {
        let binary_id = RustBinaryId::new("test-package::test-binary");

        // Valid: tests only.
        let input = indoc! {"
            simple_test: test
            module::nested_test: test
            deeply::nested::module::test_name: test
        "};
        let results: Vec<_> = parse_list_lines(&binary_id, input)
            .collect::<Result<_, _>>()
            .expect("parsed valid test output");
        insta::assert_debug_snapshot!("valid_tests", results);

        // Valid: benchmarks only.
        let input = indoc! {"
            simple_bench: benchmark
            benches::module::my_benchmark: benchmark
        "};
        let results: Vec<_> = parse_list_lines(&binary_id, input)
            .collect::<Result<_, _>>()
            .expect("parsed valid benchmark output");
        insta::assert_debug_snapshot!("valid_benchmarks", results);

        // Valid: mixed tests and benchmarks.
        let input = indoc! {"
            test_one: test
            bench_one: benchmark
            test_two: test
            bench_two: benchmark
        "};
        let results: Vec<_> = parse_list_lines(&binary_id, input)
            .collect::<Result<_, _>>()
            .expect("parsed mixed output");
        insta::assert_debug_snapshot!("mixed_tests_and_benchmarks", results);

        // Valid: special characters.
        let input = indoc! {r#"
            test_with_underscore_123: test
            test::with::colons: test
            test_with_numbers_42: test
        "#};
        let results: Vec<_> = parse_list_lines(&binary_id, input)
            .collect::<Result<_, _>>()
            .expect("parsed tests with special characters");
        insta::assert_debug_snapshot!("special_characters", results);

        // Valid: empty input.
        let input = "";
        let results: Vec<_> = parse_list_lines(&binary_id, input)
            .collect::<Result<_, _>>()
            .expect("parsed empty output");
        insta::assert_debug_snapshot!("empty_input", results);

        // Invalid: wrong suffix.
        let input = "invalid_test: wrong_suffix";
        let result = parse_list_lines(&binary_id, input).collect::<Result<Vec<_>, _>>();
        assert!(result.is_err());
        insta::assert_snapshot!("invalid_suffix_error", result.unwrap_err());

        // Invalid: missing suffix.
        let input = "test_without_suffix";
        let result = parse_list_lines(&binary_id, input).collect::<Result<Vec<_>, _>>();
        assert!(result.is_err());
        insta::assert_snapshot!("missing_suffix_error", result.unwrap_err());

        // Invalid: partial valid (stops at first error).
        let input = indoc! {"
            valid_test: test
            invalid_line
            another_valid: benchmark
        "};
        let result = parse_list_lines(&binary_id, input).collect::<Result<Vec<_>, _>>();
        assert!(result.is_err());
        insta::assert_snapshot!("partial_valid_error", result.unwrap_err());

        // Invalid: control character.
        let input = indoc! {"
            valid_test: test
            \rinvalid_line
            another_valid: benchmark
        "};
        let result = parse_list_lines(&binary_id, input).collect::<Result<Vec<_>, _>>();
        assert!(result.is_err());
        insta::assert_snapshot!("control_character_error", result.unwrap_err());
    }

    // Proptest to verify that the `Borrow<dyn TestInstanceIdKey>` implementation for
    // `OwnedTestInstanceId` is consistent with Eq, Ord, and Hash.
    #[proptest]
    fn test_instance_id_key_borrow_consistency(
        owned1: OwnedTestInstanceId,
        owned2: OwnedTestInstanceId,
    ) {
        // Create borrowed trait object references.
        let borrowed1: &dyn TestInstanceIdKey = &owned1;
        let borrowed2: &dyn TestInstanceIdKey = &owned2;

        // Verify Eq consistency: owned equality must match borrowed equality.
        assert_eq!(
            owned1 == owned2,
            borrowed1 == borrowed2,
            "Eq must be consistent between OwnedTestInstanceId and dyn TestInstanceIdKey"
        );

        // Verify PartialOrd consistency.
        assert_eq!(
            owned1.partial_cmp(&owned2),
            borrowed1.partial_cmp(borrowed2),
            "PartialOrd must be consistent between OwnedTestInstanceId and dyn TestInstanceIdKey"
        );

        // Verify Ord consistency.
        assert_eq!(
            owned1.cmp(&owned2),
            borrowed1.cmp(borrowed2),
            "Ord must be consistent between OwnedTestInstanceId and dyn TestInstanceIdKey"
        );

        // Verify Hash consistency.
        fn hash_value(x: &impl Hash) -> u64 {
            let mut hasher = DefaultHasher::new();
            x.hash(&mut hasher);
            hasher.finish()
        }

        assert_eq!(
            hash_value(&owned1),
            hash_value(&borrowed1),
            "Hash must be consistent for owned1 and its borrowed form"
        );
        assert_eq!(
            hash_value(&owned2),
            hash_value(&borrowed2),
            "Hash must be consistent for owned2 and its borrowed form"
        );
    }
}
