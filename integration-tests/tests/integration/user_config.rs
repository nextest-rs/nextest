// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for user config functionality.
//!
//! These tests verify that:
//! - User config is loaded from the expected location.
//! - CLI and environment variables override user config values.
//! - Invalid config files produce clear error messages.

use super::TempProject;
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use integration_tests::{env::set_env_vars, nextest_cli::CargoNextestCli};
use std::fs;

/// Creates a temporary user config file.
///
/// Returns the temp dir (which must be kept alive) and the path to the config file.
fn create_user_config_file(config_contents: &str) -> (Utf8TempDir, Utf8PathBuf) {
    let temp_dir = camino_tempfile::Builder::new()
        .prefix("nextest-user-config-")
        .tempdir()
        .expect("created temp dir for user config");

    let config_path = temp_dir.path().join("config.toml");
    fs::write(&config_path, config_contents).expect("wrote user config file");

    (temp_dir, config_path)
}

/// Applies user config settings to a CLI command.
fn apply_user_config(cli: &mut CargoNextestCli, config_path: &Utf8PathBuf) {
    // Use --user-config-file to specify the config file directly.
    cli.args(["--user-config-file", config_path.as_str()]);
    // Remove NEXTEST_SHOW_PROGRESS so user config can be tested without
    // interference from the env var that each test sets via set_env_vars().
    cli.env_remove("NEXTEST_SHOW_PROGRESS");
}

/// Applies settings for "no user config" tests.
fn apply_no_user_config(cli: &mut CargoNextestCli) {
    // Use --user-config-file=none to skip user config loading.
    cli.args(["--user-config-file", "none"]);
    // Remove NEXTEST_SHOW_PROGRESS so user config can be tested without
    // interference from the env var that each test sets via set_env_vars().
    cli.env_remove("NEXTEST_SHOW_PROGRESS");
}

/// Test that user config values are applied.
///
/// Verifies by checking debug output for resolved values.
#[test]
fn test_user_config_values_applied() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "counter"
max-progress-running = 4
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    // Enable debug logging to see resolved values.
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "user config should be loaded without error\n{output}"
    );

    let stderr = output.stderr_as_str();
    // Verify show_progress was set from user config.
    assert!(
        stderr.contains("ui_show_progress = Counter"),
        "show_progress should be Counter from user config\n{output}"
    );
    // Verify max_progress_running was set from user config.
    assert!(
        stderr.contains("max_progress_running = Count(4)"),
        "max_progress_running should be Count(4) from user config\n{output}"
    );
}

/// Test that CLI options override user config values.
#[test]
fn test_user_config_cli_override() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "none"
max-progress-running = 4
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
        "--show-progress",
        "bar",
        "--max-progress-running",
        "12",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "CLI should override user config\n{output}"
    );

    let stderr = output.stderr_as_str();
    // CLI values should override user config.
    assert!(
        stderr.contains("ui_show_progress = Bar"),
        "ui_show_progress should be Bar from CLI (bar)\n{output}"
    );
    assert!(
        stderr.contains("max_progress_running = Count(12)"),
        "max_progress_running should be Count(12) from CLI\n{output}"
    );
}

/// Test that environment variables override user config values.
#[test]
fn test_user_config_env_override() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "none"
max-progress-running = 4
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.env("NEXTEST_SHOW_PROGRESS", "counter");
    cli.env("NEXTEST_MAX_PROGRESS_RUNNING", "20");
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "env var should override user config\n{output}"
    );

    let stderr = output.stderr_as_str();
    // Environment variable values should override user config.
    assert!(
        stderr.contains("ui_show_progress = Counter"),
        "show_progress should be Counter from env var\n{output}"
    );
    assert!(
        stderr.contains("max_progress_running = Count(20)"),
        "max_progress_running should be Count(20) from env var\n{output}"
    );
}

/// Test that a missing user config file uses the defaults.
#[test]
fn test_user_config_missing_uses_defaults() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_no_user_config(&mut cli);
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "missing user config should use defaults\n{output}"
    );

    let stderr = output.stderr_as_str();
    // Should use default values.
    assert!(
        stderr.contains("ui_show_progress = Auto"),
        "show_progress should be Auto (default)\n{output}"
    );
    assert!(
        stderr.contains("max_progress_running = Count(8)"),
        "max_progress_running should be Count(8) (default)\n{output}"
    );
}

/// Test that malformed TOML in user config produces a clear error.
#[test]
fn test_user_config_malformed_toml() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui
show-progress = "bar"
"#; // Missing closing bracket.
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.unchecked(true);
    let output = cli.output();

    assert!(
        !output.exit_status.success(),
        "malformed TOML should cause an error\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("failed to parse user config"),
        "error message should mention user config parse failure\n{output}"
    );
}

/// Test that specifying a non-existent config file produces a clear error.
///
/// When `--user-config-file` points to a path that doesn't exist, nextest
/// should error rather than silently falling back to defaults.
#[test]
fn test_user_config_explicit_path_not_found() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "--user-config-file",
        "/nonexistent/path/to/config.toml",
        "list",
    ])
    .unchecked(true);
    let output = cli.output();

    assert!(
        !output.exit_status.success(),
        "non-existent config file should cause an error\n{output}"
    );

    // Extract just the error line for snapshot testing, since stderr includes
    // build output.
    let stderr = output.stderr_as_str();
    let error_line = stderr
        .lines()
        .find(|line| line.starts_with("error:"))
        .expect("should have an error line");
    insta::assert_snapshot!(error_line);
}

/// Test that an invalid show-progress value produces a clear error.
#[test]
fn test_user_config_invalid_show_progress() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "invalid-value"
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.unchecked(true);
    let output = cli.output();

    assert!(
        !output.exit_status.success(),
        "invalid show-progress value should cause an error\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("unknown variant") && stderr.contains("invalid-value"),
        "error message should mention unknown variant\n{output}"
    );
}

/// Test that invalid max-progress-running value produces a clear error.
#[test]
fn test_user_config_invalid_max_progress_running() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
max-progress-running = "not-a-number"
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.unchecked(true);
    let output = cli.output();

    assert!(
        !output.exit_status.success(),
        "invalid max-progress-running value should cause an error\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("invalid value: string \"not-a-number\""),
        "error message should mention invalid max-progress-running value\n{output}"
    );
}

/// Test that unknown sections in user config are allowed (forward compatibility).
#[test]
fn test_user_config_unknown_section() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "bar"

[future-section]
some-key = "some-value"
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "unknown section should be silently ignored for forward compatibility\n{output}"
    );

    let stderr = output.stderr_as_str();
    // Should still apply the known settings.
    assert!(
        stderr.contains("ui_show_progress = Bar"),
        "ui_show_progress should be Bar despite unknown section\n{output}"
    );
}

/// Test that max-progress-running = "infinite" is correctly applied.
#[test]
fn test_user_config_max_progress_running_infinite() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
max-progress-running = "infinite"
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "max-progress-running = 'infinite' should be accepted\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("max_progress_running = Infinite"),
        "max_progress_running should be Infinite\n{output}"
    );
}

/// Test that max-progress-running with integer is correctly applied.
#[test]
fn test_user_config_max_progress_running_integer() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
max-progress-running = 16
"#;
    let (_temp_dir, config_path) = create_user_config_file(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    apply_user_config(&mut cli, &config_path);
    cli.env("NEXTEST_LOG", "cargo_nextest=debug");
    let output = cli.output();

    assert!(
        output.exit_status.success(),
        "max-progress-running with integer should be accepted\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("max_progress_running = Count(16)"),
        "max_progress_running should be Count(16)\n{output}"
    );
}
