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
//! `NEXTEST_BIN_EXE_cargo-nextest-dup`.

use camino::Utf8PathBuf;
use nextest_metadata::{BuildPlatform, NextestExitCode};
use std::{fs::File, io::Write};

mod fixtures;
mod temp_project;

use camino_tempfile::Utf8TempDir;
use fixtures::*;
use temp_project::TempProject;

#[test]
fn test_list_default() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
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
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
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
        ])
        .output();

    check_list_full_output(&output.stdout, None);
}

#[test]
fn test_list_binaries_only() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
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
}

#[test]
fn test_target_dir() {
    set_env_vars();

    let p = TempProject::new().unwrap();

    std::env::set_current_dir(p.workspace_root())
        .expect("changed current directory to workspace root");

    let run_check = |target_dir: &str, extra_args: Vec<&str>| {
        // The test is for the target directory more than for any specific package, so pick a
        // package that builds quickly.
        let output = CargoNextestCli::new()
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
        std::env::set_var("CARGO_TARGET_DIR", "test-target-dir-2");
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
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let output = CargoNextestCli::new()
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
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let output = CargoNextestCli::new()
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
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let output = CargoNextestCli::new()
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
fn test_run() {
    set_env_vars();

    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output(&output.stderr, false);
}

#[test]
fn test_run_after_build() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output(&output.stderr, false);
}

#[test]
fn test_relocated_run() {
    set_env_vars();

    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path();
    let p = TempProject::new_custom_target_dir(custom_target_path).unwrap();

    build_tests(&p);
    save_cargo_metadata(&p);

    let mut p2 = TempProject::new().unwrap();
    let new_target_path = p2.workspace_root().join("test-subdir");

    // copy target directory over
    std::fs::create_dir_all(&new_target_path).unwrap();
    temp_project::copy_dir_all(custom_target_path, &new_target_path, false).unwrap();
    // Remove the old target path to ensure that any tests that refer to files within it
    // fail.
    std::fs::remove_dir_all(custom_target_path).unwrap();

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

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output(&output.stderr, true);
}

#[test]
fn test_run_from_archive() {
    set_env_vars();

    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path();
    let p = TempProject::new_custom_target_dir(custom_target_path).unwrap();

    let archive_file = p.temp_root().join("my-archive.tar.zst");

    // Write the archive to the archive_file above.
    _ = CargoNextestCli::new()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "archive",
            "--archive-file",
            archive_file.as_str(),
            "--workspace",
            "--target-dir",
            p.target_dir().as_str(),
            "--all-targets",
        ])
        .output();

    // Remove the old source and target directories to ensure that any tests that refer to files within
    // it fail.
    std::fs::remove_dir_all(p.workspace_root()).unwrap();
    std::fs::remove_dir_all(p.target_dir()).unwrap();

    let p2 = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
        .args([
            "run",
            "--archive-file",
            archive_file.as_str(),
            "--workspace-remap",
            p2.workspace_root().as_str(),
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_code,
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output(&output.stderr, true);
}

#[test]
fn test_show_config_test_groups() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let default_profile_output = CargoNextestCli::new()
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

    let default_profile_all_output = CargoNextestCli::new()
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

    let with_retries_output = CargoNextestCli::new()
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

    let with_retries_all_output = CargoNextestCli::new()
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

    let with_termination_output = CargoNextestCli::new()
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

    let with_termination_all_output = CargoNextestCli::new()
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
fn test_show_config_version() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    // This is the same as dispatch.rs.
    const TEST_VERSION_ENV: &str = "__NEXTEST_TEST_VERSION";

    // Required 0.9.56, recommended 0.9.54.

    let output = CargoNextestCli::new()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.56")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::REQUIRED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::new()
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
        output.exit_code,
        Some(NextestExitCode::REQUIRED_VERSION_NOT_MET)
    );
    insta::assert_snapshot!(output.stdout_as_str());

    // ---
    // With --override-version-check
    // ---
    let output = CargoNextestCli::new()
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

    let output = CargoNextestCli::new()
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

    let output = CargoNextestCli::new()
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

#[test]
fn test_setup_scripts_not_enabled() {
    set_env_vars();

    let p = TempProject::new().unwrap();

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

    let output = CargoNextestCli::new()
        .args(["run", "--manifest-path", p.manifest_path().as_str()])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_code,
        Some(NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED)
    );
}

#[test]
fn test_setup_script_error() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::new()
        .args(["run", "--manifest-path", p.manifest_path().as_str()])
        .env("__NEXTEST_SETUP_SCRIPT_ERROR", "1")
        .unchecked(true)
        .output();

    assert_eq!(output.exit_code, Some(NextestExitCode::SETUP_SCRIPT_FAILED));
}
