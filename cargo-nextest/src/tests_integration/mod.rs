// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests
//!
//! Those tests are not in "cargo-nextest/tests/integration/main.rs"
//! as this would fail on CI on windows.
//! If a crate with a binary has integration tests, cargo consider that
//! it should rebuild the binary when building the tests.
//! In our case, when running `cargo run -p cargo-nextest -- rnextest run`:
//! - Execution of `cargo-nextest`
//!     - Execution of `cargo test --no-run`
//!         - Build `cargo-nextest`
//! So we try to replace the binary we are currently running. This is forbidden on Windows.

use crate::{dispatch::CargoNextestApp, ExpectedError, OutputWriter};
use camino::{Utf8Path, Utf8PathBuf};
use clap::StructOpt;
use nextest_metadata::{BuildPlatform, TestListSummary};

mod fixtures;
mod temp_project;

use fixtures::*;
use temp_project::TempProject;
use tempfile::TempDir;

#[test]
fn test_list_default() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
        "--all-targets",
        "--message-format",
        "json",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), None);
}

#[test]
fn test_list_full() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
        "--all-targets",
        "--message-format",
        "json",
        "--list-type",
        "full",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).map_err(print_error).unwrap();

    check_list_full_output(output.stdout().unwrap(), None);
}

#[test]
fn test_list_binaries_only() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
        "--all-targets",
        "--message-format",
        "json",
        "--list-type",
        "binaries-only",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_binaries_output(output.stdout().unwrap());
}

#[test]
fn test_target_dir() {
    set_env_vars();

    let p = TempProject::new().unwrap();

    std::env::set_current_dir(p.workspace_root())
        .expect("changed current directory to workspace root");

    let run_check = |target_dir: &str, extra_args: Vec<&str>| {
        let mut args = vec![
            "cargo",
            "nextest",
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
        ];
        args.extend(extra_args);
        let args = CargoNextestApp::parse_from(args);

        let mut output = OutputWriter::new_test();
        args.exec(&mut output).unwrap();

        let summary: TestListSummary = serde_json::from_slice(output.stdout().unwrap()).unwrap();
        assert_eq!(
            summary.rust_build_meta.target_directory,
            p.workspace_root().join(target_dir),
            "target directory matches"
        );
    };

    // Absolute target direcctory
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

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path().as_str(),
        "--message-format",
        "json",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), None);
}

#[test]
fn test_list_host_after_build() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path().as_str(),
        "--message-format",
        "json",
        "-E",
        "platform(host)",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(BuildPlatform::Host));
}

#[test]
fn test_list_target_after_build() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path().as_str(),
        "--message-format",
        "json",
        "-E",
        "platform(target)",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(BuildPlatform::Target));
}

#[test]
fn test_run() {
    set_env_vars();

    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "--workspace",
        "--all-targets",
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed", err.to_string());

    check_run_output(output.stderr().unwrap(), false);
}

#[test]
fn test_run_after_build() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    build_tests(&p);

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "--binaries-metadata",
        p.binaries_metadata_path().as_str(),
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed", err.to_string());

    check_run_output(output.stderr().unwrap(), false);
}

#[test]
fn test_relocated_run() {
    set_env_vars();

    let custom_target_dir = TempDir::new().unwrap();
    let custom_target_path: &Utf8Path = custom_target_dir
        .path()
        .try_into()
        .expect("tempdir is valid UTF-8");
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

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
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
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed", err.to_string());

    check_run_output(output.stderr().unwrap(), true);
}

#[test]
fn test_run_from_archive() {
    set_env_vars();

    let custom_target_dir = TempDir::new().unwrap();
    let custom_target_path: &Utf8Path = custom_target_dir
        .path()
        .try_into()
        .expect("tempdir is valid UTF-8");
    let p = TempProject::new_custom_target_dir(custom_target_path).unwrap();

    let archive_file = p.temp_root().join("my-archive.tar.zst");

    // Write the archive to the archive_file above.
    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "archive",
        "--archive-file",
        archive_file.as_str(),
        "--workspace",
        "--all-targets",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).map_err(print_error).unwrap();

    // Remove the old source and target directories to ensure that any tests that refer to files within
    // it fail.
    std::fs::remove_dir_all(p.workspace_root()).unwrap();
    std::fs::remove_dir_all(p.target_dir()).unwrap();

    let p2 = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "run",
        "--archive-file",
        archive_file.as_str(),
        "--workspace-remap",
        p2.workspace_root().as_str(),
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed", err.to_string());

    check_run_output(output.stderr().unwrap(), true);
}

// Debugging helper to print out the full chain of errors for a report.
fn print_error(error: ExpectedError) -> ExpectedError {
    error.display_to_stderr();
    error
}
