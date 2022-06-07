// Copyright (c) The nextest Contributors
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

    /// Creating an archive produced an error.
    pub const ARCHIVE_CREATION_FAILED: i32 = 103;

    /// Parsing a test list produced an error.
    pub const PARSE_TEST_LIST_FAILED: i32 = 104;

    /// A user issue happened while setting up a nextest invocation.
    pub const SETUP_ERROR: i32 = 96;

    /// An experimental feature was used without the environment variable to enable it.
    pub const EXPERIMENTAL_FEATURE_NOT_ENABLED: i32 = 95;

    /// A filtering expression failed to parse.
    pub const INVALID_FILTER_EXPRESSION: i32 = 94;
}
