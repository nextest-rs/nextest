// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Extra details for invoking a test binary.
//!
//! Cargo builds a test binary that nextest invokes directly: the program is the
//! binary path, and the only arguments are the libtest ones nextest appends
//! (`--list --format terse` when listing, `--exact <name> --nocapture` when
//! running).
//!
//! Other build systems describe a test target as a full argument vector plus a
//! set of environment variables, and name the directory the test runs in rather
//! than deriving it from a manifest. Buck2, for instance, hands its test
//! executor an `ExternalRunnerTestInfo` whose `command` may place a launcher in
//! front of the binary. This type carries that extra material so the libtest
//! arguments can still be appended to the end.

use camino::Utf8PathBuf;
use std::collections::BTreeMap;

/// Extra details for invoking a test binary, beyond its path.
///
/// The program is always the test suite's binary path; this type carries what
/// goes around it. Cargo produces an empty value, for which nextest's behavior
/// is exactly as though this type did not exist.
///
/// See the [module-level documentation](self) for more.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestBinaryInvocation {
    /// Arguments placed between the binary path and nextest's libtest
    /// arguments.
    ///
    /// These are positioned so that the libtest arguments remain last, which is
    /// what a libtest-compatible harness expects.
    pub leading_args: Vec<String>,

    /// Environment variables applied in both the list and run phases.
    ///
    /// These take priority over Cargo's `[env]` configuration, but are
    /// overridden by wrapper script environments and by the variables nextest
    /// sets for every test process.
    pub env: BTreeMap<String, String>,

    /// The directory tests from this binary run in.
    ///
    /// `None` means the directory containing the package's manifest, which is
    /// what Cargo builds want. A build system that names the directory itself
    /// should set it here rather than encoding it in
    /// [`PackageInfo::manifest_path`], since the manifest path also describes
    /// where the target was declared.
    ///
    /// Note that nextest sets `CARGO_MANIFEST_DIR` to whichever directory this
    /// resolves to.
    ///
    /// [`PackageInfo::manifest_path`]: crate::list::PackageInfo::manifest_path
    pub cwd: Option<Utf8PathBuf>,
}

impl TestBinaryInvocation {
    /// Creates an invocation with no extra arguments or environment.
    ///
    /// This is what Cargo-built test binaries use.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Returns true if there is nothing to apply beyond the binary path.
    pub fn is_empty(&self) -> bool {
        self.leading_args.is_empty() && self.env.is_empty() && self.cwd.is_none()
    }
}
