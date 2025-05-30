// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::CommandError;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    path::PathBuf,
    process::Command,
};
use target_spec::summaries::PlatformSummary;

/// Command builder for `cargo nextest list`.
#[derive(Clone, Debug, Default)]
pub struct ListCommand {
    cargo_path: Option<Box<Utf8Path>>,
    manifest_path: Option<Box<Utf8Path>>,
    current_dir: Option<Box<Utf8Path>>,
    args: Vec<Box<str>>,
}

impl ListCommand {
    /// Creates a new `ListCommand`.
    ///
    /// This command runs `cargo nextest list`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Path to `cargo` executable. If not set, this will use the the `$CARGO` environment variable, and
    /// if that is not set, will simply be `cargo`.
    pub fn cargo_path(&mut self, path: impl Into<Utf8PathBuf>) -> &mut Self {
        self.cargo_path = Some(path.into().into());
        self
    }

    /// Path to `Cargo.toml`.
    pub fn manifest_path(&mut self, path: impl Into<Utf8PathBuf>) -> &mut Self {
        self.manifest_path = Some(path.into().into());
        self
    }

    /// Current directory of the `cargo nextest list` process.
    pub fn current_dir(&mut self, path: impl Into<Utf8PathBuf>) -> &mut Self {
        self.current_dir = Some(path.into().into());
        self
    }

    /// Adds an argument to the end of `cargo nextest list`.
    pub fn add_arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into().into());
        self
    }

    /// Adds several arguments to the end of `cargo nextest list`.
    pub fn add_args(&mut self, args: impl IntoIterator<Item = impl Into<String>>) -> &mut Self {
        for arg in args {
            self.add_arg(arg.into());
        }
        self
    }

    /// Builds a command for `cargo nextest list`. This is the first part of the work of [`self.exec`].
    pub fn cargo_command(&self) -> Command {
        let cargo_path: PathBuf = self.cargo_path.as_ref().map_or_else(
            || std::env::var_os("CARGO").map_or("cargo".into(), PathBuf::from),
            |path| PathBuf::from(path.as_std_path()),
        );

        let mut command = Command::new(cargo_path);
        if let Some(path) = &self.manifest_path.as_deref() {
            command.args(["--manifest-path", path.as_str()]);
        }
        if let Some(current_dir) = &self.current_dir.as_deref() {
            command.current_dir(current_dir);
        }

        command.args(["nextest", "list", "--message-format=json"]);

        command.args(self.args.iter().map(|s| s.as_ref()));
        command
    }

    /// Executes `cargo nextest list` and parses the output into a [`TestListSummary`].
    pub fn exec(&self) -> Result<TestListSummary, CommandError> {
        let mut command = self.cargo_command();
        let output = command.output().map_err(CommandError::Exec)?;

        if !output.status.success() {
            // The process exited with a non-zero code.
            let exit_code = output.status.code();
            let stderr = output.stderr;
            return Err(CommandError::CommandFailed { exit_code, stderr });
        }

        // Try parsing stdout.
        serde_json::from_slice(&output.stdout).map_err(CommandError::Json)
    }

    /// Executes `cargo nextest list --list-type binaries-only` and parses the output into a
    /// [`BinaryListSummary`].
    pub fn exec_binaries_only(&self) -> Result<BinaryListSummary, CommandError> {
        let mut command = self.cargo_command();
        command.arg("--list-type=binaries-only");
        let output = command.output().map_err(CommandError::Exec)?;

        if !output.status.success() {
            // The process exited with a non-zero code.
            let exit_code = output.status.code();
            let stderr = output.stderr;
            return Err(CommandError::CommandFailed { exit_code, stderr });
        }

        // Try parsing stdout.
        serde_json::from_slice(&output.stdout).map_err(CommandError::Json)
    }
}

/// Root element for a serializable list of tests generated by nextest.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct TestListSummary {
    /// Rust metadata used for builds and test runs.
    pub rust_build_meta: RustBuildMetaSummary,

    /// Number of tests (including skipped and ignored) across all binaries.
    pub test_count: usize,

    /// A map of Rust test suites to the test binaries within them, keyed by a unique identifier
    /// for each test suite.
    pub rust_suites: BTreeMap<RustBinaryId, RustTestSuiteSummary>,
}

impl TestListSummary {
    /// Creates a new `TestListSummary` with the given Rust metadata.
    pub fn new(rust_build_meta: RustBuildMetaSummary) -> Self {
        Self {
            rust_build_meta,
            test_count: 0,
            rust_suites: BTreeMap::new(),
        }
    }
    /// Parse JSON output from `cargo nextest list --message-format json`.
    pub fn parse_json(json: impl AsRef<str>) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json.as_ref())
    }
}

/// The platform a binary was built on (useful for cross-compilation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildPlatform {
    /// The target platform.
    Target,

    /// The host platform: the platform the build was performed on.
    Host,
}

impl fmt::Display for BuildPlatform {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Target => write!(f, "target"),
            Self::Host => write!(f, "host"),
        }
    }
}

/// A serializable Rust test binary.
///
/// Part of a [`RustTestSuiteSummary`] and [`BinaryListSummary`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustTestBinarySummary {
    /// A unique binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test binary within the package.
    pub binary_name: String,

    /// The unique package ID assigned by Cargo to this test.
    ///
    /// This package ID can be used for lookups in `cargo metadata`.
    pub package_id: String,

    /// The kind of Rust test binary this is.
    pub kind: RustTestBinaryKind,

    /// The path to the test binary executable.
    pub binary_path: Utf8PathBuf,

    /// Platform for which this binary was built.
    /// (Proc-macro tests are built for the host.)
    pub build_platform: BuildPlatform,
}

/// Information about the kind of a Rust test binary.
///
/// Kinds are used to generate [`RustBinaryId`] instances, and to figure out whether some
/// environment variables should be set.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RustTestBinaryKind(pub Cow<'static, str>);

impl RustTestBinaryKind {
    /// Creates a new `RustTestBinaryKind` from a string.
    #[inline]
    pub fn new(kind: impl Into<Cow<'static, str>>) -> Self {
        Self(kind.into())
    }

    /// Creates a new `RustTestBinaryKind` from a static string.
    #[inline]
    pub const fn new_const(kind: &'static str) -> Self {
        Self(Cow::Borrowed(kind))
    }

    /// Returns the kind as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The "lib" kind, used for unit tests within the library.
    pub const LIB: Self = Self::new_const("lib");

    /// The "test" kind, used for integration tests.
    pub const TEST: Self = Self::new_const("test");

    /// The "bench" kind, used for benchmarks.
    pub const BENCH: Self = Self::new_const("bench");

    /// The "bin" kind, used for unit tests within binaries.
    pub const BIN: Self = Self::new_const("bin");

    /// The "example" kind, used for unit tests within examples.
    pub const EXAMPLE: Self = Self::new_const("example");

    /// The "proc-macro" kind, used for tests within procedural macros.
    pub const PROC_MACRO: Self = Self::new_const("proc-macro");
}

impl fmt::Display for RustTestBinaryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A serializable suite of test binaries.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BinaryListSummary {
    /// Rust metadata used for builds and test runs.
    pub rust_build_meta: RustBuildMetaSummary,

    /// The list of Rust test binaries (indexed by binary-id).
    pub rust_binaries: BTreeMap<RustBinaryId, RustTestBinarySummary>,
}

// IMPLEMENTATION NOTE: SmolStr is *not* part of the public API.

/// A unique identifier for a test suite (a Rust binary).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RustBinaryId(SmolStr);

impl fmt::Display for RustBinaryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl RustBinaryId {
    /// Creates a new `RustBinaryId` from a string.
    #[inline]
    pub fn new(id: &str) -> Self {
        Self(id.into())
    }

    /// Creates a new `RustBinaryId` from its constituent parts:
    ///
    /// * `package_name`: The name of the package as defined in `Cargo.toml`.
    /// * `kind`: The kind of the target (see [`RustTestBinaryKind`]).
    /// * `target_name`: The name of the target.
    ///
    /// The algorithm is as follows:
    ///
    /// 1. If the kind is `lib` or `proc-macro` (i.e. for unit tests), the binary ID is the same as
    ///    the package name. There can only be one library per package, so this will always be
    ///    unique.
    /// 2. If the target is an integration test, the binary ID is `package_name::target_name`.
    /// 3. Otherwise, the binary ID is `package_name::{kind}/{target_name}`.
    ///
    /// This format is part of nextest's stable API.
    ///
    /// # Examples
    ///
    /// ```
    /// use nextest_metadata::{RustBinaryId, RustTestBinaryKind};
    ///
    /// // The lib and proc-macro kinds.
    /// assert_eq!(
    ///     RustBinaryId::from_parts("foo-lib", &RustTestBinaryKind::LIB, "foo_lib"),
    ///     RustBinaryId::new("foo-lib"),
    /// );
    /// assert_eq!(
    ///     RustBinaryId::from_parts("foo-derive", &RustTestBinaryKind::PROC_MACRO, "derive"),
    ///     RustBinaryId::new("foo-derive"),
    /// );
    ///
    /// // Integration tests.
    /// assert_eq!(
    ///     RustBinaryId::from_parts("foo-lib", &RustTestBinaryKind::TEST, "foo_test"),
    ///     RustBinaryId::new("foo-lib::foo_test"),
    /// );
    ///
    /// // Other kinds.
    /// assert_eq!(
    ///     RustBinaryId::from_parts("foo-lib", &RustTestBinaryKind::BIN, "foo_bin"),
    ///     RustBinaryId::new("foo-lib::bin/foo_bin"),
    /// );
    /// ```
    pub fn from_parts(package_name: &str, kind: &RustTestBinaryKind, target_name: &str) -> Self {
        let mut id = package_name.to_owned();
        // To ensure unique binary IDs, we use the following scheme:
        if kind == &RustTestBinaryKind::LIB || kind == &RustTestBinaryKind::PROC_MACRO {
            // 1. The binary ID is the same as the package name.
        } else if kind == &RustTestBinaryKind::TEST {
            // 2. For integration tests, use package_name::target_name. Cargo enforces unique names
            //    for the same kind of targets in a package, so these will always be unique.
            id.push_str("::");
            id.push_str(target_name);
        } else {
            // 3. For all other target kinds, use a combination of the target kind and
            //    the target name. For the same reason as above, these will always be
            //    unique.
            write!(id, "::{kind}/{target_name}").unwrap();
        }

        Self(id.into())
    }

    /// Returns the identifier as a string.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the length of the identifier in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the identifier is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the components of this identifier.
    #[inline]
    pub fn components(&self) -> RustBinaryIdComponents<'_> {
        RustBinaryIdComponents::new(self)
    }
}

impl<S> From<S> for RustBinaryId
where
    S: AsRef<str>,
{
    #[inline]
    fn from(s: S) -> Self {
        Self(s.as_ref().into())
    }
}

impl Ord for RustBinaryId {
    fn cmp(&self, other: &RustBinaryId) -> Ordering {
        // Use the components as the canonical sort order.
        //
        // Note: this means that we can't impl Borrow<str> for RustBinaryId,
        // since the Ord impl is inconsistent with that of &str.
        self.components().cmp(&other.components())
    }
}

impl PartialOrd for RustBinaryId {
    fn partial_cmp(&self, other: &RustBinaryId) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The components of a [`RustBinaryId`].
///
/// This defines the canonical sort order for a `RustBinaryId`.
///
/// Returned by [`RustBinaryId::components`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RustBinaryIdComponents<'a> {
    /// The name of the package.
    pub package_name: &'a str,

    /// The kind and binary name, if specified.
    pub binary_name_and_kind: RustBinaryIdNameAndKind<'a>,
}

impl<'a> RustBinaryIdComponents<'a> {
    fn new(id: &'a RustBinaryId) -> Self {
        let mut parts = id.as_str().splitn(2, "::");

        let package_name = parts
            .next()
            .expect("splitn(2) returns at least 1 component");
        let binary_name_and_kind = if let Some(suffix) = parts.next() {
            let mut parts = suffix.splitn(2, '/');

            let part1 = parts
                .next()
                .expect("splitn(2) returns at least 1 component");
            if let Some(binary_name) = parts.next() {
                RustBinaryIdNameAndKind::NameAndKind {
                    kind: part1,
                    binary_name,
                }
            } else {
                RustBinaryIdNameAndKind::NameOnly { binary_name: part1 }
            }
        } else {
            RustBinaryIdNameAndKind::None
        };

        Self {
            package_name,
            binary_name_and_kind,
        }
    }
}

/// The name and kind of a Rust binary, present within a [`RustBinaryId`].
///
/// Part of [`RustBinaryIdComponents`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RustBinaryIdNameAndKind<'a> {
    /// The binary has no name or kind.
    None,

    /// The binary has a name but no kind.
    NameOnly {
        /// The name of the binary.
        binary_name: &'a str,
    },

    /// The binary has a name and kind.
    NameAndKind {
        /// The kind of the binary.
        kind: &'a str,

        /// The name of the binary.
        binary_name: &'a str,
    },
}

/// Rust metadata used for builds and test runs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct RustBuildMetaSummary {
    /// The target directory for Rust artifacts.
    pub target_directory: Utf8PathBuf,

    /// Base output directories, relative to the target directory.
    pub base_output_directories: BTreeSet<Utf8PathBuf>,

    /// Information about non-test binaries, keyed by package ID.
    pub non_test_binaries: BTreeMap<String, BTreeSet<RustNonTestBinarySummary>>,

    /// Build script output directory, relative to the target directory and keyed by package ID.
    /// Only present for workspace packages that have build scripts.
    ///
    /// Added in cargo-nextest 0.9.65.
    #[serde(default)]
    pub build_script_out_dirs: BTreeMap<String, Utf8PathBuf>,

    /// Linked paths, relative to the target directory.
    pub linked_paths: BTreeSet<Utf8PathBuf>,

    /// The build platforms used while compiling the Rust artifacts.
    ///
    /// Added in cargo-nextest 0.9.72.
    #[serde(default)]
    pub platforms: Option<BuildPlatformsSummary>,

    /// The target platforms used while compiling the Rust artifacts.
    ///
    /// Deprecated in favor of [`Self::platforms`]; use that if available.
    #[serde(default)]
    pub target_platforms: Vec<PlatformSummary>,

    /// A deprecated form of the target platform used for cross-compilation, if any.
    ///
    /// Deprecated in favor of (in order) [`Self::platforms`] and [`Self::target_platforms`]; use
    /// those if available.
    #[serde(default)]
    pub target_platform: Option<String>,
}

/// A non-test Rust binary. Used to set the correct environment
/// variables in reused builds.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustNonTestBinarySummary {
    /// The name of the binary.
    pub name: String,

    /// The kind of binary this is.
    pub kind: RustNonTestBinaryKind,

    /// The path to the binary, relative to the target directory.
    pub path: Utf8PathBuf,
}

/// Serialized representation of the host and the target platform.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BuildPlatformsSummary {
    /// The host platform used while compiling the Rust artifacts.
    pub host: HostPlatformSummary,

    /// The target platforms used while compiling the Rust artifacts.
    ///
    /// With current versions of nextest, this will contain at most one element.
    pub targets: Vec<TargetPlatformSummary>,
}

/// Serialized representation of the host platform.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct HostPlatformSummary {
    /// The host platform, if specified.
    pub platform: PlatformSummary,

    /// The libdir for the host platform.
    pub libdir: PlatformLibdirSummary,
}

/// Serialized representation of the target platform.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TargetPlatformSummary {
    /// The target platform, if specified.
    pub platform: PlatformSummary,

    /// The libdir for the target platform.
    ///
    /// Err if we failed to discover it.
    pub libdir: PlatformLibdirSummary,
}

/// Serialized representation of a platform's library directory.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum PlatformLibdirSummary {
    /// The libdir is available.
    Available {
        /// The libdir.
        path: Utf8PathBuf,
    },

    /// The libdir is unavailable, for the reason provided in the inner value.
    Unavailable {
        /// The reason why the libdir is unavailable.
        reason: PlatformLibdirUnavailable,
    },
}

/// The reason why a platform libdir is unavailable.
///
/// Part of [`PlatformLibdirSummary`].
///
/// This is an open-ended enum that may have additional deserializable variants in the future.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PlatformLibdirUnavailable(pub Cow<'static, str>);

impl PlatformLibdirUnavailable {
    /// The libdir is not available because the rustc invocation to obtain it failed.
    pub const RUSTC_FAILED: Self = Self::new_const("rustc-failed");

    /// The libdir is not available because it was attempted to be read from rustc, but there was an
    /// issue with its output.
    pub const RUSTC_OUTPUT_ERROR: Self = Self::new_const("rustc-output-error");

    /// The libdir is unavailable because it was deserialized from a summary serialized by an older
    /// version of nextest.
    pub const OLD_SUMMARY: Self = Self::new_const("old-summary");

    /// The libdir is unavailable because a build was reused from an archive, and the libdir was not
    /// present in the archive
    pub const NOT_IN_ARCHIVE: Self = Self::new_const("not-in-archive");

    /// Converts a static string into Self.
    pub const fn new_const(reason: &'static str) -> Self {
        Self(Cow::Borrowed(reason))
    }

    /// Converts a string into Self.
    pub fn new(reason: impl Into<Cow<'static, str>>) -> Self {
        Self(reason.into())
    }

    /// Returns self as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Information about the kind of a Rust non-test binary.
///
/// This is part of [`RustNonTestBinarySummary`], and is used to determine runtime environment
/// variables.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RustNonTestBinaryKind(pub Cow<'static, str>);

impl RustNonTestBinaryKind {
    /// Creates a new `RustNonTestBinaryKind` from a string.
    #[inline]
    pub fn new(kind: impl Into<Cow<'static, str>>) -> Self {
        Self(kind.into())
    }

    /// Creates a new `RustNonTestBinaryKind` from a static string.
    #[inline]
    pub const fn new_const(kind: &'static str) -> Self {
        Self(Cow::Borrowed(kind))
    }

    /// Returns the kind as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The "dylib" kind, used for dynamic libraries (`.so` on Linux). Also used for
    /// .pdb and other similar files on Windows.
    pub const DYLIB: Self = Self::new_const("dylib");

    /// The "bin-exe" kind, used for binary executables.
    pub const BIN_EXE: Self = Self::new_const("bin-exe");
}

impl fmt::Display for RustNonTestBinaryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A serializable suite of tests within a Rust test binary.
///
/// Part of a [`TestListSummary`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustTestSuiteSummary {
    /// The name of this package in the workspace.
    pub package_name: String,

    /// The binary within the package.
    #[serde(flatten)]
    pub binary: RustTestBinarySummary,

    /// The working directory that tests within this package are run in.
    pub cwd: Utf8PathBuf,

    /// Status of this test suite.
    ///
    /// Introduced in cargo-nextest 0.9.25. Older versions always imply
    /// [`LISTED`](RustTestSuiteStatusSummary::LISTED).
    #[serde(default = "listed_status")]
    pub status: RustTestSuiteStatusSummary,

    /// Test cases within this test suite.
    #[serde(rename = "testcases")]
    pub test_cases: BTreeMap<String, RustTestCaseSummary>,
}

fn listed_status() -> RustTestSuiteStatusSummary {
    RustTestSuiteStatusSummary::LISTED
}

/// Information about whether a test suite was listed or skipped.
///
/// This is part of [`RustTestSuiteSummary`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RustTestSuiteStatusSummary(pub Cow<'static, str>);

impl RustTestSuiteStatusSummary {
    /// Creates a new `RustNonTestBinaryKind` from a string.
    #[inline]
    pub fn new(kind: impl Into<Cow<'static, str>>) -> Self {
        Self(kind.into())
    }

    /// Creates a new `RustNonTestBinaryKind` from a static string.
    #[inline]
    pub const fn new_const(kind: &'static str) -> Self {
        Self(Cow::Borrowed(kind))
    }

    /// Returns the kind as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The "listed" kind, which means that the test binary was executed with `--list` to gather the
    /// list of tests in it.
    pub const LISTED: Self = Self::new_const("listed");

    /// The "skipped" kind, which indicates that the test binary was not executed because it didn't
    /// match any filtersets.
    ///
    /// In this case, the contents of [`RustTestSuiteSummary::test_cases`] is empty.
    pub const SKIPPED: Self = Self::new_const("skipped");

    /// The binary doesn't match the profile's `default-filter`.
    ///
    /// This is the lowest-priority reason for skipping a binary.
    pub const SKIPPED_DEFAULT_FILTER: Self = Self::new_const("skipped-default-filter");
}

/// Serializable information about an individual test case within a Rust test suite.
///
/// Part of a [`RustTestSuiteSummary`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustTestCaseSummary {
    /// Returns true if this test is marked ignored.
    ///
    /// Ignored tests, if run, are executed with the `--ignored` argument.
    pub ignored: bool,

    /// Whether the test matches the provided test filter.
    ///
    /// Only tests that match the filter are run.
    pub filter_match: FilterMatch,
}

/// An enum describing whether a test matches a filter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum FilterMatch {
    /// This test matches this filter.
    Matches,

    /// This test does not match this filter.
    Mismatch {
        /// Describes the reason this filter isn't matched.
        reason: MismatchReason,
    },
}

impl FilterMatch {
    /// Returns true if the filter doesn't match.
    pub fn is_match(&self) -> bool {
        matches!(self, FilterMatch::Matches)
    }
}

/// The reason for why a test doesn't match a filter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MismatchReason {
    /// This test does not match the run-ignored option in the filter.
    Ignored,

    /// This test does not match the provided string filters.
    String,

    /// This test does not match the provided expression filters.
    Expression,

    /// This test is in a different partition.
    Partition,

    /// This test is filtered out by the default-filter.
    ///
    /// This is the lowest-priority reason for skipping a test.
    DefaultFilter,
}

impl fmt::Display for MismatchReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MismatchReason::Ignored => write!(f, "does not match the run-ignored option"),
            MismatchReason::String => write!(f, "does not match the provided string filters"),
            MismatchReason::Expression => {
                write!(f, "does not match the provided expression filters")
            }
            MismatchReason::Partition => write!(f, "is in a different partition"),
            MismatchReason::DefaultFilter => {
                write!(f, "is filtered out by the profile's default-filter")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(r#"{
        "target-directory": "/foo",
        "base-output-directories": [],
        "non-test-binaries": {},
        "linked-paths": []
    }"#, RustBuildMetaSummary {
        target_directory: "/foo".into(),
        base_output_directories: BTreeSet::new(),
        non_test_binaries: BTreeMap::new(),
        build_script_out_dirs: BTreeMap::new(),
        linked_paths: BTreeSet::new(),
        target_platform: None,
        target_platforms: vec![],
        platforms: None,
    }; "no target platform")]
    #[test_case(r#"{
        "target-directory": "/foo",
        "base-output-directories": [],
        "non-test-binaries": {},
        "linked-paths": [],
        "target-platform": "x86_64-unknown-linux-gnu"
    }"#, RustBuildMetaSummary {
        target_directory: "/foo".into(),
        base_output_directories: BTreeSet::new(),
        non_test_binaries: BTreeMap::new(),
        build_script_out_dirs: BTreeMap::new(),
        linked_paths: BTreeSet::new(),
        target_platform: Some("x86_64-unknown-linux-gnu".to_owned()),
        target_platforms: vec![],
        platforms: None,
    }; "single target platform specified")]
    fn test_deserialize_old_rust_build_meta(input: &str, expected: RustBuildMetaSummary) {
        let build_meta: RustBuildMetaSummary =
            serde_json::from_str(input).expect("input deserialized correctly");
        assert_eq!(
            build_meta, expected,
            "deserialized input matched expected output"
        );
    }

    #[test]
    fn test_binary_id_ord() {
        let empty = RustBinaryId::new("");
        let foo = RustBinaryId::new("foo");
        let bar = RustBinaryId::new("bar");
        let foo_name1 = RustBinaryId::new("foo::name1");
        let foo_name2 = RustBinaryId::new("foo::name2");
        let bar_name = RustBinaryId::new("bar::name");
        let foo_bin_name1 = RustBinaryId::new("foo::bin/name1");
        let foo_bin_name2 = RustBinaryId::new("foo::bin/name2");
        let bar_bin_name = RustBinaryId::new("bar::bin/name");
        let foo_proc_macro_name = RustBinaryId::new("foo::proc_macro/name");
        let bar_proc_macro_name = RustBinaryId::new("bar::proc_macro/name");

        // This defines the expected sort order.
        let sorted_ids = [
            empty,
            bar,
            bar_name,
            bar_bin_name,
            bar_proc_macro_name,
            foo,
            foo_name1,
            foo_name2,
            foo_bin_name1,
            foo_bin_name2,
            foo_proc_macro_name,
        ];

        for (i, id) in sorted_ids.iter().enumerate() {
            for (j, other_id) in sorted_ids.iter().enumerate() {
                let expected = i.cmp(&j);
                assert_eq!(
                    id.cmp(other_id),
                    expected,
                    "comparing {id:?} to {other_id:?} gave {expected:?}"
                );
            }
        }
    }
}
