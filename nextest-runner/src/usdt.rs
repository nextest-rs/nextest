// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [USDT](usdt) probes for nextest.
//!
//! This module acts as documentation for USDT (Userland Statically Defined
//! Tracing) probes defined by nextest.
//!
//! USDT probes are supported on:
//!
//! * x86_64 Linux, via [bpftrace](https://bpftrace.org/) (aarch64 might work as well)
//! * macOS, illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/)
//!
//! The probes and their contents are not part of nextest's stable API.

use nextest_metadata::RustBinaryId;
use serde::Serialize;

/// Data associated with the `test-attempt-start` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestAttemptStart {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    pub test_name: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `test-attempt-done` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestAttemptDone {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    pub test_name: String,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,

    /// The test result as a string (e.g., "pass", "fail", "timeout", "exec-fail").
    pub result: &'static str,

    /// The exit code of the test process, if available.
    pub exit_code: Option<i32>,

    /// The duration of the test in seconds.
    pub duration_secs: f64,

    /// Whether file descriptors were leaked.
    pub leaked: bool,

    /// Time taken for the standard output and standard error file descriptors
    /// to close, in seconds. None if they didn't close (timed out).
    pub time_to_close_fds_secs: Option<f64>,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `test-slow` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestSlow {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    pub test_name: String,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,

    /// The time elapsed since the test started, in seconds.
    pub elapsed_secs: f64,

    /// Whether the test is about to be terminated due to timeout.
    pub will_terminate: bool,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `setup-script-start` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptStart {
    /// The script ID.
    pub script_id: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `setup-script-slow` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptSlow {
    /// The script ID.
    pub script_id: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The time elapsed since the script started, in seconds.
    pub elapsed_secs: f64,

    /// Whether the script is about to be terminated due to timeout.
    pub will_terminate: bool,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `setup-script-done` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptDone {
    /// The script ID.
    pub script_id: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The script result as a string (e.g., "pass", "fail", "timeout", "exec-fail").
    pub result: &'static str,

    /// The exit code of the script process, if available.
    pub exit_code: Option<i32>,

    /// The duration of the script execution in seconds.
    pub duration_secs: f64,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `run-start` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtRunStart {
    /// The profile name (e.g., "default", "ci").
    pub profile_name: String,

    /// Total number of tests in the test list.
    pub total_tests: usize,

    /// Number of tests after filtering.
    pub filter_count: usize,

    /// Number of test threads (concurrency level).
    pub test_threads: usize,
}

/// Data associated with the `run-done` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtRunDone {
    /// The profile name (e.g., "default", "ci").
    pub profile_name: String,

    /// Total number of tests that were run.
    pub total_tests: usize,

    /// Number of tests that passed.
    pub passed: usize,

    /// Number of tests that failed.
    pub failed: usize,

    /// Number of tests that were skipped.
    pub skipped: usize,

    /// Total active duration of the run in seconds, not including paused time.
    pub duration_secs: f64,

    /// The number of seconds the run was paused.
    pub paused_secs: f64,
}

#[usdt::provider(provider = "nextest")]
pub mod usdt_probes {
    use crate::usdt::*;

    pub fn test__attempt__start(attempt: &UsdtTestAttemptStart) {}
    pub fn test__attempt__done(attempt: &UsdtTestAttemptDone) {}
    pub fn test__slow(slow: &UsdtTestSlow) {}
    pub fn setup__script__start(script: &UsdtSetupScriptStart) {}
    pub fn setup__script__slow(script: &UsdtSetupScriptSlow) {}
    pub fn setup__script__done(script: &UsdtSetupScriptDone) {}
    pub fn run__start(run: &UsdtRunStart) {}
    pub fn run__done(run: &UsdtRunDone) {}
}
