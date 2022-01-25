// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Documented exit codes for `cargo nextest` failures.
pub enum NextestExitCodes {}

impl NextestExitCodes {
    /// Running `cargo metadata` produced an error.
    pub const CARGO_METADATA_FAILED: i32 = 102;

    /// Building tests produced an error.
    pub const BUILD_FAILED: i32 = 101;

    /// One or more tests failed.
    pub const TEST_RUN_FAILED: i32 = 100;
}
