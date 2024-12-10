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

use camino::{Utf8Path, Utf8PathBuf};
use integration_tests::{
    env::set_env_vars,
    nextest_cli::{CargoNextestCli, CargoNextestOutput},
};
use nextest_metadata::{BuildPlatform, NextestExitCode, TestListSummary};
use std::{borrow::Cow, fs::File, io::Write};
use target_spec::Platform;

mod fixtures;
mod temp_project;

use crate::temp_project::{create_uds, UdsStatus};
use camino_tempfile::Utf8TempDir;
use fixtures::*;
use temp_project::TempProject;

#[test]
fn test_version_info() {
    // Note that this is slightly overdetermined: details like the length of the short commit hash
    // are not part of the format, and we have some flexibility in changing it.
    let version_regex = regex::Regex::new(
        r"^cargo-nextest (0\.9\.[0-9\-a-z\.]+) \(([a-f0-9]{9}) (\d{4}-\d{2}-\d{2})\)\n$",
    )
    .unwrap();

    set_env_vars();

    // First run nextest with -V to get a one-line version string.
    let output = CargoNextestCli::for_test().args(["-V"]).output();
    let short_stdout = output.stdout_as_str();
    let captures = version_regex
        .captures(&short_stdout)
        .unwrap_or_else(|| panic!("short version matches regex: {short_stdout}"));

    let version = captures.get(1).unwrap().as_str();
    let short_hash = captures.get(2).unwrap().as_str();
    let date = captures.get(3).unwrap().as_str();

    let output = CargoNextestCli::for_test().args(["--version"]).output();
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
    assert_eq!(release_line, format!("release: {}", version));

    // Line 3 is the commit hash.
    let commit_hash_line = lines.next().unwrap();
    assert!(
        commit_hash_line.starts_with(&format!("commit-hash: {}", short_hash)),
        "commit hash line matches short hash: {commit_hash_line}"
    );

    // Line 4 is the commit date.
    let commit_date_line = lines.next().unwrap();
    assert_eq!(commit_date_line, format!("commit-date: {}", date));

    // Line 5 is the host. Just check that it begins with "host: ".
    let host_line = lines.next().unwrap();
    assert!(
        host_line.starts_with("host: "),
        "host line starts with 'host: ': {host_line}"
    );
}

#[test]
fn test_list_default() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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
}

#[test]
fn test_list_binaries_only() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::for_test()
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
        let output = CargoNextestCli::for_test()
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
    save_binaries_metadata(&p);

    let output = CargoNextestCli::for_test()
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
    save_binaries_metadata(&p);

    let output = CargoNextestCli::for_test()
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
    save_binaries_metadata(&p);

    let output = CargoNextestCli::for_test()
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
    set_env_vars();

    let p = TempProject::new().unwrap();
    save_binaries_metadata(&p);

    let output = CargoNextestCli::for_test()
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
        stderr.contains("Starting 0 tests across 0 binaries (8 binaries skipped)"),
        "stderr contains 'Starting' message: {output}"
    );
    assert!(
        stderr.contains("error: no tests to run\n(hint: use `--no-tests` to customize)"),
        "stderr contains no tests message: {output}"
    );

    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "-E",
            "none()",
            "--no-tests=warn",
        ])
        .output();

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("warning: no tests to run"),
        "stderr contains no tests message: {output}"
    );

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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
    set_env_vars();

    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, 0);

    // --exact with nothing else should be the same as above.
    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, 0);

    // Check the output with --skip.
    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, RunProperty::WithSkipCdylibFilter as u64);

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, RunProperty::WithSkipCdylibFilter as u64);

    // Check the output with --exact.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithMultiplyTwoExactFilter as u64,
    );

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithMultiplyTwoExactFilter as u64,
    );

    // Check the output with --exact and --skip.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithSkipCdylibFilter as u64 | RunProperty::WithMultiplyTwoExactFilter as u64,
    );

    // Equivalent filterset to the above.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithSkipCdylibFilter as u64 | RunProperty::WithMultiplyTwoExactFilter as u64,
    );

    // Another equivalent.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithSkipCdylibFilter as u64 | RunProperty::WithMultiplyTwoExactFilter as u64,
    );

    // Yet another equivalent.
    let output = CargoNextestCli::for_test()
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
    check_run_output(
        &output.stderr,
        RunProperty::WithSkipCdylibFilter as u64 | RunProperty::WithMultiplyTwoExactFilter as u64,
    );
}

#[test]
fn test_run_after_build() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    save_binaries_metadata(&p);

    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, 0);
}

#[test]
fn test_relocated_run() {
    set_env_vars();

    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path().join("target");
    let p = TempProject::new_custom_target_dir(&custom_target_path).unwrap();
    save_binaries_metadata(&p);
    save_cargo_metadata(&p);

    let mut p2 = TempProject::new().unwrap();
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

    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, RunProperty::Relocated as u64);
}

#[test]
fn test_run_from_archive_with_no_includes() {
    set_env_vars();

    let (_p1, archive_file) =
        create_archive("", false, "archive_no_includes").expect("archive succeeded");
    let (_p2, extracted_target) = run_archive(&archive_file);

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
    set_env_vars();

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
        create_archive(config, true, "archive_includes").expect("archive succeeded");
    let (_p2, extracted_target) = run_archive(&archive_file);

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
    set_env_vars();

    let config = r#"
[profile.default]
archive.include = [
    { path = "missing-file", relative-to = "target", on-missing = "error" },
]"#;
    create_archive(config, false, "archive_missing_includes")
        .expect_err("archive should have failed");
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
    config_contents: &str,
    make_uds: bool,
    snapshot_name: &str,
) -> Result<(TempProject, Utf8PathBuf), CargoNextestOutput> {
    let custom_target_dir = Utf8TempDir::new().unwrap();
    let custom_target_path = custom_target_dir.path().join("target");
    let p = TempProject::new_custom_target_dir(&custom_target_path).unwrap();

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

    // Write the archive to the archive_file above.
    let output = CargoNextestCli::for_test()
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
            // Make cargo fully quiet since we're testing just nextest output below.
            "--cargo-quiet",
            "--cargo-quiet",
        ])
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

    insta::assert_snapshot!(snapshot_name, output.stderr_as_str());

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

fn run_archive(archive_file: &Utf8Path) -> (TempProject, Utf8PathBuf) {
    let p2 = TempProject::new().unwrap();
    let extract_to = p2.workspace_root().join("extract_to");
    std::fs::create_dir_all(&extract_to).unwrap();

    let output = CargoNextestCli::for_test()
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
        Some(NextestExitCode::TEST_RUN_FAILED),
        "correct exit code for command\n{output}"
    );
    check_run_output(&output.stderr, RunProperty::Relocated as u64);

    // project is included in return value to keep tempdirs alive
    (p2, extract_to.join("target"))
}

#[test]
fn test_show_config_test_groups() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let default_profile_output = CargoNextestCli::for_test()
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

    let default_profile_all_output = CargoNextestCli::for_test()
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

    let with_retries_output = CargoNextestCli::for_test()
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

    let with_retries_all_output = CargoNextestCli::for_test()
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

    let with_termination_output = CargoNextestCli::for_test()
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

    let with_termination_all_output = CargoNextestCli::for_test()
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
    set_env_vars();
    let p = TempProject::new().unwrap();

    // Show the output of the default filter (does not include tests not in default-filter).
    let default_set_output = CargoNextestCli::for_test()
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
    let all_tests_output = CargoNextestCli::for_test()
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
    let bound_all_output = CargoNextestCli::for_test()
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
    let default_tests_output = CargoNextestCli::for_test()
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
    let package_example_output = CargoNextestCli::for_test()
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
    let package_example_bound_all_output = CargoNextestCli::for_test()
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
    let with_args_output = CargoNextestCli::for_test()
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
    let with_args_bound_all_output = CargoNextestCli::for_test()
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
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::for_test()
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
    check_run_output(&output.stderr, RunProperty::WithDefaultFilter as u64);
}

#[test]
fn test_show_config_version() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    // This is the same as dispatch.rs.
    const TEST_VERSION_ENV: &str = "__NEXTEST_TEST_VERSION";

    // Required 0.9.56, recommended 0.9.54.

    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "show-config",
            "version",
        ])
        .env(TEST_VERSION_ENV, "0.9.56")
        .output();

    insta::assert_snapshot!(output.stdout_as_str());

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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
    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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

    let output = CargoNextestCli::for_test()
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
    set_env_vars();
    let p = TempProject::new().unwrap();

    let output = CargoNextestCli::for_test()
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
fn test_target_arg() {
    set_env_vars();
    let p = TempProject::new().unwrap();

    let host_platform = Platform::current().expect("should detect the host target successfully");
    let host_triple = host_platform.triple_str();
    let output = CargoNextestCli::for_test()
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
    assert_eq!(build_platforms.host.platform, host_platform.to_summary());
    assert_eq!(build_platforms.targets[0].platform.triple, host_triple);
    assert_eq!(
        build_platforms.targets[0].libdir,
        build_platforms.host.libdir
    );
}
