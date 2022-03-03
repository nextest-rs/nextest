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
use clap::StructOpt;
use nextest_metadata::{BinaryListSummary, Platform};

mod fixtures;
mod temp_project;

use fixtures::*;
use temp_project::TempProject;

#[test]
fn test_list_default() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
        "--message-format",
        "json",
        "--platform-filter",
        "host",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(Platform::Host));
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
        "list",
        "--binaries-metadata",
        p.binaries_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
        "--message-format",
        "json",
        "--platform-filter",
        "target",
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    check_list_full_output(output.stdout().unwrap(), Some(Platform::Target));
}

#[test]
fn test_run() {
    let p = TempProject::new().unwrap();

    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
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
        p.manifest_path().as_os_str().to_string_lossy().as_ref(),
        "run",
        "--binaries-metadata",
        p.binaries_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed\n", err.to_string());

    check_run_output(output.stderr().unwrap(), false);
}

#[test]
fn test_relocated_run() {
    let _ = &*ENABLE_EXPERIMENTAL;

    let p = TempProject::new().unwrap();
    save_cargo_metadata(&p);
    build_tests(&p);

    let p2 = TempProject::new().unwrap();
    // copy metadata files
    std::fs::copy(p.binaries_metadata_path(), p2.binaries_metadata_path()).unwrap();
    std::fs::copy(p.cargo_metadata_path(), p2.cargo_metadata_path()).unwrap();

    // copy test binaries
    let raw_binary_list = std::fs::read_to_string(p.binaries_metadata_path()).unwrap();
    let binary_list: BinaryListSummary = serde_json::from_str(&raw_binary_list).unwrap();
    let tests_dir = p2.workspace_root().join("build-artifacts");
    std::fs::create_dir(&tests_dir).unwrap();
    for bin in binary_list.rust_binaries.values() {
        std::fs::copy(
            &bin.binary_path,
            tests_dir.join(bin.binary_path.file_name().unwrap()),
        )
        .unwrap();
    }

    // Run relocated tests
    let args = CargoNextestApp::parse_from([
        "cargo",
        "nextest",
        "--manifest-path",
        p2.manifest_path().as_os_str().to_string_lossy().as_ref(),
        "run",
        "--binaries-metadata",
        p2.binaries_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
        "--cargo-metadata",
        p2.cargo_metadata_path()
            .as_os_str()
            .to_string_lossy()
            .as_ref(),
        "--workspace-remap",
        p2.workspace_root().as_os_str().to_string_lossy().as_ref(),
        "--binaries-dir-remap",
        tests_dir.as_os_str().to_string_lossy().as_ref(),
    ]);

    let mut output = OutputWriter::new_test();
    let err = args.exec(&mut output).unwrap_err();
    assert_eq!("test run failed\n", err.to_string());

    check_run_output(output.stderr().unwrap(), true);
}
