// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use crate::{dispatch::CargoNextestApp, OutputWriter};
use clap::StructOpt;
use nextest_metadata::{BinaryListSummary, BuildPlatform, TestListSummary};
use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;

pub struct TestInfo {
    id: &'static str,
    platform: BuildPlatform,
    test_cases: Vec<(&'static str, bool)>,
}

impl TestInfo {
    fn new(
        id: &'static str,
        platform: BuildPlatform,
        test_cases: Vec<(&'static str, bool)>,
    ) -> Self {
        Self {
            id,
            platform,
            test_cases,
        }
    }
}

pub static EXPECTED_LIST: Lazy<Vec<TestInfo>> = Lazy::new(|| {
    vec![
        TestInfo::new(
            "cdylib-example",
            BuildPlatform::Target,
            vec![("tests::test_multiply_two_cdylib", false)],
        ),
        TestInfo::new(
            "cdylib-link",
            BuildPlatform::Target,
            vec![("test_multiply_two", false)],
        ),
        TestInfo::new("dylib-test", BuildPlatform::Target, vec![]),
        TestInfo::new(
            "nextest-tests::basic",
            BuildPlatform::Target,
            vec![
                ("test_cargo_env_vars", false),
                ("test_cwd", false),
                ("test_execute_bin", false),
                ("test_failure_assert", false),
                ("test_failure_error", false),
                ("test_failure_should_panic", false),
                ("test_flaky_mod_2", false),
                ("test_flaky_mod_3", false),
                ("test_ignored", true),
                ("test_ignored_fail", true),
                ("test_success", false),
                ("test_success_should_panic", false),
            ],
        ),
        TestInfo::new(
            "nextest-derive::proc-macro/nextest-derive",
            BuildPlatform::Host,
            vec![("it_works", false)],
        ),
        TestInfo::new(
            "nextest-tests::bench/my-bench",
            BuildPlatform::Target,
            vec![("tests::test_execute_bin", false)],
        ),
        TestInfo::new(
            "nextest-tests::bin/nextest-tests",
            BuildPlatform::Target,
            vec![("tests::bin_success", false)],
        ),
        TestInfo::new(
            "nextest-tests",
            BuildPlatform::Target,
            vec![
                ("tests::call_dylib_add_two", false),
                ("tests::unit_test_success", false),
            ],
        ),
        TestInfo::new(
            "nextest-tests::other",
            BuildPlatform::Target,
            vec![("other_test_success", false)],
        ),
        TestInfo::new(
            "nextest-tests::bin/other",
            BuildPlatform::Target,
            vec![("tests::other_bin_success", false)],
        ),
        TestInfo::new(
            "nextest-tests::example/nextest-tests",
            BuildPlatform::Target,
            vec![("tests::example_success", false)],
        ),
        TestInfo::new(
            "nextest-tests::example/other",
            BuildPlatform::Target,
            vec![("tests::other_example_success", false)],
        ),
    ]
});

pub fn cargo_bin() -> String {
    match std::env::var("CARGO") {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {}", err),
    }
}

#[track_caller]
pub(super) fn set_env_vars() {
    // The dynamic library tests require this flag.
    std::env::set_var("RUSTFLAGS", "-C prefer-dynamic");
    // Set CARGO_TERM_COLOR to never to ensure that ANSI color codes don't interfere with the
    // output.
    // TODO: remove this once programmatic run statuses are supported.
    std::env::set_var("CARGO_TERM_COLOR", "never");
}

#[track_caller]
pub fn save_cargo_metadata(p: &TempProject) {
    let mut cmd = Command::new(cargo_bin());
    cmd.args([
        "metadata",
        "--format-version=1",
        "--all-features",
        "--no-deps",
        "--manifest-path",
    ]);
    cmd.arg(p.manifest_path());
    let output = cmd.output().expect("cargo metadata could run");

    assert_eq!(Some(0), output.status.code());
    std::fs::write(p.cargo_metadata_path(), &output.stdout).unwrap();
}

#[track_caller]
pub fn build_tests(p: &TempProject) {
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
        "--target-dir",
        p.target_dir().as_str(),
    ]);

    let mut output = OutputWriter::new_test();
    args.exec(&mut output).unwrap();

    std::fs::write(p.binaries_metadata_path(), output.stdout().unwrap()).unwrap();
}

#[track_caller]
pub fn check_list_full_output(stdout: &[u8], platform: Option<BuildPlatform>) {
    let result: TestListSummary = serde_json::from_slice(stdout).unwrap();

    let host_binaries_count = 1;
    let test_suite = &*EXPECTED_LIST;
    match platform {
        Some(BuildPlatform::Host) => assert_eq!(
            host_binaries_count,
            result.rust_suites.len(),
            "host suite counts match"
        ),
        Some(BuildPlatform::Target) => assert_eq!(
            test_suite.len() - host_binaries_count,
            result.rust_suites.len(),
            "target suite counts match",
        ),
        None => assert_eq!(
            test_suite.len(),
            result.rust_suites.len(),
            "test suite counts match"
        ),
    }

    for test in test_suite {
        match platform {
            Some(p) if test.platform != p => continue,
            _ => {}
        }

        let entry = result.rust_suites.get(test.id);
        let entry = match entry {
            Some(e) => e,
            _ => panic!("Missing binary: {}", test.id),
        };

        assert_eq!(
            test.test_cases.len(),
            entry.testcases.len(),
            "testcase lengths match for {}",
            test.id
        );
        for case in &test.test_cases {
            let e = entry.testcases.get(case.0);
            let e = match e {
                Some(e) => e,
                _ => panic!("Missing test case '{}' in '{}'", case.0, test.id),
            };
            assert_eq!(case.1, e.ignored);
        }
    }
}

#[track_caller]
pub fn check_list_binaries_output(stdout: &[u8]) {
    let result: BinaryListSummary = serde_json::from_slice(stdout).unwrap();

    let test_suite = &*EXPECTED_LIST;
    assert_eq!(test_suite.len(), result.rust_binaries.len());

    for test in test_suite {
        let entry = result
            .rust_binaries
            .iter()
            .find(|(_, bin)| bin.binary_id == test.id);
        let entry = match entry {
            Some(e) => e,
            _ => panic!("Missing binary: {}", test.id),
        };

        assert_eq!(test.platform, entry.1.build_platform);
    }
}

fn make_check_result_regex(result: bool, name: &str) -> Regex {
    let name = regex::escape(name);
    if result {
        Regex::new(&format!(r"PASS \[.*\] *{}", name)).unwrap()
    } else {
        Regex::new(&format!(r"FAIL \[.*\] *{}", name)).unwrap()
    }
}

#[track_caller]
pub fn check_run_output(stderr: &[u8], relocated: bool) {
    // This could be made more robust with a machine-readable output,
    // or maybe using quick-junit output

    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{}", output);

    let cwd_pass = !relocated;

    let expected = &[
        (true, "cdylib-link test_multiply_two"),
        (true, "cdylib-example tests::test_multiply_two_cdylib"),
        (true, "nextest-tests::basic test_cargo_env_vars"),
        (true, "nextest-tests::basic test_execute_bin"),
        (
            true,
            "nextest-tests::bench/my-bench tests::test_execute_bin",
        ),
        (false, "nextest-tests::basic test_failure_error"),
        (false, "nextest-tests::basic test_flaky_mod_2"),
        (true, "nextest-tests::bin/nextest-tests tests::bin_success"),
        (false, "nextest-tests::basic test_failure_should_panic"),
        (true, "nextest-tests::bin/nextest-tests tests::bin_success"),
        (false, "nextest-tests::basic test_failure_should_panic"),
        (true, "nextest-tests::bin/other tests::other_bin_success"),
        (true, "nextest-tests::basic test_success_should_panic"),
        (false, "nextest-tests::basic test_failure_assert"),
        (false, "nextest-tests::basic test_flaky_mod_3"),
        (cwd_pass, "nextest-tests::basic test_cwd"),
        (
            true,
            "nextest-tests::example/nextest-tests tests::example_success",
        ),
        (true, "nextest-tests::other other_test_success"),
        (true, "nextest-tests::basic test_success"),
        (true, "nextest-derive::proc-macro/nextest-derive it_works"),
        (
            true,
            "nextest-tests::example/other tests::other_example_success",
        ),
        (true, "nextest-tests tests::unit_test_success"),
    ];

    for (result, name) in expected {
        let reg = make_check_result_regex(*result, name);
        assert!(reg.is_match(&output), "{}: result didn't match", name);
    }

    let summary_reg = if relocated {
        Regex::new(r"Summary \[.*\] *21 tests run: 15 passed, 6 failed, 2 skipped").unwrap()
    } else {
        Regex::new(r"Summary \[.*\] *21 tests run: 16 passed, 5 failed, 2 skipped").unwrap()
    };
    assert!(
        summary_reg.is_match(&output),
        "summary didn't match (actual output: {})",
        output
    );
}
