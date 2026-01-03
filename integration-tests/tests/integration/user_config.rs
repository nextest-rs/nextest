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

/// Creates a temporary directory with a user config file.
///
/// Returns the temp dir (which must be kept alive) and a helper to set env vars.
fn create_user_config_home(config_contents: &str) -> (Utf8TempDir, UserConfigEnv) {
    let temp_dir = camino_tempfile::Builder::new()
        .prefix("nextest-user-config-")
        .tempdir()
        .expect("created temp dir for user config");

    let base_path: Utf8PathBuf = temp_dir.path().to_path_buf();

    // Config is always at <temp>/.config/nextest/config.toml.
    // UserConfigEnv::apply sets env vars so this path is discovered on all platforms.
    let config_dir = base_path.join(".config/nextest");

    fs::create_dir_all(&config_dir).expect("created nextest config directory");

    let config_path = config_dir.join("config.toml");
    fs::write(&config_path, config_contents).expect("wrote user config file");

    (temp_dir, UserConfigEnv { base_path })
}

/// Helper to set the appropriate environment variables for user config discovery.
struct UserConfigEnv {
    base_path: Utf8PathBuf,
}

impl UserConfigEnv {
    /// Apply the environment variables to a CLI command.
    fn apply(&self, cli: &mut CargoNextestCli) {
        // Set APPDATA (Windows) and HOME/XDG_CONFIG_HOME (Unix) so that
        // <base_path>/.config/nextest/config.toml is discovered on all platforms.
        cli.env("APPDATA", self.base_path.join(".config").as_str());
        cli.env("HOME", self.base_path.as_str());
        cli.env("XDG_CONFIG_HOME", self.base_path.join(".config").as_str());
        // Remove NEXTEST_SHOW_PROGRESS so user config can be tested without
        // interference from the env var set by set_env_vars().
        cli.env_remove("NEXTEST_SHOW_PROGRESS");
    }
}

/// Creates a temporary directory with no user config file.
fn create_empty_config_home() -> (Utf8TempDir, UserConfigEnv) {
    let temp_dir = camino_tempfile::Builder::new()
        .prefix("nextest-no-config-")
        .tempdir()
        .expect("created temp dir");

    let base_path: Utf8PathBuf = temp_dir.path().to_path_buf();

    (temp_dir, UserConfigEnv { base_path })
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

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
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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

    let (_temp_dir, env) = create_empty_config_home();

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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

/// Test that an invalid show-progress value produces a clear error.
#[test]
fn test_user_config_invalid_show_progress() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let config = r#"
[ui]
show-progress = "invalid-value"
"#;
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
    cli.unchecked(true);
    let output = cli.output();

    assert!(
        !output.exit_status.success(),
        "invalid show-progress value should cause an error\n{output}"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("invalid show-progress value"),
        "error message should mention invalid show-progress value\n{output}"
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
    let (_temp_home, env) = create_user_config_home(config);

    let mut cli = CargoNextestCli::for_test();
    cli.args([
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "-E",
        "test(=test_success)",
    ]);
    env.apply(&mut cli);
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
