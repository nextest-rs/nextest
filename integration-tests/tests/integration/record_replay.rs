// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the record-replay feature.
//!
//! These tests verify that nextest can record test runs to disk and replay them later.

use crate::{
    fixtures::{check_rerun_expanded_output, check_rerun_output, check_run_output},
    temp_project::TempProject,
};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use fixture_data::models::RunProperties;
use integration_tests::{
    env::{TestEnvInfo, set_env_vars_for_test},
    nextest_cli::CargoNextestCli,
};
use nextest_metadata::NextestExitCode;
use regex::Regex;
use std::{fs, io::Read as _, sync::LazyLock};

/// Expected files in the store.zip archive.
const EXPECTED_ARCHIVE_FILES: &[&str] = &[
    "meta/cargo-metadata.json",
    "meta/test-list.json",
    "meta/record-opts.json",
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

/// Regex for matching Cargo build times (e.g., "target(s) in 0.01s").
static CARGO_BUILD_TIME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"target\(s\) in \d+\.\d+s").unwrap());

/// User config content that enables recording.
///
/// The `record` experimental feature must be enabled AND `[record] enabled = true`
/// must be set for recording to occur.
const RECORD_USER_CONFIG: &str = r#"
[experimental]
record = true

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
///    `[experimental] record = true` and `[record] enabled = true`)
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
/// - Cargo build times like `target(s) in 0.01s` with `target(s) in [ELAPSED]`
///
/// Store list/info output (timestamps, durations, sizes, paths) is redacted by
/// the Redactor infrastructure when `__NEXTEST_REDACT=1` is set. This function
/// handles additional dynamic fields in replay output.
fn redact_dynamic_fields(output: &str, temp_root: &Utf8Path) -> String {
    let output: String = output
        .lines()
        .filter(|line| {
            if line.contains("Blocking waiting for file lock") {
                return false;
            }
            // Cargo warnings from fixture Cargo.toml appear in non-deterministic
            // order.
            if line.contains("only one of `license` or `license-file` is necessary")
                || line.contains("no edition set: defaulting to the 2015 edition")
                || line.contains("`license` should be used if the package license")
                || line.contains("`license-file` should be used if the package uses")
                || line.contains("See https://doc.rust-lang.org/cargo/reference/manifest.html")
            {
                return false;
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n");

    let temp_root_escaped = regex::escape(temp_root.as_str());
    let temp_root_regex = Regex::new(&format!(r"{temp_root_escaped}[^\s]*")).unwrap();
    let output = temp_root_regex.replace_all(&output, "[TEMP_DIR]");

    let output = TIMESTAMP_REGEX.replace_all(&output, "XXXX-XX-XX XX:XX:XX");

    let output = BRACKETED_DURATION_REGEX.replace_all(&output, |caps: &regex::Captures| {
        let matched = caps.get(0).unwrap().as_str();
        let width = matched.len();
        let inner_width = width - 2;
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

    let output = CARGO_BUILD_TIME_REGEX.replace_all(&output, "target(s) in [ELAPSED]");

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

    // Verify store info shows detailed run info including CLI and env vars.
    let info_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "info", "latest"])
        .output();
    assert!(
        info_output.exit_status.success(),
        "store info should succeed"
    );
    insta::assert_snapshot!(
        "store_info_single_run",
        redact_dynamic_fields(&info_output.stdout_as_str(), temp_root)
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
        .args(["replay", "-R", "latest", "--status-level", "all"])
        .output();
    assert!(
        replay_latest.exit_status.success(),
        "replay with -R latest should succeed: {replay_latest}"
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

/// Replayability: missing store.zip.
///
/// Coverage: When store.zip is deleted, the run is not replayable. `store info`
/// shows the reason, `store list` does not mark it as `*latest`.
#[test]
fn test_replayability_missing_store_zip() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const RUN_ID: &str = "80000001-0000-0000-0000-000000000001";

    // Create a recording.
    let recording = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "-E", "test(=test_success)"])
        .output();
    assert!(
        recording.exit_status.success(),
        "recording should succeed: {recording}"
    );

    // Find and delete store.zip.
    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    let run_dir = runs_dir.join(RUN_ID);
    let store_zip = run_dir.join("store.zip");
    assert!(store_zip.exists(), "store.zip should exist before deletion");
    fs::remove_file(&store_zip).expect("deleted store.zip");

    // Verify store info shows the run is not replayable due to missing store.zip.
    let info_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "info", "--run-id", RUN_ID])
        .output();
    assert!(
        info_output.exit_status.success(),
        "store info should succeed: {info_output}"
    );
    let info_str = info_output.stdout_as_str();
    assert!(
        info_str.contains("store.zip is missing"),
        "store info should mention missing store.zip: {info_str}"
    );
    insta::assert_snapshot!(
        "store_info_missing_store_zip",
        redact_dynamic_fields(&info_str, temp_root)
    );

    // Verify store list does not show `*latest` marker (since the only run is
    // not replayable).
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    let list_str = list_output.stdout_as_str();
    assert!(
        !list_str.contains("*latest"),
        "store list should not show *latest for non-replayable run: {list_str}"
    );
    insta::assert_snapshot!("store_list_no_replayable_runs_store_zip", list_str);

    // Verify replay fails with appropriate error.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay"])
        .unchecked(true)
        .output();
    assert!(
        !replay_output.exit_status.success(),
        "replay should fail when no replayable runs exist"
    );
    insta::assert_snapshot!(
        "error_replay_no_replayable_runs_store_zip",
        redact_dynamic_fields(&replay_output.stderr_as_str(), temp_root)
    );
}

/// Replayability: missing run.log.zst.
///
/// Coverage: When run.log.zst is deleted, the run is not replayable.
#[test]
fn test_replayability_missing_run_log() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const RUN_ID: &str = "81000001-0000-0000-0000-000000000001";

    // Create a recording.
    let recording = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "-E", "test(=test_success)"])
        .output();
    assert!(
        recording.exit_status.success(),
        "recording should succeed: {recording}"
    );

    // Find and delete run.log.zst.
    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    let run_dir = runs_dir.join(RUN_ID);
    let run_log = run_dir.join("run.log.zst");
    assert!(run_log.exists(), "run.log.zst should exist before deletion");
    fs::remove_file(&run_log).expect("deleted run.log.zst");

    // Verify store info shows the run is not replayable due to missing run.log.zst.
    let info_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "info", "--run-id", RUN_ID])
        .output();
    assert!(
        info_output.exit_status.success(),
        "store info should succeed: {info_output}"
    );
    let info_str = info_output.stdout_as_str();
    assert!(
        info_str.contains("run.log.zst is missing"),
        "store info should mention missing run.log.zst: {info_str}"
    );
    insta::assert_snapshot!(
        "store_info_missing_run_log",
        redact_dynamic_fields(&info_str, temp_root)
    );
}

/// Replayability: `*latest` marker with mixed replayability.
///
/// Coverage: When the most recent run is not replayable, `*latest` should
/// appear on the most recent replayable run instead.
#[test]
fn test_replayability_latest_marker_skips_non_replayable() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Create two runs. Run 1 is older, run 2 is newer.
    const RUN_ID_1: &str = "82000001-0000-0000-0000-000000000001";
    const RUN_ID_2: &str = "82000002-0000-0000-0000-000000000002";

    for run_id in [RUN_ID_1, RUN_ID_2] {
        let recording =
            cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(run_id))
                .args(["run", "-E", "test(=test_success)"])
                .output();
        assert!(
            recording.exit_status.success(),
            "recording {run_id} should succeed: {recording}"
        );
    }

    // Verify store list shows run 2 as `*latest` initially.
    let list_before = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_before.exit_status.success());
    let list_str_before = list_before.stdout_as_str();
    // The `*latest` marker should appear on the same line as run 2's short ID.
    assert!(
        list_str_before.contains("82000002") && list_str_before.contains("*latest"),
        "initially run 2 should be marked as *latest: {list_str_before}"
    );

    // Delete store.zip from run 2 (the newer one).
    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    let run_2_store_zip = runs_dir.join(RUN_ID_2).join("store.zip");
    fs::remove_file(&run_2_store_zip).expect("deleted store.zip from run 2");

    // Verify store list now shows run 1 as `*latest`.
    let list_after = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_after.exit_status.success());
    let list_str_after = list_after.stdout_as_str();
    insta::assert_snapshot!(
        "store_list_latest_skips_non_replayable",
        redact_dynamic_fields(&list_str_after, temp_root)
    );

    // Check that the line with `*latest` contains run 1's ID, not run 2's.
    for line in list_str_after.lines() {
        if line.contains("*latest") {
            assert!(
                line.contains("82000001"),
                "*latest should be on run 1's line, not run 2's: {line}"
            );
            assert!(
                !line.contains("82000002"),
                "*latest should not be on run 2's line: {line}"
            );
        }
    }

    // Verify replay uses run 1 (the replayable one) by default.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay"])
        .output();
    assert!(
        replay_output.exit_status.success(),
        "replay should succeed using run 1: {replay_output}"
    );
}

/// Replayability: all runs non-replayable.
///
/// Coverage: When all runs are non-replayable, `store list` shows no `*latest`
/// marker and `replay` (without explicit run ID) fails with appropriate error.
#[test]
fn test_replayability_all_runs_non_replayable() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    // Create two runs.
    const RUN_ID_1: &str = "83000001-0000-0000-0000-000000000001";
    const RUN_ID_2: &str = "83000002-0000-0000-0000-000000000002";

    for run_id in [RUN_ID_1, RUN_ID_2] {
        let recording =
            cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(run_id))
                .args(["run", "-E", "test(=test_success)"])
                .output();
        assert!(
            recording.exit_status.success(),
            "recording {run_id} should succeed: {recording}"
        );
    }

    // Delete store.zip from both runs.
    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    for run_id in [RUN_ID_1, RUN_ID_2] {
        let store_zip = runs_dir.join(run_id).join("store.zip");
        fs::remove_file(&store_zip).expect("deleted store.zip");
    }

    // Verify store list shows no `*latest` marker.
    let list_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["store", "list"])
        .output();
    assert!(list_output.exit_status.success());
    let list_str = list_output.stdout_as_str();
    assert!(
        !list_str.contains("*latest"),
        "store list should not show *latest when all runs are non-replayable: {list_str}"
    );
    insta::assert_snapshot!(
        "store_list_all_non_replayable",
        redact_dynamic_fields(&list_str, temp_root)
    );

    // Verify replay fails with appropriate error message.
    let replay_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["replay"])
        .unchecked(true)
        .output();
    assert!(
        !replay_output.exit_status.success(),
        "replay should fail when all runs are non-replayable"
    );
    insta::assert_snapshot!(
        "error_replay_all_non_replayable",
        redact_dynamic_fields(&replay_output.stderr_as_str(), temp_root)
    );
}

// --- Rerun tests ---

/// Reads rerun-info.json from a recorded run's store.zip.
///
/// Returns None if the file doesn't exist (i.e., this is an original run, not a
/// rerun).
fn read_rerun_info(runs_dir: &Utf8Path, run_id: &str) -> Option<serde_json::Value> {
    let store_zip_path = runs_dir.join(run_id).join("store.zip");
    let file = std::fs::File::open(&store_zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut rerun_info_file = archive.by_name("meta/rerun-info.json").ok()?;
    let mut contents = String::new();
    rerun_info_file.read_to_string(&mut contents).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Basic rerun flow.
///
/// Coverage: Record run with failures, rerun only failed tests, verify passing
/// tests are skipped. Uses the fixture model to verify that only tests that
/// failed in the initial run are executed in the rerun.
#[test]
fn test_rerun_basic_flow() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const INITIAL_RUN_ID: &str = "90000001-0000-0000-0000-000000000001";
    const RERUN_ID: &str = "90000002-0000-0000-0000-000000000002";

    let initial_output = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(INITIAL_RUN_ID),
    )
    .args(["run", "--workspace", "--all-targets"])
    .unchecked(true)
    .output();
    assert_eq!(
        initial_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "initial run should fail due to failing tests: {initial_output}"
    );
    check_run_output(&initial_output.stderr, RunProperties::empty());

    let rerun_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RERUN_ID))
            .args(["run", "--rerun", INITIAL_RUN_ID])
            .unchecked(true)
            .output();
    assert_eq!(
        rerun_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun should fail because failing tests still fail: {rerun_output}"
    );
    check_rerun_output(
        &rerun_output.stderr,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");
    let rerun_info =
        read_rerun_info(&runs_dir, RERUN_ID).expect("rerun should have rerun-info.json");
    assert_eq!(
        rerun_info["parent-run-id"].as_str(),
        Some(INITIAL_RUN_ID),
        "rerun-info should reference parent run"
    );
    assert_eq!(
        rerun_info["root-info"]["run-id"].as_str(),
        Some(INITIAL_RUN_ID),
        "root-info should reference the original run"
    );
    assert!(
        read_rerun_info(&runs_dir, INITIAL_RUN_ID).is_none(),
        "initial run should not have rerun-info.json"
    );
}

/// Perform a rerun where all tests in the original run passed.
///
/// Coverage: When the original run had only passing tests and the rerun uses
/// the same filter, there are no outstanding tests to run. The rerun should
/// complete with all tests skipped (already passed).
#[test]
fn test_rerun_all_pass() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const INITIAL_RUN_ID: &str = "91000001-0000-0000-0000-000000000001";
    const RERUN_ID: &str = "91000002-0000-0000-0000-000000000002";

    let initial_output = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(INITIAL_RUN_ID),
    )
    .args(["run", "-E", "test(=test_success) | test(=test_cwd)"])
    .output();
    assert!(
        initial_output.exit_status.success(),
        "initial run should succeed: {initial_output}"
    );

    let rerun_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RERUN_ID))
            .args([
                "run",
                "--rerun",
                INITIAL_RUN_ID,
                "-E",
                "test(=test_success) | test(=test_cwd)",
            ])
            .unchecked(true)
            .output();
    assert!(
        rerun_output.exit_status.success(),
        "rerun should succeed when no outstanding tests: {rerun_output}"
    );
    insta::assert_snapshot!(
        "rerun_all_pass",
        redact_dynamic_fields(&rerun_output.stderr_as_str(), temp_root)
    );
}

/// Rerun error handling.
///
/// Coverage: Invalid run ID, nonexistent run ID.
#[test]
fn test_rerun_errors() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const RUN_ID: &str = "92000001-0000-0000-0000-000000000001";

    let recording = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID))
        .args(["run", "-E", "test(=test_success)"])
        .output();
    assert!(
        recording.exit_status.success(),
        "recording should succeed: {recording}"
    );

    let invalid_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", "not-a-uuid!!!"])
        .unchecked(true)
        .output();
    assert_eq!(
        invalid_output.exit_status.code(),
        Some(2),
        "rerun with invalid run ID should fail with clap error: {invalid_output}"
    );
    insta::assert_snapshot!(
        "rerun_error_invalid_run_id",
        redact_dynamic_fields(&invalid_output.stderr_as_str(), temp_root)
    );

    let nonexistent_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", "00000000-0000-0000-0000-000000000000"])
        .unchecked(true)
        .output();
    assert_eq!(
        nonexistent_output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "rerun with nonexistent run ID should fail with setup error: {nonexistent_output}"
    );
    insta::assert_snapshot!(
        "rerun_error_nonexistent_run_id",
        redact_dynamic_fields(&nonexistent_output.stderr_as_str(), temp_root)
    );
}

/// Rerun ID selectors.
///
/// Coverage: Full UUID, short prefix, "latest" keyword.
#[test]
fn test_rerun_run_id_selectors() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const RUN_ID_1: &str = "93000001-0000-0000-0000-000000000001";
    const RUN_ID_2: &str = "93000002-0000-0000-0000-000000000002";

    for run_id in [RUN_ID_1, RUN_ID_2] {
        let recording =
            cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(run_id))
                .args([
                    "run",
                    "-E",
                    "test(=test_success) | test(=test_failure_assert)",
                ])
                .unchecked(true)
                .output();
        assert_eq!(
            recording.exit_status.code(),
            Some(NextestExitCode::TEST_RUN_FAILED),
            "recording should fail due to test_failure_assert: {recording}"
        );
    }

    let full_uuid_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", RUN_ID_1])
        .unchecked(true)
        .output();
    assert_eq!(
        full_uuid_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun with full UUID should fail: {full_uuid_output}"
    );

    let short_prefix = &RUN_ID_2[..8];
    let prefix_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", short_prefix])
        .unchecked(true)
        .output();
    assert_eq!(
        prefix_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun with short prefix should fail: {prefix_output}"
    );

    let latest_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", "latest"])
        .unchecked(true)
        .output();
    assert_eq!(
        latest_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun with 'latest' should fail: {latest_output}"
    );

    // "930" matches both runs.
    let ambiguous_output = cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, None)
        .args(["run", "--rerun", "930"])
        .unchecked(true)
        .output();
    assert_eq!(
        ambiguous_output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "rerun with ambiguous prefix should fail with setup error: {ambiguous_output}"
    );
    insta::assert_snapshot!(
        "rerun_error_ambiguous_prefix",
        redact_dynamic_fields(&ambiguous_output.stderr_as_str(), temp_root)
    );
}

/// Rerun chain: multiple reruns building on each other.
///
/// Coverage: Chain of reruns where outstanding tests are tracked across runs.
/// Uses the fixture model to verify each rerun only runs the expected tests.
#[test]
fn test_rerun_chain() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const RUN_ID_1: &str = "94000001-0000-0000-0000-000000000001";
    const RUN_ID_2: &str = "94000002-0000-0000-0000-000000000002";
    const RUN_ID_3: &str = "94000003-0000-0000-0000-000000000003";

    let initial_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID_1))
            .args(["run", "--workspace", "--all-targets"])
            .unchecked(true)
            .output();
    assert_eq!(
        initial_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "initial run should fail: {initial_output}"
    );
    check_run_output(&initial_output.stderr, RunProperties::empty());

    let rerun1_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID_2))
            .args(["run", "--rerun", RUN_ID_1])
            .unchecked(true)
            .output();
    assert_eq!(
        rerun1_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun 1 should fail (tests still failing): {rerun1_output}"
    );
    check_rerun_output(
        &rerun1_output.stderr,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    let rerun2_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RUN_ID_3))
            .args(["run", "--rerun", RUN_ID_2])
            .unchecked(true)
            .output();
    assert_eq!(
        rerun2_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "rerun 2 should fail (tests still failing): {rerun2_output}"
    );
    check_rerun_output(
        &rerun2_output.stderr,
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );

    let runs_dir = find_runs_dir(&cache_dir).expect("runs directory should exist");

    assert!(
        read_rerun_info(&runs_dir, RUN_ID_1).is_none(),
        "initial run should not have rerun-info.json"
    );

    let rerun1_info =
        read_rerun_info(&runs_dir, RUN_ID_2).expect("rerun 1 should have rerun-info.json");
    assert_eq!(
        rerun1_info["parent-run-id"].as_str(),
        Some(RUN_ID_1),
        "rerun 1 parent should be run 1"
    );
    assert_eq!(
        rerun1_info["root-info"]["run-id"].as_str(),
        Some(RUN_ID_1),
        "rerun 1 root should be run 1"
    );

    let rerun2_info =
        read_rerun_info(&runs_dir, RUN_ID_3).expect("rerun 2 should have rerun-info.json");
    assert_eq!(
        rerun2_info["parent-run-id"].as_str(),
        Some(RUN_ID_2),
        "rerun 2 parent should be run 2"
    );
    assert_eq!(
        rerun2_info["root-info"]["run-id"].as_str(),
        Some(RUN_ID_1),
        "rerun 2 root should still be run 1 (chain root)"
    );
}

/// Rerun with new tests added.
///
/// Coverage: Tests added after the initial run are included in the rerun by
/// default.
#[test]
fn test_rerun_expanded() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const INITIAL_RUN_ID: &str = "95000001-0000-0000-0000-000000000001";
    const RERUN_ID: &str = "95000002-0000-0000-0000-000000000002";

    let initial_output = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(INITIAL_RUN_ID),
    )
    .args([
        "run",
        "-E",
        "test(=test_success) | test(=test_failure_assert)",
    ])
    .unchecked(true)
    .output();
    assert_eq!(
        initial_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "initial run should fail: {initial_output}"
    );

    let rerun_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RERUN_ID))
            .args([
                "run",
                "--rerun",
                INITIAL_RUN_ID,
                "-E",
                "test(=test_success) | test(=test_failure_assert) | test(=test_cwd)",
            ])
            .unchecked(true)
            .output();

    check_rerun_expanded_output(
        &rerun_output.stderr,
        &["test_success", "test_failure_assert"],
        &["test_cwd"],
        RunProperties::ALLOW_SKIPPED_NAMES_IN_OUTPUT,
    );
}

/// Rerun with outstanding tests not seen.
///
/// Coverage: When a rerun uses a filter that excludes some originally-failed
/// tests, those tests remain outstanding. If the tests that do run all pass,
/// the exit code is `RERUN_TESTS_OUTSTANDING`.
#[test]
fn test_rerun_tests_outstanding() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let cache_dir = create_cache_dir(&p);
    let temp_root = p.temp_root();
    let (_user_config_dir, user_config_path) = create_record_user_config();

    const INITIAL_RUN_ID: &str = "96000001-0000-0000-0000-000000000001";
    const RERUN_ID: &str = "96000002-0000-0000-0000-000000000002";

    let initial_output = cli_with_recording(
        &env_info,
        &p,
        &cache_dir,
        &user_config_path,
        Some(INITIAL_RUN_ID),
    )
    .args([
        "run",
        "-E",
        "test(=test_failure_assert) | test(=test_failure_error)",
    ])
    .unchecked(true)
    .output();
    assert_eq!(
        initial_output.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "initial run should fail: {initial_output}"
    );

    // The rerun filter excludes the originally-failed tests, including only a
    // new passing test.
    let rerun_output =
        cli_with_recording(&env_info, &p, &cache_dir, &user_config_path, Some(RERUN_ID))
            .args([
                "run",
                "--rerun",
                INITIAL_RUN_ID,
                "-E",
                "test(=test_success)",
            ])
            .unchecked(true)
            .output();
    assert_eq!(
        rerun_output.exit_status.code(),
        Some(NextestExitCode::RERUN_TESTS_OUTSTANDING),
        "rerun with outstanding tests not seen should return RERUN_TESTS_OUTSTANDING: {rerun_output}"
    );
    insta::assert_snapshot!(
        "rerun_tests_outstanding",
        redact_dynamic_fields(&rerun_output.stderr_as_str(), temp_root)
    );
}

// --- Experimental feature gating tests ---

/// Rerun requires experimental feature.
///
/// Coverage: Using -R/--rerun without the record experimental feature enabled
/// should produce an error.
#[test]
fn test_rerun_requires_experimental_feature() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let temp_root = p.temp_root();

    // Run with --rerun without enabling the experimental feature.
    // Note: We don't pass --user-config-file, so the default config is used
    // which does not have the experimental feature enabled.
    let output = cli_for_project(&env_info, &p)
        .args(["run", "--rerun", "latest"])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED),
        "--rerun without experimental feature should fail: {output}"
    );

    // Verify the error message mentions the experimental feature.
    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("experimental feature"),
        "error should mention experimental feature: {stderr}"
    );
    assert!(
        stderr.contains("NEXTEST_EXPERIMENTAL_RECORD"),
        "error should mention the environment variable: {stderr}"
    );
    insta::assert_snapshot!(
        "error_rerun_experimental_not_enabled",
        redact_dynamic_fields(&stderr, temp_root)
    );
}

/// Replay requires experimental feature.
///
/// Coverage: Using `cargo nextest replay` without the record experimental
/// feature enabled should produce an error.
#[test]
fn test_replay_requires_experimental_feature() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();
    let temp_root = p.temp_root();

    // Run replay without enabling the experimental feature.
    // Note: We don't pass --user-config-file, so the default config is used
    // which does not have the experimental feature enabled.
    let output = cli_for_project(&env_info, &p)
        .args(["replay"])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED),
        "replay without experimental feature should fail: {output}"
    );

    // Verify the error message mentions the experimental feature.
    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("experimental feature"),
        "error should mention experimental feature: {stderr}"
    );
    assert!(
        stderr.contains("NEXTEST_EXPERIMENTAL_RECORD"),
        "error should mention the environment variable: {stderr}"
    );
    insta::assert_snapshot!(
        "error_replay_experimental_not_enabled",
        redact_dynamic_fields(&stderr, temp_root)
    );
}
