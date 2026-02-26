// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests
//!
//! Running integration tests on Windows is a bit challenging. When using the local cargo-nextest as
//! the test runner:
//!
//! ```
//! cargo run -p cargo-nextest -- nextest run
//! ```
//!
//! This causes:
//! - Execution of `cargo-nextest`
//!     - Execution of `cargo test --no-run`
//!         - Build `cargo-nextest`
//!
//! Which means that cargo will try and replace the cargo-nextest binary that's currently running.
//! This is forbidden on Windows.
//!
//! To solve this issue, we introduce a "cargo-nextest-dup" binary which is exactly the same as
//! cargo-nextest, except it isn't used as the actual test runner. We refer to it with
//! `NEXTEST_BIN_EXE_cargo_nextest_dup`.

use camino::{Utf8Path, Utf8PathBuf};
use fixture_data::{models::RunProperties, nextest_tests::EXPECTED_TEST_SUITES};
use integration_tests::{
    env::{TestEnvInfo, set_env_vars_for_test},
    nextest_cli::{CargoNextestCli, CargoNextestOutput},
};
use nextest_metadata::{
    BuildPlatform, FilterMatch, MismatchReason, NextestExitCode, TestCaseName, TestListSummary,
};
use std::{borrow::Cow, fs::File, io::Write};
use target_spec::{Platform, summaries::TargetFeaturesSummary};

mod cargo_message_format;
mod fixtures;
mod interceptor;
mod large_alloc;
mod record_replay;
#[cfg(unix)]
mod sigttou;
mod stuck_signal;
mod temp_project;
mod user_config;

use crate::temp_project::{UdsStatus, create_uds};
use camino_tempfile::Utf8TempDir;
use fixtures::*;
use temp_project::TempProject;

#[test]
fn test_version_info() {
    // Note that this is slightly overdetermined: details like the length of the short commit hash
    // are not part of the format, and we have some flexibility in changing it.
    // The commit hash and date are optional because local dev builds may not include them.
    let version_regex = regex::Regex::new(
        r"^cargo-nextest (0\.9\.[0-9\-a-z\.]+)(?: \(([a-f0-9]{9}) (\d{4}-\d{2}-\d{2})\))?\n$",
    )
    .unwrap();

    let env_info = set_env_vars_for_test();

    // First run nextest with -V to get a one-line version string.
    let output = CargoNextestCli::for_test(&env_info).args(["-V"]).output();
    let short_stdout = output.stdout_as_str();
    let captures = version_regex
        .captures(&short_stdout)
        .unwrap_or_else(|| panic!("short version matches regex: {short_stdout}"));

    let version = captures.get(1).unwrap().as_str();
    let short_hash = captures.get(2).map(|m| m.as_str());
    let date = captures.get(3).map(|m| m.as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args(["--version"])
        .output();
    let long_stdout = output.stdout_as_str();

    // Check that all expected lines are found.
    let mut lines = long_stdout.lines();

    // Line 1 is the version line. Check that it matches the short version line.
    let version_line = lines.next().unwrap();
    assert_eq!(
        version_line,
        short_stdout.trim_end(),
        "long version line 1 matches short version"
    );

    // Line 2 is of the form "release: 0.9.0".
    let release_line = lines.next().unwrap();
    assert_eq!(release_line, format!("release: {version}"));

    // Lines 3 and 4 are the commit hash and date, if present.
    if let Some(short_hash) = short_hash {
        let commit_hash_line = lines.next().unwrap();
        assert!(
            commit_hash_line.starts_with(&format!("commit-hash: {short_hash}")),
            "commit hash line matches short hash: {commit_hash_line}"
        );
    }
    if let Some(date) = date {
        let commit_date_line = lines.next().unwrap();
        assert_eq!(commit_date_line, format!("commit-date: {date}"));
    }

    // The last line is the host. Just check that it begins with "host: ".
    let host_line = lines.next().unwrap();
    assert!(
        host_line.starts_with("host: "),
        "host line starts with 'host: ': {host_line}"
    );
}

#[test]
fn test_list_default() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
        ])
        .output();

    check_list_full_output(&output.stdout, None);
}

#[test]
fn test_list_full() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--list-type",
            "full",
            // These are left in for debugging and are generally quite useful.
            "--cargo-verbose",
            "--cargo-verbose",
        ])
        .output();

    check_list_full_output(&output.stdout, None);

    // Test oneline format: output should be one test per line with format "binary_id test_name".
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "oneline",
        ])
        .output();

    check_list_oneline_output(&output.stdout_as_str());

    // Test auto format: when stdout is not a TTY, auto should produce oneline format.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "auto",
        ])
        .output();

    // Auto format should produce oneline when stdout is not a TTY.
    check_list_oneline_output(&output.stdout_as_str());
}

#[test]
fn test_list_binaries_only() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--list-type",
            "binaries-only",
        ])
        .output();

    check_list_binaries_output(&output.stdout);

    // Check error messages for unknown binary IDs.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--cargo-quiet",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--list-type",
            "binaries-only",
            // Doesn't exist.
            "-E",
            "binary(unknown) & binary_id(unknown) & binary(=unknown) | binary_id(=unknown)",
            // Does exist.
            "-E",
            "binary_id(with-build-script) | binary_id(=with-build-script) | \
             binary(nextest-tests) | binary(=nextest-tests)",
            // First one doesn't exist, second one does.
            "-E",
            "binary_id(nextest-tests::does_not_exist) | binary_id(=nextest-tests::basic)",
            // First one exists, second one doesn't.
            "-E",
            "binary_id(nextest-tests::example/*) | binary_id(dylib-test::example/*)",
        ])
        .unchecked(true)
        .output();

    insta::assert_snapshot!(output.stderr_as_str());

    // Test oneline format for binaries-only: output should be one binary per line.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "oneline",
            "--list-type",
            "binaries-only",
        ])
        .output();

    check_list_oneline_binaries_output(&output.stdout_as_str());
}

#[test]
fn test_target_dir() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();

    std::env::set_current_dir(p.workspace_root())
        .expect("changed current directory to workspace root");

    let run_check = |target_dir: &str, extra_args: Vec<&str>| {
        // The test is for the target directory more than for any specific package, so pick a
        // package that builds quickly.
        let output = CargoNextestCli::for_test(&env_info)
            .args(["list", "-p", "cdylib-example", "--message-format", "json"])
            .args(extra_args)
            .output();

        let summary = output.decode_test_list_json().unwrap();
        assert_eq!(
            summary.rust_build_meta.target_directory,
            p.workspace_root().join(target_dir),
            "target directory matches"
        );
    };

    // Absolute target directory
    {
        let abs_target_dir = p.workspace_root().join("test-target-dir-abs");
        run_check(
            "test-target-dir-abs",
            vec!["--target-dir", abs_target_dir.as_str()],
        );
    }

    // Relative target directory
    run_check("test-target-dir", vec!["--target-dir", "test-target-dir"]);

    // CARGO_TARGET_DIR env var
    {
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("CARGO_TARGET_DIR", "test-target-dir-2") };
        run_check("test-target-dir-2", vec![]);
    }

    // CARGO_TARGET_DIR env var + --target-dir
    run_check(
        "test-target-dir-3",
        vec!["--target-dir", "test-target-dir-3"],
    );

    cfg_if::cfg_if! {
        if #[cfg(unix)] {
            // Symlink (should not be dereferenced, same as cargo)
            std::fs::create_dir("symlink-target").unwrap();
            std::os::unix::fs::symlink("symlink-target", "symlink-link").unwrap();
            run_check("symlink-link", vec!["--target-dir", "symlink-link"]);
        }
    }
}

#[test]
fn test_list_full_after_build() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();
    save_binaries_metadata(&env_info, &p);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--binaries-metadata",
            p.binaries_metadata_path().as_str(),
            "--message-format",
            "json",
        ])
        .output();

    check_list_full_output(&output.stdout, None);
}

#[test]
fn test_list_host_after_build() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();
    save_binaries_metadata(&env_info, &p);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--binaries-metadata",
            p.binaries_metadata_path().as_str(),
            "--message-format",
            "json",
            "-E",
            "platform(host)",
        ])
        .output();

    check_list_full_output(&output.stdout, Some(BuildPlatform::Host));
}

#[test]
fn test_list_target_after_build() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();
    save_binaries_metadata(&env_info, &p);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--binaries-metadata",
            p.binaries_metadata_path().as_str(),
            "--message-format",
            "json",
            "-E",
            "platform(target)",
        ])
        .output();

    check_list_full_output(&output.stdout, Some(BuildPlatform::Target));
}

#[test]
fn test_run_no_tests() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();
    save_binaries_metadata(&env_info, &p);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "-E",
            "none()",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "correct exit code for command\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("Starting 0 tests across 0 binaries (7 binaries skipped)"),
        "stderr contains 'Starting' message: {output}"
    );
    assert!(
        stderr.contains("error: no tests to run\n(hint: use `--no-tests` to customize)"),
        "stderr contains no tests message: {output}"
    );

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "-E",
            "none()",
            "--no-tests",
            "warn",
        ])
        .output();

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("warning: no tests to run"),
        "stderr contains no tests message: {output}"
    );

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "-E",
            "none()",
            "--no-tests=fail",
        ])
        .unchecked(true)
        .output();
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "correct exit code for command\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("error: no tests to run"),
        "stderr contains no tests message: {output}"
    );

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "-E",
            "none()",
            "--no-tests=pass",
        ])
        .output();

    let stderr = output.stderr_as_str();
    assert!(
        !stderr.contains("no tests to run"),
        "no tests message does not error out, stderr: {output}"
    );
}

#[test]
fn test_run() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::empty(),
    );

    // --exact with nothing else should be the same as above.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--",
            "--exact",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::empty(),
    );

    // Check the output with --skip.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--",
            "--skip",
            "cdylib",
        ])
        .unchecked(true)
        .output();
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER,
    );

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-E",
            "not test(cdylib)",
        ])
        .unchecked(true)
        .output();
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER,
    );

    // Check the output with --exact.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--",
            "test_multiply_two",
            "--exact",
            "tests::test_multiply_two_cdylib",
        ])
        // The above tests pass so don't pass in unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-E",
            "test(=test_multiply_two) | test(=tests::test_multiply_two_cdylib)",
        ])
        // The above tests pass so don't pass in unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );

    // Check the output with --exact and --skip.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--",
            "test_multiply_two",
            // Note the position of --exact doesn't matter.
            "--exact",
            "tests::test_multiply_two_cdylib",
            "--skip",
            "tests::test_multiply_two_cdylib",
        ])
        // This should only select the one test_multiply_two test, which pass. So don't pass in
        // unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER | RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-E",
            "(test(=test_multiply_two) | test(=tests::test_multiply_two_cdylib)) & not test(=tests::test_multiply_two_cdylib)",
        ])
        // This should only select the test_multiply_two test, which passes. So don't pass in
        // unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER | RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );

    // Another equivalent.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-E",
            "test(=test_multiply_two) | test(=tests::test_multiply_two_cdylib)",
            "--",
            "--skip",
            "cdylib",
        ])
        // This should only select the test_multiply_two test, which passes. So don't pass in
        // unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER | RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );

    // Yet another equivalent.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-E",
            "not test(cdylib)",
            "--",
            "test_multiply_two",
            "--exact",
            "tests::test_multiply_two_cdylib",
        ])
        // This should only select the test_multiply_two test, which passes. So don't pass in
        // unchecked(true) here.
        .output();
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_SKIP_CDYLIB_FILTER | RunProperties::WITH_MULTIPLY_TWO_EXACT_FILTER,
    );
}

#[test]
fn test_run_after_build() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();
    save_binaries_metadata(&env_info, &p);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--binaries-metadata",
            p.binaries_metadata_path().as_str(),
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::empty(),
    );
}

/// Test that per-benchmark override for bench.slow-timeout is respected.
///
/// The profile has a 30-year bench.slow-timeout at the profile level, but an
/// override that sets a 1s timeout for `bench_slow_timeout`. The benchmark
/// sleeps for 4 seconds, so it should time out.
#[test]
fn test_bench_override_slow_timeout() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "bench",
            "--profile",
            "with-bench-override",
            // Set the dev profile here to avoid a rebuild.
            "--cargo-profile",
            "dev",
            "--run-ignored",
            "only",
            "-E",
            "test(=bench_slow_timeout)",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command (benchmark should time out)\n{output}",
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-bench-override"),
        RunProperties::BENCH_OVERRIDE_TIMEOUT
            | RunProperties::BENCHMARKS
            | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that bench.slow-timeout causes benchmarks to time out.
#[test]
fn test_bench_termination() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "bench",
            "--profile",
            "with-bench-termination",
            // Use test profile to avoid rebuilding: `cargo nextest bench`
            // defaults to the `bench` cargo profile, but the fixture is already
            // built with `test`.
            "--cargo-profile",
            "test",
            "--run-ignored",
            "only",
            "-E",
            "test(=bench_slow_timeout)",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command (benchmark should time out)\n{output}",
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-bench-termination"),
        RunProperties::BENCH_TERMINATION
            | RunProperties::BENCHMARKS
            | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that benchmarks ignore regular slow-timeout and only use bench.slow-timeout.
///
/// The benchmark sleeps for 2 seconds. The profile has slow-timeout = 1s but no
/// bench.slow-timeout (defaults to 30 years). If benchmarks incorrectly used
/// slow-timeout, this would fail. But it passes because benchmarks use
/// bench.slow-timeout.
#[test]
fn test_bench_ignores_test_slow_timeout() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "bench",
            "--profile",
            "with-test-termination-only",
            // Use test profile to avoid rebuilding: `cargo nextest bench`
            // defaults to the `bench` cargo profile, but the fixture is already
            // built with `test`.
            "--cargo-profile",
            "test",
            "--run-ignored",
            "only",
            "-E",
            "test(=bench_slow_timeout)",
        ])
        .output();

    // The benchmark should pass (not time out), even though slow-timeout is 1s,
    // because benchmarks only use bench.slow-timeout which defaults to 30
    // years.
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::OK),
        "correct exit code for command (benchmark should succeed)\n{output}",
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-test-termination-only"),
        RunProperties::BENCH_IGNORES_TEST_TIMEOUT
            | RunProperties::BENCHMARKS
            | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

#[test]
fn test_relocated_run() {
    let env_info = set_env_vars_for_test();

    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path().join("target");
    let p = TempProject::new_custom_target_dir(&env_info, &custom_target_path).unwrap();
    save_binaries_metadata(&env_info, &p);
    save_cargo_metadata(&p);

    let mut p2 = TempProject::new(&env_info).unwrap();
    let new_target_path = p2.workspace_root().join("test-subdir/target");

    // copy target directory over
    std::fs::create_dir_all(&new_target_path).unwrap();
    temp_project::copy_dir_all(&custom_target_path, &new_target_path, false).unwrap();
    // Remove the old target path to ensure that any tests that refer to files within it
    // fail.
    std::fs::remove_dir_all(&custom_target_path).unwrap();

    p2.set_target_dir(new_target_path);

    // Use relative paths to the workspace root and target directory to do testing in.
    let current_dir: Utf8PathBuf = std::env::current_dir()
        .expect("able to get current directory")
        .try_into()
        .expect("current directory is valid UTF-8");
    let rel_workspace_root = pathdiff::diff_utf8_paths(p2.workspace_root(), &current_dir)
        .expect("diff of two absolute paths should work");
    let rel_target_dir = pathdiff::diff_utf8_paths(p2.target_dir(), &current_dir)
        .expect("diff of two absolute paths should work");

    // Run relocated tests

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p2.manifest_path().as_str(),
            "run",
            "--binaries-metadata",
            p2.binaries_metadata_path().as_str(),
            "--cargo-metadata",
            p2.cargo_metadata_path().as_str(),
            "--workspace-remap",
            rel_workspace_root.as_str(),
            "--target-dir-remap",
            rel_target_dir.as_str(),
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p2.junit_path("default"),
        RunProperties::RELOCATED,
    );
}

#[test]
fn test_run_with_priorities() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "-j1",
            "--profile",
            "with-priorities",
        ])
        .unchecked(true)
        // The above tests pass so don't pass in unchecked(true) here.
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}",
    );

    // -j1 means the tests should always run in the order specified in
    // `with-priorities`: `test_success`, then `test_flaky_mod_4`, then
    // `test_cargo_env_vars`.
    let stderr = output.stderr_as_str();
    let test_success = stderr
        .find("nextest-tests::basic test_success")
        .expect("test_success is present in output");
    let test_flaky_mod_4 = stderr
        .find("nextest-tests::basic test_flaky_mod_4")
        .expect("test_flaky_mod_4 is present in output");
    let test_cargo_env_vars = stderr
        .find("nextest-tests::basic test_cargo_env_vars")
        .expect("test_cargo_env_vars is present in output");

    assert!(
        test_success < test_flaky_mod_4,
        "test_success runs before test_flaky_mod_4\n{output}"
    );
    assert!(
        test_flaky_mod_4 < test_cargo_env_vars,
        "test_flaky_mod_4 runs before test_cargo_env_vars\n{output}"
    );
}

#[test]
fn test_run_from_archive_with_no_includes() {
    let env_info = set_env_vars_for_test();

    let (_p1, archive_file) =
        create_archive(&env_info, "", false, "archive_no_includes").expect("archive succeeded");
    let (_p2, extracted_target) = run_archive(&env_info, &archive_file);

    for path in [
        APP_DATA_DIR,
        TOP_LEVEL_FILE,
        TOP_LEVEL_DIR,
        TOP_LEVEL_DIR_OTHER_FILE,
    ] {
        _ = extracted_target
            .join(path)
            .symlink_metadata()
            .map(|_| panic!("file {path} must not be included in the archive"));
    }
}

#[test]
fn test_run_from_archive_with_includes() {
    let env_info = set_env_vars_for_test();

    let config = r#"
[profile.default]
archive.include = [
    { path = "application-data", relative-to = "target", on-missing = "error" },
    { path = "top-level-file.txt", relative-to = "target", depth = 0 },
    { path = "excluded-dir", relative-to = "target", depth = 0 },
    { path = "depth-0-dir", relative-to = "target", depth = 0 },
    { path = "file_that_does_not_exist.txt", relative-to = "target" },
    { path = "uds.sock", relative-to = "target" },
    { path = "missing-file", relative-to = "target", on-missing = "ignore" },  # should not be printed out
]"#;
    let (_p1, archive_file) =
        create_archive(&env_info, config, true, "archive_includes").expect("archive succeeded");
    let (_p2, extracted_target) = run_archive(&env_info, &archive_file);

    // TODO: we should test which of these paths above warn here, either by defining a serialization
    // format or via screen-scraping.

    // The included file should be present, but the excluded file should not.
    for path in [INCLUDED_PATH, TOP_LEVEL_FILE] {
        let contents = std::fs::read_to_string(extracted_target.join(path))
            .expect("extra file written to archive");
        assert_eq!(contents, "a test string");
    }

    for path in [EXCLUDED_PATH, TOP_LEVEL_DIR, TOP_LEVEL_DIR_OTHER_FILE] {
        _ = extracted_target
            .join(path)
            .symlink_metadata()
            .map(|_| panic!("file {path} must not be included in the archive"));
    }
}

#[test]
fn test_run_from_archive_with_missing_includes() {
    let env_info = set_env_vars_for_test();

    let config = r#"
[profile.default]
archive.include = [
    { path = "missing-file", relative-to = "target", on-missing = "error" },
]"#;
    create_archive(&env_info, config, false, "archive_missing_includes")
        .expect_err("archive should have failed");
}

#[test]
fn test_archive_with_build_filter() {
    let env_info = set_env_vars_for_test();

    let all_test_binaries: Vec<String> = EXPECTED_TEST_SUITES
        .iter()
        // The test fixture binary name uses hyphens, but the files will use an
        // underscore.
        .map(|fixture| fixture.binary_name.replace("-", "_"))
        .collect();

    // Check that all test files are present with the `all()` filter.
    check_archive_contents(&env_info, "all()", |env_info, archive_file, paths| {
        for file in all_test_binaries.iter() {
            assert!(
                paths
                    .iter()
                    .any(|path| path_contains_test_fixture_file(path, file)),
                "{:?} was missing from the test archive",
                file
            );
        }
        run_archive_with_args(
            env_info,
            &archive_file,
            RunProperties::RELOCATED,
            NextestExitCode::TEST_RUN_FAILED,
        );
    });

    // Check that no test files are present with the `none()` filter.
    check_archive_contents(&env_info, "none()", |env_info, archive_file, paths| {
        for file in all_test_binaries.iter() {
            if let Some(found) = paths
                .iter()
                .filter(|path| {
                    path.ancestors()
                        // Test binaries are in the `deps` folder.
                        .any(|folder| folder.file_name() == Some("deps"))
                })
                .find(|path| path_contains_test_fixture_file(path, file))
            {
                panic!(
                    "{} was present in the test archive as {}, but it should be missing",
                    file, found
                );
            }
        }
        run_archive_with_args(
            env_info,
            &archive_file,
            RunProperties::SKIP_SUMMARY_CHECK | RunProperties::EXPECT_NO_BINARIES,
            NextestExitCode::NO_TESTS_RUN,
        );
    });

    let expected_package_test_file = "cdylib_example";
    let filtered_test = "nextest_tests";
    // Check that test files are filtered by the `package()` filter.
    check_archive_contents(
        &env_info,
        "package(cdylib-example)",
        |env_info, archive_file, paths| {
            assert!(
                paths
                    .iter()
                    .any(|path| path_contains_test_fixture_file(path, expected_package_test_file)),
                "{:?} was missing from the test archive",
                expected_package_test_file
            );
            assert!(
                !paths
                    .iter()
                    .any(|path| path_contains_test_fixture_file(path, filtered_test)),
                "{:?} was present in the test archive but it should be missing",
                filtered_test
            );
            run_archive_with_args(
                env_info,
                &archive_file,
                RunProperties::CDYLIB_EXAMPLE_PACKAGE_FILTER | RunProperties::SKIP_SUMMARY_CHECK,
                NextestExitCode::OK,
            );
        },
    );
}

/// Checks if the file name at `path` contains `expected_file_name`
/// Returns `true` if it does, otherwise `false`.
fn path_contains_test_fixture_file(path: &Utf8Path, expected_file_name: &str) -> bool {
    let file_name = path.file_name().unwrap_or_else(|| {
        panic!(
            "test fixture path {:?} did not have a file name, does the path contain '..'?",
            path
        )
    });
    file_name.contains(expected_file_name)
}

#[test]
fn test_archive_with_unsupported_test_filter() {
    let env_info = set_env_vars_for_test();

    let unsupported_filter = "test(sample_test)";
    assert!(
        create_archive_with_args(
            &env_info,
            "",
            false,
            "archive_unsupported_build_filter",
            &["-E", unsupported_filter],
            true
        )
        .is_err()
    );
}

fn check_archive_contents(
    env_info: &TestEnvInfo,
    filter: &str,
    cb: impl FnOnce(&TestEnvInfo, Utf8PathBuf, Vec<Utf8PathBuf>),
) {
    let (_p1, archive_file) =
        create_archive_with_args(env_info, "", false, "", &["-E", filter], false)
            .expect("archive succeeded");
    let file = File::open(archive_file.clone()).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let paths = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned().try_into().unwrap())
        .collect::<Vec<_>>();
    cb(env_info, archive_file, paths);
}

const APP_DATA_DIR: &str = "application-data";
// The default limit is 16, so anything at depth 17 (under d16) is excluded.
const DIR_TREE: &str = "application-data/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16";
const INCLUDED_PATH: &str =
    "application-data/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/included.txt";
const EXCLUDED_PATH: &str =
    "application-data/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/excluded.txt";
const DIR_AT_DEPTH_0: &str = "depth-0-dir";
const UDS_PATH: &str = "uds.sock";

const TOP_LEVEL_FILE: &str = "top-level-file.txt";
const TOP_LEVEL_DIR: &str = "top-level-dir";
const TOP_LEVEL_DIR_OTHER_FILE: &str = "top-level-dir/other-file.txt";

fn create_archive(
    env_info: &TestEnvInfo,
    config_contents: &str,
    make_uds: bool,
    snapshot_name: &str,
) -> Result<(TempProject, Utf8PathBuf), CargoNextestOutput> {
    create_archive_with_args(
        env_info,
        config_contents,
        make_uds,
        snapshot_name,
        &[],
        true,
    )
}

fn create_archive_with_args(
    env_info: &TestEnvInfo,
    config_contents: &str,
    make_uds: bool,
    snapshot_name: &str,
    extra_args: &[&str],
    check_output: bool,
) -> Result<(TempProject, Utf8PathBuf), CargoNextestOutput> {
    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path().join("target");
    let p = TempProject::new_custom_target_dir(env_info, &custom_target_path).unwrap();

    let config_path = p.workspace_root().join(".config/nextest.toml");
    std::fs::write(config_path, config_contents).unwrap();

    // Setup extra files to include.
    let test_dir = p.target_dir().join(DIR_TREE);
    std::fs::create_dir_all(test_dir).unwrap();
    std::fs::write(p.target_dir().join(INCLUDED_PATH), "a test string").unwrap();
    std::fs::write(p.target_dir().join(EXCLUDED_PATH), "a test string").unwrap();

    let top_level_file = p.target_dir().join(TOP_LEVEL_FILE);
    std::fs::write(top_level_file, "a test string").unwrap();

    let top_level_dir = p.target_dir().join(TOP_LEVEL_DIR);
    let top_level_dir_other_file = p.target_dir().join(TOP_LEVEL_DIR_OTHER_FILE);
    std::fs::create_dir(top_level_dir).unwrap();
    std::fs::write(top_level_dir_other_file, "a test string").unwrap();

    // This produces a warning.
    std::fs::create_dir_all(p.target_dir().join(DIR_AT_DEPTH_0)).unwrap();

    // This produces a warning as well, since Unix domain sockets are unrecognized.
    let uds_created = if make_uds {
        create_uds(&p.target_dir().join(UDS_PATH)).unwrap()
    } else {
        UdsStatus::NotRequested
    };

    let archive_file = p.temp_root().join("my-archive.tar.zst");

    let manifest_path = p.manifest_path();
    let mut cli_args = vec![
        "--manifest-path",
        manifest_path.as_str(),
        "archive",
        "--archive-file",
        archive_file.as_str(),
        "--workspace",
        "--target-dir",
        p.target_dir().as_str(),
        "--all-targets",
        // Make cargo fully quiet since we're testing just nextest output below.
        "--cargo-quiet",
        "--cargo-quiet",
    ];
    cli_args.extend(extra_args);

    // Write the archive to the archive_file above.
    let output = CargoNextestCli::for_test(env_info)
        .args(cli_args)
        .env("__NEXTEST_REDACT", "1")
        // Used for linked path testing. See comment in
        // binary_list.rs:detect_linked_path.
        .env("__NEXTEST_ALT_TARGET_DIR", p.orig_target_dir())
        .unchecked(true)
        .output();

    // If a UDS was created, we're going to have a slightly different snapshot.
    let snapshot_name = match uds_created {
        UdsStatus::Created | UdsStatus::NotRequested => snapshot_name.to_string(),
        UdsStatus::NotCreated => format!("{snapshot_name}_without_uds"),
    };

    if check_output {
        insta::assert_snapshot!(snapshot_name, output.stderr_as_str());
    }

    // Remove the old source and target directories to ensure that any tests that refer to files within
    // it fail.
    std::fs::remove_dir_all(p.workspace_root()).unwrap();
    std::fs::remove_dir_all(p.target_dir()).unwrap();

    // project is included in return value to keep tempdirs alive
    if output.exit_status.success() {
        Ok((p, archive_file))
    } else {
        Err(output)
    }
}

fn run_archive(env_info: &TestEnvInfo, archive_file: &Utf8Path) -> (TempProject, Utf8PathBuf) {
    run_archive_with_args(
        env_info,
        archive_file,
        RunProperties::RELOCATED,
        NextestExitCode::TEST_RUN_FAILED,
    )
}

fn run_archive_with_args(
    env_info: &TestEnvInfo,
    archive_file: &Utf8Path,
    run_property: RunProperties,
    expected_exit_code: i32,
) -> (TempProject, Utf8PathBuf) {
    let p2 = TempProject::new(env_info).unwrap();
    let extract_to = p2.workspace_root().join("extract_to");
    std::fs::create_dir_all(&extract_to).unwrap();

    let output = CargoNextestCli::for_test(env_info)
        .args([
            "run",
            "--archive-file",
            archive_file.as_str(),
            "--workspace-remap",
            p2.workspace_root().as_str(),
            "--extract-to",
            extract_to.as_str(),
        ])
        .unchecked(true)
        .output();
    assert_eq!(
        output.exit_status.code(),
        Some(expected_exit_code),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(&output.stderr, &p2.junit_path("default"), run_property);

    // project is included in return value to keep tempdirs alive
    (p2, extract_to.join("target"))
}

#[test]
fn test_bench() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "bench",
            // Set the dev profile here to avoid a rebuild.
            "--cargo-profile",
            "dev",
            "--no-capture",
        ])
        .unchecked(true)
        .output();
    assert_eq!(
        output.exit_status.code(),
        Some(0),
        "correct exit code for command\n{output}",
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::BENCHMARKS,
    );
}

#[test]
fn test_show_config_test_groups() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let default_profile_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
        ])
        .output();

    insta::assert_snapshot!(default_profile_output.stdout_as_str());

    let default_profile_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
            "--show-default",
        ])
        .output();

    insta::assert_snapshot!(default_profile_all_output.stdout_as_str());

    let with_retries_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
            "--profile=with-retries",
        ])
        .output();

    insta::assert_snapshot!(with_retries_output.stdout_as_str());

    let with_retries_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
            "--profile=with-retries",
            "--show-default",
        ])
        .output();

    insta::assert_snapshot!(with_retries_all_output.stdout_as_str());

    let with_termination_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
            "--profile=with-termination",
        ])
        .output();

    insta::assert_snapshot!(with_termination_output.stdout_as_str());

    let with_termination_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "test-groups",
            "--workspace",
            "--all-targets",
            "--profile=with-termination",
            "--show-default",
        ])
        .output();

    insta::assert_snapshot!(with_termination_all_output.stdout_as_str());
}

#[test]
fn test_list_with_default_filter() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Show the output of the default filter (does not include tests not in default-filter).
    let default_set_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_basic",
        default_filter_stdout(&default_set_output)
    );

    // Show the output with -E 'all()' (does not include tests not in default-filter).
    let all_tests_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "-E",
            "all()",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_expr_all",
        default_filter_stdout(&all_tests_output)
    );

    // Show the output with --ignore-default-filter (does include tests not in default-filter).
    let bound_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "--workspace",
            "--all-targets",
            "--ignore-default-filter",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_bound_all",
        bound_all_output.stdout_as_str()
    );

    // -E 'default()' --ignore-default-filter (same as no arguments).
    let default_tests_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "-E",
            "default()",
            "--ignore-default-filter",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_expr_default",
        default_filter_stdout(&default_tests_output)
    );
    assert_eq!(
        default_tests_output.stdout_as_str(),
        default_set_output.stdout_as_str(),
        "default() and no arguments are the same"
    );

    // -E 'package(cdylib-example)' (empty)
    let package_example_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "-E",
            "package(cdylib-example)",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_expr_package",
        package_example_output.stdout_as_str(),
    );

    // -E 'package(cdylib-example)' --ignore-default-filter (includes cdylib-example).
    let package_example_bound_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "-E",
            "package(cdylib-example)",
            "--ignore-default-filter",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_expr_package_bound_all",
        package_example_bound_all_output.stdout_as_str(),
    );

    // With additional regular arguments passed in (should be affected by the default fitler).
    let with_args_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "test_stdin_closed",
            "cdylib",
            "--workspace",
            "--all-targets",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_args",
        with_args_output.stdout_as_str(),
    );

    // With --ignore-default-filter.
    let with_args_bound_all_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--profile=with-default-filter",
            "test_stdin_closed",
            "cdylib",
            "--workspace",
            "--all-targets",
            "--ignore-default-filter",
        ])
        .output();
    insta::assert_snapshot!(
        "list_with_default_set_args_bound_all",
        with_args_bound_all_output.stdout_as_str(),
    );
}

#[cfg(unix)]
fn default_filter_stdout(output: &CargoNextestOutput) -> Cow<'_, str> {
    output.stdout_as_str()
}

#[cfg(not(unix))]
#[track_caller]
fn default_filter_stdout(output: &CargoNextestOutput) -> Cow<'_, str> {
    // On Unix platforms, we additionally filter out `test_cargo_env_vars` here
    // as a test. Ensure that on non-Unix platforms it is present in the output,
    // and remove it from the output.
    let stdout = output.stdout_as_str();
    assert!(
        stdout.contains("test_cargo_env_vars"),
        "test_cargo_env_vars should be in the output:\n------\n{output}"
    );

    itertools::Itertools::intersperse(
        stdout
            .lines()
            .filter(|line| !line.contains("test_cargo_env_vars")),
        "\n",
    )
    .chain(std::iter::once("\n"))
    .collect()
}

#[test]
fn test_run_with_default_filter() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile=with-default-filter",
            "--workspace",
            "--all-targets",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-default-filter"),
        RunProperties::WITH_DEFAULT_FILTER,
    );
}

#[test]
fn test_show_config_version() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // This is the same as dispatch.rs.
    const TEST_VERSION_ENV: &str = "__NEXTEST_TEST_VERSION";

    // Required 0.9.56, recommended 0.9.54.

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.56")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.55-a.1")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.54")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.54-rc.1")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::REQUIRED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.53")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::REQUIRED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    // ---
    // With --override-version-check
    // ---
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
            "--override-version-check",
        ])
        .env(TEST_VERSION_ENV, "0.9.55")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
            "--override-version-check",
        ])
        .env(TEST_VERSION_ENV, "0.9.53")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());

    // Add an invalid test group to the config file.
    let config_path = p.workspace_root().join(".config/nextest.toml");
    let mut f = File::options()
        .append(true)
        .create(false)
        .open(config_path)
        .unwrap();
    f.write_all(
        r#"
    [test-groups.invalid-group]
    max-threads = { foo = 42 }
    "#
        .as_bytes(),
    )
    .unwrap();
    f.flush().unwrap();
    std::mem::drop(f);

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
            "--override-version-check",
        ])
        .env(TEST_VERSION_ENV, "0.9.53")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());
}

/// Tests that the version error takes precedence over unknown experimental features.
///
/// When a config has both a future nextest-version requirement and an unknown
/// experimental feature, the version error should be shown first. This is
/// because a future version may have new experimental features that the current
/// version doesn't know about.
#[test]
fn test_version_error_precedes_unknown_experimental() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    const TEST_VERSION_ENV: &str = "__NEXTEST_TEST_VERSION";

    // Create a config with both a future version and an unknown experimental feature.
    let config_path = p.workspace_root().join(".config/nextest.toml");
    std::fs::write(
        &config_path,
        r#"
nextest-version = "0.9.9999"
experimental = ["setup-scripts", "unknown-experimental-feature"]

[profile.default]
fail-fast = false
"#,
    )
    .unwrap();

    // Run with a "current" version that doesn't meet the requirement.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--message-format",
            "human",
        ])
        .env(TEST_VERSION_ENV, "0.9.100")
        .unchecked(true)
        .output();

    // Should get the version error, not the unknown experimental feature error.
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::REQUIRED_VERSION_NOT_MET),
        "expected REQUIRED_VERSION_NOT_MET exit code, got {:?}\nstderr: {}",
        output.exit_status.code(),
        output.stderr_as_str()
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("requires nextest version 0.9.9999"),
        "expected version error in stderr, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("unknown-experimental-feature"),
        "should not contain unknown experimental feature error, got: {}",
        stderr
    );

    // Now test that the unknown experimental feature error is shown when the version passes.
    std::fs::write(
        &config_path,
        r#"
nextest-version = "0.9.50"
experimental = ["setup-scripts", "unknown-experimental-feature"]

[profile.default]
fail-fast = false
"#,
    )
    .unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--message-format",
            "human",
        ])
        .env(TEST_VERSION_ENV, "0.9.100")
        .unchecked(true)
        .output();

    // Should get the unknown experimental feature error now.
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "expected SETUP_ERROR exit code for unknown experimental feature, got {:?}\nstderr: {}",
        output.exit_status.code(),
        output.stderr_as_str()
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("unknown-experimental-feature"),
        "expected unknown experimental feature error in stderr, got: {}",
        stderr
    );
}

/// Test that unknown experimental features in table format cause an error.
///
/// This is consistent with the array format behavior: unknown features in repo
/// config cause an error regardless of format.
#[test]
fn test_experimental_table_format_unknown_error() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Create a config with an [experimental] table containing an unknown
    // experimental feature.
    let config_path = p.workspace_root().join(".config/nextest.toml");
    std::fs::write(
        &config_path,
        r#"
[experimental]
setup-scripts = true
unknown-feature = true

[profile.default]
fail-fast = false
"#,
    )
    .unwrap();

    // cargo nextest list should fail with an error.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--message-format",
            "human",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "expected SETUP_ERROR exit code for unknown experimental feature, got {:?}\nstderr: {}",
        output.exit_status.code(),
        output.stderr_as_str()
    );

    // The error message should contain the unknown feature name.
    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("unknown experimental features defined: unknown-feature"),
        "expected unknown-feature in stderr, got: {}",
        stderr
    );
}

/// Tests that valid experimental features in table format work correctly.
#[test]
fn test_experimental_table_format_valid() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Create a config with an [experimental] table.
    let config_path = p.workspace_root().join(".config/nextest.toml");
    std::fs::write(
        &config_path,
        r#"
[experimental]
setup-scripts = true
"#,
    )
    .unwrap();

    // cargo nextest list should succeed.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--message-format",
            "human",
        ])
        .unchecked(true)
        .output();

    assert!(
        output.exit_status.success(),
        "expected success exit code with table format, got {:?}\nstderr: {}",
        output.exit_status.code(),
        output.stderr_as_str()
    );

    // Nextest should log that experimental features are enabled.
    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("info: experimental features enabled: setup-scripts"),
        "expected 'setup-scripts' in stderr (enabled feature), got: {}",
        stderr
    );
}

#[test]
fn test_setup_scripts_not_enabled() {
    let env_info = set_env_vars_for_test();

    let p = TempProject::new(&env_info).unwrap();

    // Remove the "experimental" line from the config file.
    let config_path = p.workspace_root().join(".config/nextest.toml");
    let s = std::fs::read_to_string(&config_path).unwrap();
    let mut out = String::new();
    for line in s.lines() {
        if !line.starts_with("experimental") {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(&config_path, out).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args(["run", "--manifest-path", p.manifest_path().as_str()])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED)
    );
}

#[test]
fn test_setup_script_error() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args(["run", "--manifest-path", p.manifest_path().as_str()])
        .env("__NEXTEST_SETUP_SCRIPT_ERROR", "1")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_SCRIPT_FAILED),
        "expected exit code to be SETUP_SCRIPT_FAILED\noutput: {output}",
    );
}

#[test]
fn test_setup_script_defined_env() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args(["run", "-E", "test(test_cargo_env_vars)"])
        // Changing the current dir to where the manifest resides to ensure the `.cargo/config`
        // over there is picked up rather than the config for the main nextest project.
        .current_dir(
            p.manifest_path()
                .parent()
                .expect("manifest_path's parent should be a dir"),
        )
        .env("CMD_ENV_VAR", "not-set-in-conf")
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::OK),
        "env var should not override the value defined in the conf file\n{output}"
    );
}

#[test]
fn test_setup_script_reserved_env() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args(["run", "--manifest-path", p.manifest_path().as_str()])
        .env("__NEXTEST_SETUP_SCRIPT_RESERVED_ENV", "1")
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_SCRIPT_FAILED),
        "expected exit code to be SETUP_SCRIPT_FAILED\noutput: {output}",
    );
}

#[test]
fn test_target_arg() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let build_target_platform =
        Platform::build_target().expect("should detect the host target successfully");
    let host_triple = build_target_platform.triple_str();
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--target",
            host_triple,
            "--message-format",
            "json",
        ])
        .output();
    let result: TestListSummary = serde_json::from_slice(&output.stdout).unwrap();
    let build_platforms = &result
        .rust_build_meta
        .platforms
        .expect("should have the platforms field");

    // Target features get reset to unknown, unfortunately, so we can't compare
    // the full platform.
    let mut summary = build_target_platform.to_summary();
    summary.target_features = TargetFeaturesSummary::Unknown;
    assert_eq!(build_platforms.host.platform, summary);

    assert_eq!(build_platforms.targets[0].platform.triple, host_triple);
    assert_eq!(
        build_platforms.targets[0].libdir,
        build_platforms.host.libdir
    );
}

#[test]
fn test_rustc_version_verbose_errors() {
    let env_info = set_env_vars_for_test();

    // Set RUSTC to the shim.
    let shim_rustc = &env_info.rustc_shim_bin;

    let mut command = CargoNextestCli::for_test(&env_info);
    command
        .args(["debug", "build-platforms", "--output-format", "triple"])
        .env("RUSTC", shim_rustc);

    // --- Error cases ---
    {
        let mut command = command.clone();
        command
            .env("__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR", "non-zero")
            .env("__NEXTEST_FORCE_BUILD_TARGET", "error");
        insta::assert_snapshot!(
            "rustc_vv_non_zero",
            command.unchecked(true).output().to_snapshot()
        );
    }

    {
        let mut command = command.clone();
        command
            .env(
                "__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR",
                "invalid-stdout",
            )
            .env("__NEXTEST_FORCE_BUILD_TARGET", "error");
        insta::assert_snapshot!(
            "rustc_vv_invalid_stdout",
            command.unchecked(true).output().to_snapshot()
        );
    }

    {
        let mut command = command.clone();
        command
            .env(
                "__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR",
                "invalid-triple",
            )
            .env("__NEXTEST_FORCE_BUILD_TARGET", "error");

        insta::assert_snapshot!(
            "rustc_vv_invalid_triple",
            command.unchecked(true).output().to_snapshot()
        );
    }

    // --- Warning cases ---
    {
        let mut command = command.clone();
        command
            .env("__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR", "non-zero")
            .env("__NEXTEST_FORCE_BUILD_TARGET", "x86_64-unknown-linux-gnu");
        insta::assert_snapshot!(
            "rustc_vv_non_zero_warning",
            command.unchecked(true).output().to_snapshot()
        );
    }

    {
        let mut command = command.clone();
        command
            .env(
                "__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR",
                "invalid-stdout",
            )
            .env("__NEXTEST_FORCE_BUILD_TARGET", "x86_64-unknown-linux-gnu");
        insta::assert_snapshot!(
            "rustc_vv_invalid_stdout_warning",
            command.unchecked(true).output().to_snapshot()
        );
    }

    {
        let mut command = command.clone();
        command
            .env(
                "__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR",
                "invalid-triple",
            )
            .env("__NEXTEST_FORCE_BUILD_TARGET", "x86_64-unknown-linux-gnu");
        insta::assert_snapshot!(
            "rustc_vv_invalid_triple_warning",
            command.unchecked(true).output().to_snapshot()
        );
    }
}

/// Test that filterset expressions combined with string filters work correctly.
#[test]
fn test_filterset_with_string_filters() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Expression: test(test_multiply_two) | test(=tests::call_dylib_add_two)
    // String filters: call_dylib_add_two, test_flaky_mod_4
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "-E",
            "test(test_multiply_two) | test(=tests::call_dylib_add_two)",
            "--",
            "call_dylib_add_two",
            "test_flaky_mod_4",
        ])
        .output();

    let summary: TestListSummary =
        serde_json::from_slice(&output.stdout).expect("failed to parse test list JSON");

    // Verify specific test cases:
    for (binary_id, suite) in &summary.rust_suites {
        for (test_name, test_info) in &suite.test_cases {
            let full_name = format!("{} {}", binary_id, test_name);

            if *test_name == TestCaseName::new("tests::call_dylib_add_two") {
                // Matches both expression and string filter.
                assert!(
                    test_info.filter_match.is_match(),
                    "{full_name}: expected match, got {:?}",
                    test_info.filter_match
                );
            } else if test_name.contains("test_multiply_two") {
                // Matches expression but not string filter.
                assert_eq!(
                    test_info.filter_match,
                    FilterMatch::Mismatch {
                        reason: MismatchReason::String
                    },
                    "{full_name}: expected String mismatch"
                );
            } else if test_name.contains("test_flaky_mod_4") {
                // Matches string filter but not expression.
                assert_eq!(
                    test_info.filter_match,
                    FilterMatch::Mismatch {
                        reason: MismatchReason::Expression
                    },
                    "{full_name}: expected Expression mismatch"
                );
            } else {
                // Should not match.
                assert!(
                    !test_info.filter_match.is_match(),
                    "{full_name}: expected no match, got {:?}",
                    test_info.filter_match
                );
            }
        }
    }
}

/// Test that filterset expressions without string filters work correctly.
#[test]
fn test_filterset_without_string_filters() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Expression only, no string filters.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "-E",
            "test(test_multiply_two) | test(=tests::call_dylib_add_two)",
        ])
        .output();

    let summary: TestListSummary =
        serde_json::from_slice(&output.stdout).expect("failed to parse test list JSON");

    for (binary_id, suite) in &summary.rust_suites {
        for (test_name, test_info) in &suite.test_cases {
            let full_name = format!("{} {}", binary_id, test_name);

            if test_name.contains("test_multiply_two")
                || *test_name == TestCaseName::new("tests::call_dylib_add_two")
            {
                assert!(
                    test_info.filter_match.is_match(),
                    "{full_name}: expected match, got {:?}",
                    test_info.filter_match
                );
            } else {
                assert!(
                    !test_info.filter_match.is_match(),
                    "{full_name}: expected no match, got {:?}",
                    test_info.filter_match
                );
            }
        }
    }
}

/// Test that string filters without filterset expressions work correctly.
#[test]
fn test_string_filters_without_filterset() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // String filters only, no expression.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--",
            "test_multiply_two",
            "tests::call_dylib_add_two",
        ])
        .output();

    let summary: TestListSummary =
        serde_json::from_slice(&output.stdout).expect("failed to parse test list JSON");

    for (binary_id, suite) in &summary.rust_suites {
        for (test_name, test_info) in &suite.test_cases {
            let full_name = format!("{} {}", binary_id, test_name);

            if test_name.contains("test_multiply_two")
                || test_name.contains("tests::call_dylib_add_two")
            {
                assert!(
                    test_info.filter_match.is_match(),
                    "{full_name}: expected match, got {:?}",
                    test_info.filter_match
                );
            } else {
                assert!(
                    !test_info.filter_match.is_match(),
                    "{full_name}: expected no match, got {:?}",
                    test_info.filter_match
                );
            }
        }
    }
}

/// Test that `--run-ignored only` runs only ignored tests.
#[test]
fn test_run_ignored() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--run-ignored",
            "only",
            "-E",
            // Filter out slow timeout tests to avoid long test times.
            "not test(slow_timeout)",
        ])
        .unchecked(true)
        .output();

    // The run should fail because some ignored tests fail (e.g., test_ignored_fail).
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::RUN_IGNORED_ONLY,
    );
}

/// Test that timed out tests with `on-timeout=pass` are not retried.
///
/// The profile `with-timeout-retries-success` has:
/// - `retries = 2`
/// - `slow-timeout = { period = "500ms", terminate-after = 2, on-timeout = "pass" }`
///
/// Tests that time out with `on-timeout = "pass"` should be marked as passed.
#[test]
fn test_timeout_with_retries() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile",
            "with-timeout-retries-success",
            "--run-ignored",
            "only",
            "-E",
            // Only run the slow timeout tests (not the flaky one).
            "test(/^test_slow_timeout/)",
        ])
        .output();

    // Should succeed because on-timeout=pass marks timed out tests as passed.
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::OK),
        "timed out tests with on-timeout=pass should succeed\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-timeout-retries-success"),
        RunProperties::TIMEOUT_RETRIES_PASS | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that a flaky test that times out on the 3rd attempt is handled correctly.
///
/// The test `test_flaky_slow_timeout_mod_3`:
/// - Fails on attempts 1 and 2 (attempt % 3 != 0)
/// - Times out on attempt 3 (attempt % 3 == 0, so it sleeps until timeout)
///
/// With the `with-timeout-retries-success` profile (retries=2, on-timeout=pass),
/// the test should be marked as flaky (failed twice, then passed via timeout).
#[test]
fn test_timeout_with_flaky() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile",
            "with-timeout-retries-success",
            "--run-ignored",
            "only",
            "-E",
            "test(test_flaky_slow_timeout_mod_3)",
        ])
        .output();

    // Should succeed because the test eventually passes (via timeout with on-timeout=pass).
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::OK),
        "flaky test that eventually times out with on-timeout=pass should succeed\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-timeout-retries-success"),
        RunProperties::TIMEOUT_RETRIES_FLAKY | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that retries work correctly with the with-retries profile.
///
/// The `with-retries` profile has:
/// - `retries = 2` (default)
/// - Override for test_flaky_mod_6: `retries = 5`
/// - Override for test_flaky_mod_4: `retries = 4`
///
/// Flaky tests should eventually pass after the configured number of retries.
#[test]
fn test_retries() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
            "--profile",
            "with-retries",
        ])
        .unchecked(true)
        .output();

    // Should fail because some tests fail even after retries.
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "test run should fail due to failing tests\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-retries"),
        RunProperties::WITH_RETRIES | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that tests time out correctly with the with-termination profile.
///
/// The `with-termination` profile has slow-timeout configured such that tests
/// time out after 2 seconds. The test_slow_timeout* tests sleep for longer than
/// this and should all be terminated.
#[test]
fn test_termination() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile",
            "with-termination",
            "--run-ignored",
            "only",
            "-E",
            "test(/^test_slow_timeout/)",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "tests should time out and fail\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-termination"),
        RunProperties::WITH_TERMINATION | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Test that on-timeout = "pass" works correctly with the with-timeout-success profile.
///
/// The `with-timeout-success` profile has an override for test_slow_timeout that
/// sets on-timeout = "pass", so it should pass when it times out. The other
/// test_slow_timeout* tests should fail with timeout.
#[test]
fn test_override_timeout_result() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile",
            "with-timeout-success",
            "--run-ignored",
            "only",
            "-E",
            "test(/^test_slow_timeout/)",
        ])
        .unchecked(true)
        .output();

    // Should fail because test_slow_timeout_2 and test_slow_timeout_subprocess
    // time out without on-timeout = "pass".
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "some tests should time out and fail\n{output}"
    );
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("with-timeout-success"),
        RunProperties::WITH_TIMEOUT_SUCCESS | RunProperties::SKIP_SUMMARY_CHECK,
    );
}

/// Returns the CARGO_TARGET_<triple>_RUNNER env var name for the current platform.
fn current_runner_env_var() -> String {
    let platform = Platform::build_target().expect("current platform is known to target-spec");
    let triple = platform.triple_str().to_uppercase().replace('-', "_");
    format!("CARGO_TARGET_{triple}_RUNNER")
}

/// Test that listing works correctly with a target runner set.
#[test]
fn test_listing_with_target_runner() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Get the passthrough binary path.
    let passthrough = env_info.passthrough_bin.as_str();

    // First, list without target runner to get baseline counts.
    let baseline_output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
        ])
        .output();

    let baseline: TestListSummary =
        serde_json::from_slice(&baseline_output.stdout).expect("parse baseline JSON");

    // Now list with target runner set.
    let runner_env = current_runner_env_var();
    let runner_value = format!("{passthrough} --ensure-this-arg-is-sent");

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
        ])
        .env(&runner_env, &runner_value)
        .output();

    let with_runner: TestListSummary =
        serde_json::from_slice(&output.stdout).expect("parse with-runner JSON");

    // Verify the counts are the same.
    assert_eq!(
        baseline.rust_suites.len(),
        with_runner.rust_suites.len(),
        "binary counts should match"
    );

    let baseline_test_count: usize = baseline
        .rust_suites
        .values()
        .map(|s| s.test_cases.len())
        .sum();
    let with_runner_test_count: usize = with_runner
        .rust_suites
        .values()
        .map(|s| s.test_cases.len())
        .sum();
    assert_eq!(
        baseline_test_count, with_runner_test_count,
        "test counts should match"
    );
}

/// Test that running tests works correctly with a target runner set.
#[test]
fn test_run_with_target_runner() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Get the passthrough binary path.
    let passthrough = env_info.passthrough_bin.as_str();

    let runner_env = current_runner_env_var();
    let runner_value = format!("{passthrough} --ensure-this-arg-is-sent");

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--all-targets",
        ])
        .env(&runner_env, &runner_value)
        .unchecked(true)
        .output();

    // Should fail because some tests fail (same as without target runner).
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );

    // On Unix, segfaults aren't passed through correctly by the passthrough runner
    // (they show as regular failures), so we use a special property flag.
    check_run_output_with_junit(
        &output.stderr,
        &p.junit_path("default"),
        RunProperties::WITH_TARGET_RUNNER,
    );
}
