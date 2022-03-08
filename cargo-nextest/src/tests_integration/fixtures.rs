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
            "nextest-tests::basic",
            BuildPlatform::Target,
            vec![
                ("test_cargo_env_vars", false),
                ("test_cwd", false),
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
            "nextest-tests::bin/nextest-tests",
            BuildPlatform::Target,
            vec![("tests::bin_success", false)],
        ),
        TestInfo::new(
            "nextest-tests",
            BuildPlatform::Target,
            vec![("tests::unit_test_success", false)],
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

pub static ENABLE_EXPERIMENTAL: Lazy<()> = Lazy::new(enable_experimental);

#[track_caller]
fn enable_experimental() {
    std::env::set_var("NEXTEST_EXPERIMENTAL_REUSE_BUILD", "1");
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

    std::fs::write(p.binaries_metadata_path(), output.stdout().unwrap()).unwrap();
}

#[track_caller]
pub fn check_list_full_output(stdout: &[u8], platform: Option<BuildPlatform>) {
    let result: TestListSummary = serde_json::from_slice(stdout).unwrap();

    let host_binaries_count = 1;
    let test_suite = &*EXPECTED_LIST;
    match platform {
        Some(BuildPlatform::Host) => assert_eq!(host_binaries_count, result.rust_suites.len()),
        Some(BuildPlatform::Target) => assert_eq!(
            test_suite.len() - host_binaries_count,
            result.rust_suites.len()
        ),
        None => assert_eq!(test_suite.len(), result.rust_suites.len()),
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

        assert_eq!(test.test_cases.len(), entry.testcases.len());
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

    // Weirdly on macos test_cwd doesn't pass:
    //   left: `"/private/var/folders/24/8k48jl6d249_n_qfxwsl6xvm0000gn/T/nextest-fixture.Dy8Zva13W5UR"`
    //  right: `"/var/folders/24/8k48jl6d249_n_qfxwsl6xvm0000gn/T/nextest-fixture.Dy8Zva13W5UR"`

    #[cfg(not(target_os = "macos"))]
    let cwd_pass = !relocated;
    #[cfg(target_os = "macos")]
    let cwd_pass = false;
    #[cfg(target_os = "macos")]
    let _ = relocated;

    let expected = &[
        (true, "nextest-tests::basic test_cargo_env_vars"),
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

    #[cfg(not(target_os = "macos"))]
    let summary_reg = if relocated {
        Regex::new(r"Summary \[.*\] *16 tests run: 10 passed, 6 failed, 2 skipped").unwrap()
    } else {
        Regex::new(r"Summary \[.*\] *16 tests run: 11 passed, 5 failed, 2 skipped").unwrap()
    };
    #[cfg(target_os = "macos")]
    let summary_reg =
        Regex::new(r"Summary \[.*\] *16 tests run: 10 passed, 6 failed, 2 skipped").unwrap();
    assert!(summary_reg.is_match(&output), "summary didn't match");
}
