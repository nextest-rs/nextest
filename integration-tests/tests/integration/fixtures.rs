// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use camino::Utf8PathBuf;
use color_eyre::Result;
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustBinaryId, RustTestSuiteStatusSummary, TestListSummary,
};
use once_cell::sync::Lazy;
use regex::Regex;
use std::{borrow::Cow, collections::HashMap, ffi::OsString, fmt, process::Command};

pub struct TestInfo {
    id: RustBinaryId,
    platform: BuildPlatform,
    // The bool represents whether the test is ignored.
    test_cases: Vec<(&'static str, bool)>,
}

impl TestInfo {
    fn new(
        id: &'static str,
        platform: BuildPlatform,
        test_cases: Vec<(&'static str, bool)>,
    ) -> Self {
        Self {
            id: id.into(),
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
                ("test_flaky_mod_4", false),
                ("test_flaky_mod_6", false),
                ("test_ignored", true),
                ("test_ignored_fail", true),
                ("test_result_failure", false),
                ("test_slow_timeout", true),
                ("test_slow_timeout_2", true),
                ("test_slow_timeout_subprocess", true),
                ("test_stdin_closed", false),
                ("test_subprocess_doesnt_exit", false),
                ("test_success", false),
                ("test_success_should_panic", false),
            ],
        ),
        TestInfo::new(
            "nextest-derive",
            BuildPlatform::Host,
            vec![("it_works", false)],
        ),
        TestInfo::new(
            "nextest-tests::bench/my-bench",
            BuildPlatform::Target,
            vec![("bench_add_two", false), ("tests::test_execute_bin", false)],
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
            "nextest-tests::segfault",
            BuildPlatform::Target,
            vec![("test_segfault", false)],
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
        TestInfo::new(
            "with-build-script",
            BuildPlatform::Target,
            vec![("tests::test_out_dir_present", false)],
        ),
    ]
});

pub fn cargo_bin() -> String {
    match std::env::var("CARGO") {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {err}"),
    }
}

#[derive(Clone, Debug)]
pub struct CargoNextestCli {
    bin: Utf8PathBuf,
    args: Vec<String>,
    envs: HashMap<OsString, OsString>,
    unchecked: bool,
}

impl CargoNextestCli {
    pub fn new() -> Self {
        let bin = std::env::var("NEXTEST_BIN_EXE_cargo-nextest-dup")
            .expect("unable to find cargo-nextest-dup");
        Self {
            bin: bin.into(),
            args: Vec::new(),
            envs: HashMap::new(),
            unchecked: false,
        }
    }

    #[allow(dead_code)]
    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(&mut self, arg: impl IntoIterator<Item = impl Into<String>>) -> &mut Self {
        self.args.extend(arg.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> &mut Self {
        self.envs.insert(k.into(), v.into());
        self
    }

    #[allow(dead_code)]
    pub fn envs(
        &mut self,
        envs: impl IntoIterator<Item = (impl Into<OsString>, impl Into<OsString>)>,
    ) -> &mut Self {
        self.envs
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn unchecked(&mut self, unchecked: bool) -> &mut Self {
        self.unchecked = unchecked;
        self
    }

    pub fn output(&self) -> CargoNextestOutput {
        let mut command = std::process::Command::new(&self.bin);
        command.arg("nextest").args(&self.args);
        command.envs(&self.envs);
        let output = command.output().expect("failed to execute");

        let ret = CargoNextestOutput {
            command,
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        };

        if !self.unchecked && !output.status.success() {
            panic!("command failed:\n\n{ret}");
        }

        ret
    }
}

#[derive(Debug)]
pub struct CargoNextestOutput {
    pub command: Command,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CargoNextestOutput {
    pub fn stdout_as_str(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    pub fn stderr_as_str(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    pub fn decode_test_list_json(&self) -> Result<TestListSummary> {
        Ok(serde_json::from_slice(&self.stdout)?)
    }
}

impl fmt::Display for CargoNextestOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "command: {:?}\nexit code: {:?}\n\
                   --- stdout ---\n{}\n\n--- stderr ---\n{}\n\n",
            self.command,
            self.exit_code,
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr)
        )
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
    // This environment variable is required to test the #[bench] fixture. Note that THIS IS FOR
    // TEST CODE ONLY. NEVER USE THIS IN PRODUCTION.
    std::env::set_var("RUSTC_BOOTSTRAP", "1");

    // Disable the tests which check for environment variables being set in `config.toml`, as they
    // won't be in the search path when running integration tests.
    std::env::set_var("__NEXTEST_NO_CHECK_CARGO_ENV_VARS", "1");

    // Remove OUT_DIR from the environment, as it interferes with tests (some of them expect that
    // OUT_DIR isn't set.)
    std::env::remove_var("OUT_DIR");
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
            "--target-dir",
            p.target_dir().as_str(),
        ])
        .output();

    std::fs::write(p.binaries_metadata_path(), output.stdout).unwrap();
}

pub fn check_list_full_output(stdout: &[u8], platform: Option<BuildPlatform>) {
    let result: TestListSummary = serde_json::from_slice(stdout).unwrap();

    let test_suite = &*EXPECTED_LIST;
    assert_eq!(
        test_suite.len(),
        result.rust_suites.len(),
        "test suite counts match"
    );

    for test in test_suite {
        match platform {
            Some(p) if test.platform != p => continue,
            _ => {}
        }

        let entry = result.rust_suites.get(&test.id);
        let entry = match entry {
            Some(e) => e,
            _ => panic!("Missing binary: {}", test.id),
        };

        if let Some(platform) = platform {
            if entry.binary.build_platform != platform {
                // The binary should be marked as skipped.
                assert_eq!(
                    entry.status,
                    RustTestSuiteStatusSummary::SKIPPED,
                    "for {}, test suite expected to be skipped because of platform mismatch",
                    test.id
                );
                assert!(
                    entry.test_cases.is_empty(),
                    "skipped test binaries should have no test cases"
                );
                continue;
            }
        }

        assert_eq!(
            entry.status,
            RustTestSuiteStatusSummary::LISTED,
            "for {}, test suite expected to be listed",
            test.id
        );
        assert_eq!(
            test.test_cases.len(),
            entry.test_cases.len(),
            "testcase lengths match for {}",
            test.id
        );
        for case in &test.test_cases {
            let e = entry.test_cases.get(case.0);
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
        Regex::new(&format!(r"PASS \[.*\] *{name}")).unwrap()
    } else {
        Regex::new(&format!(r"(FAIL|ABORT|SIGSEGV) \[.*\] *{name}")).unwrap()
    }
}

#[track_caller]
pub fn check_run_output(stderr: &[u8], relocated: bool) {
    // This could be made more robust with a machine-readable output,
    // or maybe using quick-junit output

    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{output}");

    let cwd_pass = !relocated;

    let expected = &[
        (true, "cdylib-link test_multiply_two"),
        (true, "cdylib-example tests::test_multiply_two_cdylib"),
        (true, "nextest-tests::basic test_cargo_env_vars"),
        (true, "nextest-tests::basic test_execute_bin"),
        (true, "nextest-tests::bench/my-bench bench_add_two"),
        (
            true,
            "nextest-tests::bench/my-bench tests::test_execute_bin",
        ),
        (false, "nextest-tests::basic test_failure_error"),
        (false, "nextest-tests::basic test_flaky_mod_4"),
        (true, "nextest-tests::bin/nextest-tests tests::bin_success"),
        (false, "nextest-tests::basic test_failure_should_panic"),
        (true, "nextest-tests::bin/nextest-tests tests::bin_success"),
        (false, "nextest-tests::basic test_failure_should_panic"),
        (true, "nextest-tests::bin/other tests::other_bin_success"),
        (false, "nextest-tests::basic test_result_failure"),
        (true, "nextest-tests::basic test_success_should_panic"),
        (false, "nextest-tests::basic test_failure_assert"),
        (true, "nextest-tests::basic test_stdin_closed"),
        (false, "nextest-tests::basic test_flaky_mod_6"),
        (cwd_pass, "nextest-tests::basic test_cwd"),
        (
            true,
            "nextest-tests::example/nextest-tests tests::example_success",
        ),
        (true, "nextest-tests::other other_test_success"),
        (true, "nextest-tests::basic test_success"),
        (false, "nextest-tests::segfault test_segfault"),
        (true, "nextest-derive it_works"),
        (
            true,
            "nextest-tests::example/other tests::other_example_success",
        ),
        (true, "nextest-tests tests::unit_test_success"),
    ];

    for (result, name) in expected {
        let reg = make_check_result_regex(*result, name);
        assert!(
            reg.is_match(&output),
            "{name}: result didn't match\n\n--- output ---\n{output}\n--- end output ---"
        );
    }

    let summary_reg = if relocated {
        Regex::new(r"Summary \[.*\] *27 tests run: 19 passed \(1 leaky\), 8 failed, 5 skipped")
            .unwrap()
    } else {
        Regex::new(r"Summary \[.*\] *27 tests run: 20 passed \(1 leaky\), 7 failed, 5 skipped")
            .unwrap()
    };
    assert!(
        summary_reg.is_match(&output),
        "summary didn't match (actual output: {output}, relocated: {relocated})"
    );
}
