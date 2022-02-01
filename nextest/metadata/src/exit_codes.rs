// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Documented exit codes for `cargo nextest` failures.
///
/// `cargo nextest` runs may fail for a variety of reasons. This structure documents the exit codes
/// that may occur in case of expected failures.
///
/// Unknown/unexpected failures will always result in exit code 1.
pub enum NextestExitCode {}

impl NextestExitCode {
    /// Running `cargo metadata` produced an error.
    pub const CARGO_METADATA_FAILED: i32 = 102;

    /// Building tests produced an error.
    pub const BUILD_FAILED: i32 = 101;

    /// One or more tests failed.
    pub const TEST_RUN_FAILED: i32 = 100;

    /// A user issue happened while setting up a nextest invocation.
    pub const SETUP_ERROR: i32 = 96;
}
