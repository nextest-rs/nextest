// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the record-replay feature.
//!
//! These tests verify that nextest can record test runs to disk and replay them later.

use crate::{fixtures::check_run_output, temp_project::TempProject};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use fixture_data::models::RunProperties;
use integration_tests::{
    env::{TestEnvInfo, set_env_vars_for_test},
    nextest_cli::CargoNextestCli,
};
use nextest_metadata::NextestExitCode;
use regex::Regex;
use std::{fs, sync::LazyLock};

/// Expected files in the store.zip archive.
const EXPECTED_ARCHIVE_FILES: &[&str] = &[
    "meta/cargo-metadata.json",
    "meta/test-list.json",
    "meta/record-opts.json",
    "meta/format.json",
    "meta/stdout.dict",
    "meta/stderr.dict",
    // out/ directory contains content-addressed output files (variable names).
];

/// Environment variable to override the nextest cache directory.
///
/// This is the same constant as `nextest_runner::record::NEXTEST_CACHE_DIR_ENV`.
const NEXTEST_CACHE_DIR_ENV: &str = "NEXTEST_CACHE_DIR";

/// Environment variable to force a specific run ID (for testing).
///
/// This is the same constant as in `nextest_runner::runner::imp`.
const FORCE_RUN_ID_ENV: &str = "__NEXTEST_FORCE_RUN_ID";

/// Environment variable to enable redaction of dynamic fields (timestamps, durations, sizes).
///
/// When set to "1", nextest produces fixed-width placeholders for these fields,
/// preserving column alignment in the output.
const NEXTEST_REDACT_ENV: &str = "__NEXTEST_REDACT";

/// Regex for matching timestamps in output (e.g., "2026-01-17 12:24:08").
static TIMESTAMP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}").unwrap());

/// Regex for matching bracketed durations in output (e.g., "[   0.024s]").
static BRACKETED_DURATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*\d+\.\d+s\]").unwrap());

/// User config content that enables recording.
///
/// The `record` experimental feature must be enabled AND `[record] enabled = true`
/// must be set for recording to occur.
const RECORD_USER_CONFIG: &str = r#"
experimental = ["record"]

[record]
enabled = true
"#;

/// Creates a user config file that enables recording.
///
/// Returns the temp dir (which must be kept alive) and the path to the config file.
fn create_record_user_config() -> (Utf8TempDir, Utf8PathBuf) {
    let temp_dir = camino_tempfile::Builder::new()
        .prefix("nextest-record-config-")
        .tempdir()
        .expect("created temp dir for record user config");

    let config_path = temp_dir.path().join("config.toml");
    fs::write(&config_path, RECORD_USER_CONFIG).expect("wrote record user config file");

    (temp_dir, config_path)
}

/// Creates a cache directory inside the temp project and returns its path.
///
/// Tests should set `NEXTEST_CACHE_DIR` to this path to ensure recordings
/// are stored within the temp directory, making cleanup automatic and
/// path redaction simple.
fn create_cache_dir(p: &TempProject) -> Utf8PathBuf {
    let cache_dir = p.temp_root().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("cache directory should be created");
    cache_dir
}

/// Returns a CLI builder with the manifest path set.
fn cli_for_project(env_info: &TestEnvInfo, p: &TempProject) -> CargoNextestCli {
    let mut cli = CargoNextestCli::for_test(env_info);
    cli.args(["--manifest-path", p.manifest_path().as_str()]);
    cli
}

/// Returns a CLI builder with recording enabled and cache directory configured.
///
/// This helper:
/// 1. Sets the manifest path
/// 2. Sets `--user-config-file` to the provided config path (which must have
///    `experimental = ["record"]` and `[record] enabled = true`)
/// 3. Sets `NEXTEST_CACHE_DIR` to a directory inside the temp project
/// 4. Optionally sets `__NEXTEST_FORCE_RUN_ID` for deterministic run IDs
/// 5. Sets `__NEXTEST_REDACT=1` to produce fixed-width placeholders for
///    timestamps, durations, and sizes, preserving column alignment
fn cli_with_recording(
    env_info: &TestEnvInfo,
    p: &TempProject,
    cache_dir: &Utf8Path,
    user_config_path: &Utf8Path,
    run_id: Option<&str>,
) -> CargoNextestCli {
    let mut cli = cli_for_project(env_info, p);
    cli.args(["--user-config-file", user_config_path.as_str()]);
    cli.env(NEXTEST_CACHE_DIR_ENV, cache_dir.as_str());
    cli.env(NEXTEST_REDACT_ENV, "1");
    if let Some(run_id) = run_id {
        cli.env(FORCE_RUN_ID_ENV, run_id);
    }
    cli
}

/// Returns the runs directory within the record store.
///
/// When using `NEXTEST_CACHE_DIR`, records are stored at:
/// `$NEXTEST_CACHE_DIR/projects/<encoded-workspace>/records/runs/`
fn find_runs_dir(cache_dir: &Utf8Path) -> Option<Utf8PathBuf> {
    // The runs directory is at: cache_dir/projects/<encoded>/records/runs
    let projects_dir = cache_dir.join("projects");
    if !projects_dir.exists() {
        return None;
    }

    // There should be exactly one encoded workspace directory.
    let entries: Vec<_> = std::fs::read_dir(&projects_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();

    if entries.len() != 1 {
        return None;
    }

    let encoded_dir = Utf8PathBuf::try_from(entries[0].path()).ok()?;
    let runs_dir = encoded_dir.join("records").join("runs");
    if runs_dir.exists() {
        Some(runs_dir)
    } else {
        None
    }
}

/// Counts runs from store list output.
///
/// Counts lines that look like run entries (start with spaces followed by 8 hex chars).
fn count_runs(output: &str) -> usize {
    // The store list output has lines like:
    //   ed48d519  2026-01-16 11:20:29      0.011s      10 KB  1 passed
    // We look for lines that start with spaces followed by 8 hex characters.
    static SHORT_ID_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s+[0-9a-f]{8}\s").unwrap());
    output
        .lines()
        .filter(|line| SHORT_ID_REGEX.is_match(line))
        .count()
}

/// Redacts dynamic fields from output for snapshot comparison.
///
/// This function redacts:
/// - Temp root paths with `[TEMP_DIR]`
/// - Timestamps with `XXXX-XX-XX XX:XX:XX` (19 chars, preserving width)
/// - Bracketed durations like `[   0.024s]` with same-width placeholders
///
/// Store list output (timestamps, durations, sizes in columns) is redacted by
/// the Redactor infrastructure when `__NEXTEST_REDACT=1` is set. This function
/// handles additional dynamic fields in replay output.
fn redact_dynamic_fields(output: &str, temp_root: &Utf8Path) -> String {
    // Redact the temp root path (this covers both workspace and cache paths).
    let temp_root_escaped = regex::escape(temp_root.as_str());
    let temp_root_regex = Regex::new(&format!(r"{temp_root_escaped}[^\s]*")).unwrap();
    let output = temp_root_regex.replace_all(output, "[TEMP_DIR]");

    // Redact timestamps with fixed-width placeholder (19 chars).
    let output = TIMESTAMP_REGEX.replace_all(&output, "XXXX-XX-XX XX:XX:XX");

    // Redact bracketed durations with same-width placeholder.
    // E.g., "[   0.024s]" (11 chars) -> "[ duration ]" (11 chars).
    let output = BRACKETED_DURATION_REGEX.replace_all(&output, |caps: &regex::Captures| {
        let matched = caps.get(0).unwrap().as_str();
        let width = matched.len();
        // Create a placeholder that fits within the brackets with same width.
        // "[" + padding + "duration" + padding + "]"
        let inner_width = width - 2; // subtract brackets
        let placeholder = "duration";
        let padding = inner_width.saturating_sub(placeholder.len());
        let left_pad = padding.div_ceil(2);
        let right_pad = padding.saturating_sub(left_pad);
        format!(
            "[{}{}{}]",
            " ".repeat(left_pad),
            placeholder,
            " ".repeat(right_pad)
        )
    });

    output.to_string()
}

// --- Tests ---

/// Full record-replay cycle.
///
/// Coverage: Basic recording, archive creation, store list/info, replay with default
/// and explicit run ID, run ID prefix resolution, fixture model verification of outputs.
#[test]
fn test_record_replay_cycle() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run ID for reproducible tests.
    const RUN_ID: &str = "10000001-0000-0000-0000-000000000001";

    // Record a run with the full test suite (matching what the fixture model expects).
    let run_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "--workspace", "--all-targets"])
        .unchecked(true)
        .output();
    assert_eq!(
        run_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "run should fail due to failing tests: {run_output}"
    );

    // Verify store list shows one run.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(
        list_output.exit_status.success(),
        "store list should succeed"
    );
    insta::assert_snapshot!(
        "store_list_single_run",
        redact_dynamic_fields(&list_output.stdout_as_str(), temp_root)
    );

    // Verify store list -v output.
    let list_verbose_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["store", "list", "-v"])
            .output();
    assert!(
        list_verbose_output.exit_status.success(),
        "store list -v should succeed"
    );
    insta::assert_snapshot!(
        "store_list_verbose_single_run",
        redact_dynamic_fields(&list_verbose_output.stdout_as_str(), temp_root)
    );

    // Replay with default (most recent) and verify against fixture model.
    // Note: Replay output goes to stdout, not stderr. Replay shows SKIP lines for
    // skipped tests, so we use ALLOW_SKIPPED_NAMES_IN_OUTPUT to permit that.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--status-level", "all"])
        .output();
    assert!(
        replay_output.exit_status.success(),
        "replay should succeed: {replay_output}"
    );
    // Verify replay output matches expected test results from fixture data.
    check_run_output(
        &replay_output.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Replay with explicit full run ID (should produce same output).
    let replay_explicit = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", RUN_ID, "--status-level", "all"])
        .output();
    assert!(
        replay_explicit.exit_status.success(),
        "replay with explicit ID should succeed: {replay_explicit}"
    );
    check_run_output(
        &replay_explicit.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Replay with run ID prefix (first 4 chars).
    let replay_prefix = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "1000", "--status-level", "all"])
        .output();
    assert!(
        replay_prefix.exit_status.success(),
        "replay with prefix should succeed: {replay_prefix}"
    );
    check_run_output(
        &replay_prefix.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Replay with explicit "latest" (should produce same output as default).
    let replay_latest = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "-r", "latest", "--status-level", "all"])
        .output();
    assert!(
        replay_latest.exit_status.success(),
        "replay with -r latest should succeed: {replay_latest}"
    );
    check_run_output(
        &replay_latest.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Verify archive structure.
    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    let run_dirs: Vec<_> = std::fs::read_dir(&runs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    assert_eq!(
        run_dirs.len(),
        1,
        "should have exactly one run directory: {:?}",
        run_dirs
    );

    let run_dir = Utf8PathBuf::try_from(run_dirs[0].path()).unwrap();
    assert!(
        run_dir.join("store.zip").exists(),
        "store.zip should exist in {run_dir}"
    );
    assert!(
        run_dir.join("run.log.zst").exists(),
        "run.log.zst should exist in {run_dir}"
    );

    // Verify archive contains all expected metadata files.
    let store_zip = run_dir.join("store.zip");
    let file = std::fs::File::open(&store_zip).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let archive_files: Vec<_> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();

    for expected in EXPECTED_ARCHIVE_FILES {
        assert!(
            archive_files.iter().any(|f| f == *expected),
            "archive should contain {expected}, found: {archive_files:?}"
        );
    }

    // Verify out/ directory exists and has content-addressed files.
    let out_files: Vec<_> = archive_files
        .iter()
        .filter(|f| f.starts_with("out/"))
        .collect();
    assert!(
        !out_files.is_empty(),
        "archive should contain output files in out/"
    );
}

/// Error handling.
///
/// Coverage: Empty store, invalid run ID, nonexistent run ID. Snapshots for error messages.
#[test]
fn test_error_handling() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run ID for reproducible tests.
    const RUN_ID: &str = "20000001-0000-0000-0000-000000000001";

    // Replay on empty store should fail with helpful error.
    let replay_empty = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay"])
        .unchecked(true)
        .output();
    assert!(
        !replay_empty.exit_status.success(),
        "replay on empty store should fail"
    );
    insta::assert_snapshot!(
        "error_replay_empty_store",
        redact_dynamic_fields(&replay_empty.stderr_as_str(), temp_root)
    );

    // Store list on empty store should succeed with empty output.
    let list_empty = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(
        list_empty.exit_status.success(),
        "store list on empty store should succeed"
    );
    insta::assert_snapshot!(
        "store_list_empty",
        redact_dynamic_fields(&list_empty.stdout_as_str(), temp_root)
    );

    // Store list -v on empty store should succeed.
    let list_verbose_empty = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list", "-v"])
        .output();
    assert!(
        list_verbose_empty.exit_status.success(),
        "store list -v on empty store should succeed"
    );
    insta::assert_snapshot!(
        "store_list_verbose_empty",
        redact_dynamic_fields(&list_verbose_empty.stdout_as_str(), temp_root)
    );

    // Create a recording for remaining error tests.
    let recording = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "-E", "test(=test_success)"])
        .output();
    assert!(
        recording.exit_status.success(),
        "recording should succeed: {recording}"
    );

    // Invalid run ID format should fail with helpful error.
    let replay_invalid = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "not-a-uuid!!!"])
        .unchecked(true)
        .output();
    assert!(
        !replay_invalid.exit_status.success(),
        "replay with invalid run ID should fail"
    );
    insta::assert_snapshot!(
        "error_replay_invalid_run_id",
        redact_dynamic_fields(&replay_invalid.stderr_as_str(), temp_root)
    );

    // Valid UUID format but nonexistent should fail with helpful error.
    let replay_nonexistent = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "00000000-0000-0000-0000-000000000000"])
        .unchecked(true)
        .output();
    assert!(
        !replay_nonexistent.exit_status.success(),
        "replay with nonexistent run ID should fail"
    );
    insta::assert_snapshot!(
        "error_replay_nonexistent_run_id",
        redact_dynamic_fields(&replay_nonexistent.stderr_as_str(), temp_root)
    );
}

/// Replay options.
///
/// Coverage: `--status-level`, `--failure-output`, `--success-output`, `--no-capture`, `--exit-code`.
/// Uses fixture model for verification with the full test suite.
#[test]
fn test_replay_options() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run ID for reproducible tests.
    const RUN_ID: &str = "30000001-0000-0000-0000-000000000001";

    // Record a run with the full test suite (matching what the fixture model expects).
    let run_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "--workspace", "--all-targets"])
        .unchecked(true)
        .output();
    assert_eq!(
        run_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "run should fail due to failing tests: {run_output}"
    );

    // Test --status-level=fail (should show only failures in status lines).
    // Note: Replay output goes to stdout. With --status-level=fail, only failures
    // appear in status lines, so we can't use the full fixture model here.
    let replay_fail_only = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--status-level", "fail"])
        .output();
    assert!(
        replay_fail_only.exit_status.success(),
        "replay itself should succeed: {replay_fail_only}"
    );

    // Test --status-level=pass (should show passes and failures).
    let replay_pass_level = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--status-level", "pass"])
        .output();
    assert!(
        replay_pass_level.exit_status.success(),
        "replay with --status-level=pass should succeed: {replay_pass_level}"
    );

    // Test --status-level=all with --failure-output=final (failures grouped at end).
    // Note: Replay output goes to stdout, not stderr. Replay shows SKIP lines for
    // skipped tests, so we use ALLOW_SKIPPED_NAMES_IN_OUTPUT.
    let replay_failure_final =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args([
                "replay",
                "--failure-output",
                "final",
                "--status-level",
                "all",
            ])
            .output();
    assert!(
        replay_failure_final.exit_status.success(),
        "replay with --failure-output=final should succeed: {replay_failure_final}"
    );
    check_run_output(
        &replay_failure_final.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Test --success-output=immediate (success output shown inline).
    let replay_success_immediate =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args([
                "replay",
                "--success-output",
                "immediate",
                "--status-level",
                "all",
            ])
            .output();
    assert!(
        replay_success_immediate.exit_status.success(),
        "replay with --success-output=immediate should succeed: {replay_success_immediate}"
    );
    check_run_output(
        &replay_success_immediate.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Test --no-capture (simulated: immediate output, no indent).
    let replay_no_capture = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--no-capture", "--status-level", "all"])
        .output();
    assert!(
        replay_no_capture.exit_status.success(),
        "replay with --no-capture should succeed: {replay_no_capture}"
    );
    check_run_output(
        &replay_no_capture.stdout,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    // Test --exit-code returns the original run's exit code.
    // Without --exit-code, replay always returns 0.
    let replay_no_exit_flag =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["replay"])
            .output();
    assert!(
        replay_no_exit_flag.exit_status.success(),
        "replay without --exit-code should succeed"
    );

    // With --exit-code, replay returns the original run's exit code (non-zero due to failures).
    let replay_with_exit_flag =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["replay", "--exit-code"])
            .unchecked(true)
            .output();
    assert_eq!(
        replay_with_exit_flag.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "--exit-code should return original run's non-zero exit: {replay_with_exit_flag}"
    );
}

/// Exit code edge cases.
///
/// Coverage: `--exit-code` with all tests passing, no tests run (both fail and pass behaviors).
#[test]
fn test_exit_code_edge_cases() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run IDs for reproducible tests.
    const RUN_ID_PASS: &str = "40000001-0000-0000-0000-000000000001";
    const RUN_ID_NO_TESTS_DEFAULT: &str = "40000002-0000-0000-0000-000000000002";
    const RUN_ID_NO_TESTS_FAIL: &str = "40000003-0000-0000-0000-000000000003";
    const RUN_ID_NO_TESTS_PASS: &str = "40000004-0000-0000-0000-000000000004";

    // Record a run where all tests pass.
    let pass_recording = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(RUN_ID_PASS),
    )
    .args(["run", "-E", "test(=test_success)"])
    .output();
    assert!(
        pass_recording.exit_status.success(),
        "all-pass recording should succeed: {pass_recording}"
    );

    // Replay with --exit-code should return 0 for an all-pass run.
    let replay_all_pass = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", RUN_ID_PASS, "--exit-code"])
        .output();
    assert!(
        replay_all_pass.exit_status.success(),
        "--exit-code should return 0 for all-pass run: {replay_all_pass}"
    );

    // Record a run with no tests matching and default --no-tests behavior (fail).
    // The default for --no-tests is "fail", so we should get exit code 4.
    let no_tests_default = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(RUN_ID_NO_TESTS_DEFAULT),
    )
    .args(["run", "-E", "test(=nonexistent_test_that_does_not_exist)"])
    .unchecked(true)
    .output();
    assert_eq!(
        no_tests_default.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "no tests with default --no-tests should return exit code 4: {no_tests_default}"
    );

    // Replay with --exit-code should return exit code 4 (NO_TESTS_RUN).
    // This tests the default behavior without explicit --no-tests flag.
    let replay_no_tests_default =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["replay", "--run-id", RUN_ID_NO_TESTS_DEFAULT, "--exit-code"])
            .unchecked(true)
            .output();
    assert_eq!(
        replay_no_tests_default.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "--exit-code should return 4 for no-tests-run with default behavior: {replay_no_tests_default}"
    );

    // Record a run with no tests matching and explicit --no-tests=fail (exit code 4).
    let no_tests_fail = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(RUN_ID_NO_TESTS_FAIL),
    )
    .args([
        "run",
        "-E",
        "test(=another_nonexistent_test)",
        "--no-tests=fail",
    ])
    .unchecked(true)
    .output();
    assert_eq!(
        no_tests_fail.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "no tests with --no-tests=fail should return exit code 4: {no_tests_fail}"
    );

    // Replay with --exit-code should return exit code 4 (NO_TESTS_RUN).
    let replay_no_tests_fail =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["replay", "--run-id", RUN_ID_NO_TESTS_FAIL, "--exit-code"])
            .unchecked(true)
            .output();
    assert_eq!(
        replay_no_tests_fail.exit_status.code(),
        Some(NextestExitCode::NO_TESTS_RUN),
        "--exit-code should return 4 for no-tests-run with explicit fail behavior: {replay_no_tests_fail}"
    );

    // Record a run with no tests matching and --no-tests=pass (exit code 0).
    let no_tests_pass = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(RUN_ID_NO_TESTS_PASS),
    )
    .args([
        "run",
        "-E",
        "test(=another_nonexistent_test)",
        "--no-tests=pass",
    ])
    .output();
    assert!(
        no_tests_pass.exit_status.success(),
        "no tests with --no-tests=pass should return 0: {no_tests_pass}"
    );

    // Replay with --exit-code should return 0 for no-tests-run with pass behavior.
    let replay_no_tests_pass =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["replay", "--run-id", RUN_ID_NO_TESTS_PASS, "--exit-code"])
            .output();
    assert!(
        replay_no_tests_pass.exit_status.success(),
        "--exit-code should return 0 for no-tests-run with pass behavior: {replay_no_tests_pass}"
    );
}

/// Store management and pruning.
///
/// Coverage: Multiple recordings, store list with multiple runs, prune dry-run, prune execution.
#[test]
fn test_store_management() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run IDs for reproducible tests.
    const RUN_IDS: [&str; 3] = [
        "50000001-0000-0000-0000-000000000001",
        "50000002-0000-0000-0000-000000000002",
        "50000003-0000-0000-0000-000000000003",
    ];

    // Create 3 recordings with slightly different test sets for variety.
    let filters = [
        "test(=test_success)",
        "test(=test_cwd)",
        "test(=test_success) | test(=test_cwd)",
    ];
    for (run_id, filter) in RUN_IDS.iter().zip(filters.iter()) {
        let output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(run_id))
            .args(["run", "-E", filter])
            .output();
        assert!(
            output.exit_status.success(),
            "recording with filter {filter} should succeed: {output}"
        );
    }

    // Verify store list shows 3 runs with snapshot.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    insta::assert_snapshot!(
        "store_list_multiple_runs",
        redact_dynamic_fields(&list_output.stdout_as_str(), temp_root)
    );
    assert_eq!(
        count_runs(&list_output.stdout_as_str()),
        3,
        "should have 3 runs"
    );

    // Store list -v with multiple runs.
    let list_verbose_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
            .args(["store", "list", "-v"])
            .output();
    assert!(list_verbose_output.exit_status.success());
    insta::assert_snapshot!(
        "store_list_verbose_multiple_runs",
        redact_dynamic_fields(&list_verbose_output.stdout_as_str(), temp_root)
    );

    // Prune dry-run should show what would be deleted but not delete.
    let prune_dry = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "prune", "--dry-run"])
        .output();
    assert!(prune_dry.exit_status.success());
    insta::assert_snapshot!(
        "store_prune_dry_run",
        redact_dynamic_fields(&prune_dry.stdout_as_str(), temp_root)
    );

    // Verify still 3 runs after dry-run.
    let list_after_dry = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert_eq!(
        count_runs(&list_after_dry.stdout_as_str()),
        3,
        "dry-run should not delete"
    );

    // Actual prune (default policy keeps all 3 since limits aren't exceeded).
    let prune_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "prune"])
        .output();
    assert!(prune_output.exit_status.success());
    insta::assert_snapshot!(
        "store_prune_nothing_to_delete",
        redact_dynamic_fields(&prune_output.stderr_as_str(), temp_root)
    );
}

/// Stress runs.
///
/// Coverage: Stress run recording, replay with iteration metadata.
#[test]
fn test_stress_runs() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run ID for reproducible tests.
    const RUN_ID: &str = "60000001-0000-0000-0000-000000000001";

    // Record a stress run with 5 iterations.
    let stress_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
            .args(["run", "--stress-count", "5", "-E", "test(=test_success)"])
            .output();
    assert!(
        stress_output.exit_status.success(),
        "stress run should succeed: {stress_output}"
    );

    // Store list should show stress run metadata.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    insta::assert_snapshot!(
        "store_list_stress_run",
        redact_dynamic_fields(&list_output.stdout_as_str(), temp_root)
    );

    // Replay stress run with snapshot.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--status-level", "all"])
        .output();
    assert!(
        replay_output.exit_status.success(),
        "replay should succeed: {replay_output}"
    );
    // Check both stdout and stderr for stress run output.
    let output = format!(
        "{}{}",
        replay_output.stdout_as_str(),
        replay_output.stderr_as_str()
    );
    insta::assert_snapshot!(
        "replay_stress_run",
        redact_dynamic_fields(&output, temp_root)
    );

    // Verify all 5 iterations appear in either stdout or stderr.
    let count = output.matches("test_success").count();
    assert!(
        count >= 5,
        "should show all 5 iterations, found {count}: {output}"
    );
}

/// Concurrent access.
///
/// Coverage: Multiple simultaneous recordings, replay during recording.
#[test]
fn test_concurrent_access() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Use deterministic run ID for the initial recording.
    // Concurrent recordings use random IDs since they run in parallel.
    const INITIAL_RUN_ID: &str = "70000001-0000-0000-0000-000000000001";

    // First create a recording to replay later.
    let initial_recording = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(INITIAL_RUN_ID),
    )
    .args(["run", "-E", "test(=test_success)"])
    .output();
    assert!(
        initial_recording.exit_status.success(),
        "initial recording should succeed: {initial_recording}"
    );

    // Start 3 concurrent recordings.
    let manifest = p.manifest_path().to_string();
    let cache_dir_str = cache_dir.to_string();
    let user_config_path_str = user_config_path.to_string();
    // Clone all paths needed for TestEnvInfo in threads.
    let cargo_nextest_dup_bin = env_info.cargo_nextest_dup_bin.clone();
    let fake_interceptor_bin = env_info.fake_interceptor_bin.clone();
    let rustc_shim_bin = env_info.rustc_shim_bin.clone();
    let passthrough_bin = env_info.passthrough_bin.clone();
    #[cfg(unix)]
    let grab_foreground_bin = env_info.grab_foreground_bin.clone();
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let m = manifest.clone();
            let c = cache_dir_str.clone();
            let u = user_config_path_str.clone();
            let cargo_nextest_dup_bin = cargo_nextest_dup_bin.clone();
            let fake_interceptor_bin = fake_interceptor_bin.clone();
            let rustc_shim_bin = rustc_shim_bin.clone();
            let passthrough_bin = passthrough_bin.clone();
            #[cfg(unix)]
            let grab_foreground_bin = grab_foreground_bin.clone();
            std::thread::spawn(move || {
                let thread_env_info = TestEnvInfo {
                    cargo_nextest_dup_bin,
                    fake_interceptor_bin,
                    rustc_shim_bin,
                    passthrough_bin,
                    #[cfg(unix)]
                    grab_foreground_bin,
                };
                CargoNextestCli::for_test(&thread_env_info)
                    .args([
                        "--manifest-path",
                        &m,
                        "--user-config-file",
                        &u,
                        "run",
                        "-E",
                        "test(=test_success)",
                    ])
                    .env(NEXTEST_CACHE_DIR_ENV, &c)
                    .output()
            })
        })
        .collect();

    // Wait for all to complete.
    for handle in handles {
        let output = handle.join().expect("thread should not panic");
        assert!(
            output.exit_status.success(),
            "concurrent recording should succeed: {output}"
        );
    }

    // Verify store is not corrupted - should have 4 runs total.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    let runs = count_runs(&list_output.stdout_as_str());
    assert_eq!(
        runs, 4,
        "should have 4 recorded runs (1 initial + 3 concurrent)"
    );

    // Replay should still work.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay"])
        .output();
    assert!(
        replay_output.exit_status.success(),
        "replay after concurrent access should work: {replay_output}"
    );
}

/// Run ID prefix resolution.
///
/// Coverage: Ambiguous prefix (multiple matches), unique prefix, no match.
#[test]
fn test_run_id_prefix_resolution() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Create recordings with UUIDs that share a common prefix.
    // These UUIDs are chosen so that:
    // - "a" matches both run 1 and run 2 (ambiguous)
    // - "aaaa0001" matches only run 1 (unique)
    // - "aaaa0002" matches only run 2 (unique)
    // - "b" matches only run 3 (unique)
    // - "c" matches nothing (not found)
    let run_ids = [
        "aaaa0001-0000-0000-0000-000000000001",
        "aaaa0002-0000-0000-0000-000000000002",
        "bbbb0001-0000-0000-0000-000000000001",
    ];

    for run_id in run_ids {
        let output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(run_id))
            .args(["run", "-E", "test(=test_success)"])
            .output();
        assert!(
            output.exit_status.success(),
            "recording with run_id {run_id} should succeed: {output}"
        );
    }

    // Verify store list shows 3 runs with the expected IDs.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    assert_eq!(
        count_runs(&list_output.stdout_as_str()),
        3,
        "should have 3 runs"
    );
    // Verify the specific short IDs appear.
    let list_str = list_output.stdout_as_str();
    assert!(list_str.contains("aaaa0001"), "should show aaaa0001");
    assert!(list_str.contains("aaaa0002"), "should show aaaa0002");
    assert!(list_str.contains("bbbb0001"), "should show bbbb0001");

    // Test: Ambiguous prefix "a" matches 2 runs.
    let replay_ambiguous = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "a"])
        .unchecked(true)
        .output();
    assert!(
        !replay_ambiguous.exit_status.success(),
        "replay with ambiguous prefix should fail"
    );
    insta::assert_snapshot!(
        "error_replay_ambiguous_prefix",
        redact_dynamic_fields(&replay_ambiguous.stderr_as_str(), temp_root)
    );

    // Test: Unique prefix "aaaa0001" matches only 1 run.
    let replay_unique = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "aaaa0001"])
        .output();
    assert!(
        replay_unique.exit_status.success(),
        "replay with unique prefix should succeed: {replay_unique}"
    );

    // Test: Prefix "b" matches only 1 run.
    let replay_b = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "b"])
        .output();
    assert!(
        replay_b.exit_status.success(),
        "replay with prefix 'b' should succeed: {replay_b}"
    );

    // Test: Prefix "c" matches nothing.
    let replay_not_found = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay", "--run-id", "c"])
        .unchecked(true)
        .output();
    assert!(
        !replay_not_found.exit_status.success(),
        "replay with non-matching prefix should fail"
    );
    insta::assert_snapshot!(
        "error_replay_prefix_not_found",
        redact_dynamic_fields(&replay_not_found.stderr_as_str(), temp_root)
    );
}
