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

use crate::{dispatch::CargoNextestApp, OutputWriter};
use camino::Utf8Path;
use clap::StructOpt;
use nextest_metadata::BuildPlatform;

mod fixtures;
mod temp_project;

use fixtures::*;
use temp_project::TempProject;
use tempfile::TempDir;

#[test]
fn test_list_default() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
        "--message-format",
        "json",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), None);
}

#[test]
fn test_list_full() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
        "--message-format",
        "json",
        "--list-type",
        "full",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), None);
}

#[test]
fn test_list_binaries_only() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "list",
        "--workspace",
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
fn test_list_full_after_build() {
    let _ = &*ENABLE_EXPERIMENTAL;

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
    let _ = &*ENABLE_EXPERIMENTAL;

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
        "--platform-filter",
        "host",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(BuildPlatform::Host));
}

#[test]
fn test_list_target_after_build() {
    let _ = &*ENABLE_EXPERIMENTAL;

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
        "--platform-filter",
        "target",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(BuildPlatform::Target));
}

#[test]
fn test_run() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_str(),
        "run",
        "--workspace",
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed\n", err.to_string());

    check_run_output(output.stderr().unwrap(), false);
}

#[test]
fn test_run_after_build() {
    let _ = &*ENABLE_EXPERIMENTAL;

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
    assert_eq!("test run failed\n", err.to_string());

    check_run_output(output.stderr().unwrap(), false);
}

#[test]
fn test_relocated_run() {
    let _ = &*ENABLE_EXPERIMENTAL;

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
    temp_project::copy_dir_all(custom_target_path, &new_target_path, true).unwrap();
    p2.set_target_dir(new_target_path);

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
        p2.workspace_root().as_str(),
        "--target-dir-remap",
        p2.target_dir().as_str(),
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed\n", err.to_string());

    check_run_output(output.stderr().unwrap(), true);
}
