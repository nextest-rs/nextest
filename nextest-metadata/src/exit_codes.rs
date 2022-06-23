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

    /// Creating a test list produced an error.
    pub const TEST_LIST_CREATION_FAILED: i32 = 104;

    /// Writing data to stdout or stderr produced an error.
    pub const WRITE_OUTPUT_ERROR: i32 = 110;

    /// Downloading an update resulted in an error.
    pub const UPDATE_ERROR: i32 = 90;

    /// An update was available and `--check` was requested.
    pub const UPDATE_AVAILABLE: i32 = 80;

    /// A downgrade was requested but not performed.
    pub const UPDATE_DOWNGRADE_NOT_PERFORMED: i32 = 81;

    /// An update was available but the user canceled it.
    pub const UPDATE_CANCELED: i32 = 82;

    /// A user issue happened while setting up a nextest invocation.
    pub const SETUP_ERROR: i32 = 96;

    /// An experimental feature was used without the environment variable to enable it.
    pub const EXPERIMENTAL_FEATURE_NOT_ENABLED: i32 = 95;

    /// A filtering expression failed to parse.
    pub const INVALID_FILTER_EXPRESSION: i32 = 94;

    /// A self-update was requested but this version of cargo-nextest cannot perform self-updates.
    pub const SELF_UPDATE_UNAVAILABLE: i32 = 93;
}
