// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [USDT](usdt) probes for nextest.
//!
//! This module acts as documentation for USDT (Userland Statically Defined
//! Tracing) probes defined by nextest.
//!
//! USDT probes are supported:
//!
//! * On x86_64: Linux, via [bpftrace](https://bpftrace.org/)
//! * On aarch64: macOS, via [DTrace](https://dtrace.org/)
//! * On x86_64 and aarch64: illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/)
//!
//! The probes and their contents are not part of nextest's stability guarantees.
//!
//! For more information and examples, see the [nextest documentation](https://nexte.st/docs/integrations/usdt).

use nextest_metadata::RustBinaryId;
use quick_junit::ReportUuid;
use serde::Serialize;

/// Register USDT probes on supported platforms.
#[cfg(any(
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "freebsd", target_os = "illumos")
    ),
    all(
        target_arch = "aarch64",
        any(target_os = "macos", target_os = "freebsd", target_os = "illumos")
    )
))]
pub fn register_probes() -> Result<(), usdt::Error> {
    usdt::register_probes()
}

/// No-op for unsupported platforms.
#[cfg(not(any(
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "freebsd", target_os = "illumos")
    ),
    all(
        target_arch = "aarch64",
        any(target_os = "macos", target_os = "freebsd", target_os = "illumos")
    )
)))]
pub fn register_probes() -> Result<(), std::convert::Infallible> {
    Ok(())
}

#[cfg(any(
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "freebsd", target_os = "illumos")
    ),
    all(
        target_arch = "aarch64",
        any(target_os = "macos", target_os = "freebsd", target_os = "illumos")
    )
))]
#[usdt::provider(provider = "nextest")]
pub mod usdt_probes {
    use crate::usdt::*;

    pub fn test__attempt__start(
        attempt: &UsdtTestAttemptStart,
        attempt_id: &str,
        binary_id: &str,
        test_name: &str,
        pid: u32,
    ) {
    }
    pub fn test__attempt__done(
        attempt: &UsdtTestAttemptDone,
        attempt_id: &str,
        binary_id: &str,
        test_name: &str,
        result: &str,
        duration_nanos: u64,
    ) {
    }
    pub fn test__attempt__slow(
        slow: &UsdtTestAttemptSlow,
        attempt_id: &str,
        binary_id: &str,
        test_name: &str,
        elapsed_nanos: u64,
    ) {
    }
    pub fn setup__script__start(
        script: &UsdtSetupScriptStart,
        id: &str,
        script_id: &str,
        pid: u32,
    ) {
    }
    pub fn setup__script__slow(
        script: &UsdtSetupScriptSlow,
        id: &str,
        script_id: &str,
        elapsed_nanos: u64,
    ) {
    }
    pub fn setup__script__done(
        script: &UsdtSetupScriptDone,
        id: &str,
        script_id: &str,
        result: &str,
        duration_nanos: u64,
    ) {
    }
    pub fn run__start(run: &UsdtRunStart, run_id: ReportUuid) {}
    pub fn run__done(run: &UsdtRunDone, run_id: ReportUuid) {}
}

/// Fires a USDT probe on supported platforms.
#[cfg(any(
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "freebsd", target_os = "illumos")
    ),
    all(
        target_arch = "aarch64",
        any(target_os = "macos", target_os = "freebsd", target_os = "illumos")
    )
))]
#[macro_export]
macro_rules! fire_usdt {
    (UsdtTestAttemptStart { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::test__attempt__start!(|| {
            let probe = $crate::usdt::UsdtTestAttemptStart { $($tt)* };
            let attempt_id = probe.attempt_id.clone();
            let binary_id = probe.binary_id.to_string();
            let test_name = probe.test_name.clone();
            let pid = probe.pid;
            (probe, attempt_id, binary_id, test_name, pid)
        })
    }};
    (UsdtTestAttemptDone { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::test__attempt__done!(|| {
            let probe = $crate::usdt::UsdtTestAttemptDone { $($tt)* };
            let attempt_id = probe.attempt_id.clone();
            let binary_id = probe.binary_id.to_string();
            let test_name = probe.test_name.clone();
            let result = probe.result;
            let duration_nanos = probe.duration_nanos;
            (
                probe,
                attempt_id,
                binary_id,
                test_name,
                result,
                duration_nanos,
            )
        })
    }};
    (UsdtTestAttemptSlow { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::test__attempt__slow!(|| {
            let probe = $crate::usdt::UsdtTestAttemptSlow { $($tt)* };
            let attempt_id = probe.attempt_id.clone();
            let binary_id = probe.binary_id.to_string();
            let test_name = probe.test_name.clone();
            let elapsed_nanos = probe.elapsed_nanos;
            (probe, attempt_id, binary_id, test_name, elapsed_nanos)
        })
    }};
    (UsdtSetupScriptStart { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::setup__script__start!(|| {
            let probe = $crate::usdt::UsdtSetupScriptStart { $($tt)* };
            let id = probe.id.clone();
            let script_id = probe.script_id.clone();
            let pid = probe.pid;
            (probe, id, script_id, pid)
        })
    }};
    (UsdtSetupScriptSlow { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::setup__script__slow!(|| {
            let probe = $crate::usdt::UsdtSetupScriptSlow { $($tt)* };
            let id = probe.id.clone();
            let script_id = probe.script_id.clone();
            let elapsed_nanos = probe.elapsed_nanos;
            (probe, id, script_id, elapsed_nanos)
        })
    }};
    (UsdtSetupScriptDone { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::setup__script__done!(|| {
            let probe = $crate::usdt::UsdtSetupScriptDone { $($tt)* };
            let id = probe.id.clone();
            let script_id = probe.script_id.clone();
            let result = probe.result;
            let duration_nanos = probe.duration_nanos;
            (probe, id, script_id, result, duration_nanos)
        })
    }};
    (UsdtRunStart { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::run__start!(|| {
            let probe = $crate::usdt::UsdtRunStart { $($tt)* };
            let run_id = probe.run_id;
            (probe, run_id)
        })
    }};
    (UsdtRunDone { $($tt:tt)* }) => {{
        $crate::usdt::usdt_probes::run__done!(|| {
            let probe = $crate::usdt::UsdtRunDone { $($tt)* };
            let run_id = probe.run_id;
            (probe, run_id)
        })
    }};
}

/// No-op version of fire_usdt for unsupported platforms.
#[cfg(not(any(
    all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "freebsd", target_os = "illumos")
    ),
    all(
        target_arch = "aarch64",
        any(target_os = "macos", target_os = "freebsd", target_os = "illumos")
    )
)))]
#[macro_export]
macro_rules! fire_usdt {
    ($($tt:tt)*) => {
        let _ = $crate::usdt::$($tt)*;
    };
}

/// Data associated with the `test-attempt-start` probe.
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestAttemptStart {
    /// A unique identifier for this test attempt, comprised of the run ID, the
    /// binary ID, the test name, the attempt number, and the stress index.
    ///
    /// Also available as `arg1`.
    pub attempt_id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The binary ID.
    ///
    /// Also available as `arg2`.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    ///
    /// Also available as `arg3`.
    pub test_name: String,

    /// The process ID of the test.
    ///
    /// Also available as `arg4`.
    pub pid: u32,

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
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestAttemptDone {
    /// A unique identifier for this test attempt, comprised of the run ID, the
    /// binary ID, the test name, the attempt number, and the stress index.
    ///
    /// Also available as `arg1`.
    pub attempt_id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The binary ID.
    ///
    /// Also available as `arg2`.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    ///
    /// Also available as `arg3`.
    pub test_name: String,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,

    /// The test result as a string (e.g., "pass", "fail", "timeout", "exec-fail").
    ///
    /// Also available as `arg4`.
    pub result: &'static str,

    /// The exit code of the test process, if available.
    pub exit_code: Option<i32>,

    /// The duration of the test in nanoseconds.
    ///
    /// Also available as `arg5`.
    pub duration_nanos: u64,

    /// Whether file descriptors were leaked.
    pub leaked: bool,

    /// Time taken for the standard output and standard error file descriptors
    /// to close, in nanoseconds. None if they didn't close (timed out).
    pub time_to_close_fds_nanos: Option<u64>,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,

    /// The length of stdout in bytes, if captured.
    pub stdout_len: Option<u64>,

    /// The length of stderr in bytes, if captured.
    pub stderr_len: Option<u64>,
}

/// Data associated with the `test-attempt-slow` probe.
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtTestAttemptSlow {
    /// A unique identifier for this test attempt, comprised of the run ID, the
    /// binary ID, the test name, the attempt number, and the stress index.
    ///
    /// Also available as `arg1`.
    pub attempt_id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The binary ID. Also available as `arg2`.
    pub binary_id: RustBinaryId,

    /// The name of the test. Also available as `arg3`.
    pub test_name: String,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,

    /// The time elapsed since the test started, in nanoseconds.
    ///
    /// Also available as `arg4`.
    pub elapsed_nanos: u64,

    /// Whether the test is about to be terminated due to timeout.
    pub will_terminate: bool,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `setup-script-start` probe.
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptStart {
    /// A unique identifier for this script run, comprised of the run ID, the
    /// script ID, and the stress index if relevant.
    ///
    /// Also available as `arg1`.
    pub id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The script ID.
    ///
    /// Also available as `arg2`.
    pub script_id: String,

    /// The process ID of the script.
    ///
    /// Also available as `arg3`.
    pub pid: u32,

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
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptSlow {
    /// A unique identifier for this script run, comprised of the run ID, the
    /// script ID, and the stress index if relevant.
    ///
    /// Also available as `arg1`.
    pub id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The script ID.
    ///
    /// Also available as `arg2`.
    pub script_id: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The time elapsed since the script started, in nanoseconds.
    ///
    /// Also available as `arg3`.
    pub elapsed_nanos: u64,

    /// Whether the script is about to be terminated due to timeout.
    pub will_terminate: bool,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,
}

/// Data associated with the `setup-script-done` probe.
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtSetupScriptDone {
    /// A unique identifier for this script run, comprised of the run ID, the
    /// script ID, and the stress index if relevant.
    ///
    /// Also available as `arg1`.
    pub id: String,

    /// The nextest run ID, unique for each run.
    pub run_id: ReportUuid,

    /// The script ID.
    ///
    /// Also available as `arg2`.
    pub script_id: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The script result as a string (e.g., "pass", "fail", "timeout",
    /// "exec-fail").
    ///
    /// Also available as `arg3`.
    pub result: &'static str,

    /// The exit code of the script process, if available.
    pub exit_code: Option<i32>,

    /// The duration of the script execution in nanoseconds.
    ///
    /// Also available as `arg4`.
    pub duration_nanos: u64,

    /// The 0-indexed stress run index, if running stress tests.
    pub stress_current: Option<u32>,

    /// The total number of stress runs, if available.
    pub stress_total: Option<u32>,

    /// The length of stdout in bytes, if captured.
    pub stdout_len: Option<u64>,

    /// The length of stderr in bytes, if captured.
    pub stderr_len: Option<u64>,
}

/// Data associated with the `run-start` probe.
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtRunStart {
    /// The nextest run ID, unique for each run.
    ///
    /// Also available as `arg1`.
    pub run_id: ReportUuid,

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
///
/// This data is JSON-encoded as `arg0`.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtRunDone {
    /// The nextest run ID, unique for each run.
    ///
    /// Also available as `arg1`.
    pub run_id: ReportUuid,

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

    /// Total active duration of the run in nanoseconds, not including paused time.
    pub duration_nanos: u64,

    /// The number of nanoseconds the run was paused.
    pub paused_nanos: u64,
}
