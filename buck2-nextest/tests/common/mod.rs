// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helpers shared by the tests that drive the example Buck2 project.
//!
//! Every test here is gated on `buck2` being installed. Nextest has no skip
//! state, so when `buck2` is missing those tests pass without checking
//! anything -- a pass is not on its own evidence that the example works.

use std::{
    ffi::OsStr,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Returns the example project's directory.
pub fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("example")
}

/// Returns whether `buck2` can be run at all.
pub fn buck2_is_installed() -> bool {
    match Command::new("buck2").arg("--version").output() {
        Ok(output) => output.status.success(),
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => panic!("failed to run `buck2 --version`: {error}"),
    }
}

/// Prepares a `buck2` invocation in a private isolation directory.
///
/// The isolation directory keeps the test from disturbing -- or being disturbed
/// by -- a daemon a developer is using in the same project. Each test passes its
/// own, so tests do not collide with each other either.
pub fn buck2<I, S>(example: &Path, isolation_dir: &str, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("buck2");
    command
        .current_dir(example)
        .args(["--isolation-dir", isolation_dir])
        .args(args);
    command
}

#[track_caller]
pub fn assert_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "`{what}` succeeded, got {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Kills the Buck2 daemon a test started when it goes out of scope.
///
/// Buck2 daemons linger for hours after a build. Since each runs in a private
/// isolation directory, nothing else will reuse them.
pub struct DaemonGuard<'a> {
    example: &'a Path,
    isolation_dir: &'a str,
}

impl<'a> DaemonGuard<'a> {
    pub fn new(example: &'a Path, isolation_dir: &'a str) -> Self {
        Self {
            example,
            isolation_dir,
        }
    }
}

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        // A failure here should not mask whatever the test was reporting.
        if let Err(error) = buck2(self.example, self.isolation_dir, ["kill"]).output() {
            eprintln!("failed to kill the buck2 daemon: {error}");
        }
    }
}
