// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::temp_project::TempProject;
use camino::Utf8PathBuf;
use color_eyre::Result;
use fixture_data::{
    models::{TestCaseFixtureProperty, TestCaseFixtureStatus, TestSuiteFixtureProperty},
    nextest_tests::EXPECTED_TEST_SUITES,
};
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustTestSuiteStatusSummary, TestListSummary,
};
use regex::Regex;
use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    fmt,
    process::{Command, ExitStatus},
};

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
            exit_status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        };

        if !self.unchecked && !output.status.success() {
            panic!("command failed:\n\n{ret}");
        }

        ret
    }
}

pub struct CargoNextestOutput {
    pub command: Command,
    pub exit_status: ExitStatus,
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
            self.exit_status.code(),
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr)
        )
    }
}

// Make Debug output the same as Display output, so `.unwrap()` and `.expect()` are nicer.
impl fmt::Debug for CargoNextestOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
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

    let test_suites = &*EXPECTED_TEST_SUITES;
    assert_eq!(
        test_suites.len(),
        result.rust_suites.len(),
        "test suite counts match"
    );

    for test_suite in test_suites.values() {
        match platform {
            Some(p) if test_suite.build_platform != p => continue,
            _ => {}
        }

        let entry = result.rust_suites.get(&test_suite.binary_id);
        let entry = match entry {
            Some(e) => e,
            _ => panic!("Missing binary: {}", test_suite.binary_id),
        };

        if let Some(platform) = platform {
            if entry.binary.build_platform != platform {
                // The binary should be marked as skipped.
                assert_eq!(
                    entry.status,
                    RustTestSuiteStatusSummary::SKIPPED,
                    "for {}, test suite expected to be skipped because of platform mismatch",
                    test_suite.binary_id
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
            test_suite.binary_id
        );
        assert_eq!(
            test_suite.test_cases.len(),
            entry.test_cases.len(),
            "testcase lengths match for {}",
            test_suite.binary_id
        );
        for case in &test_suite.test_cases {
            let e = entry.test_cases.get(case.name);
            let e = match e {
                Some(e) => e,
                _ => panic!(
                    "Missing test case '{}' in '{}'",
                    case.name, test_suite.binary_id
                ),
            };
            assert_eq!(case.status.is_ignored(), e.ignored);
        }
    }
}

#[track_caller]
pub fn check_list_binaries_output(stdout: &[u8]) {
    let result: BinaryListSummary = serde_json::from_slice(stdout).unwrap();

    let test_suite = &*EXPECTED_TEST_SUITES;
    let mut expected_binary_ids = test_suite
        .iter()
        .map(|(binary_id, fixture)| (binary_id.clone(), fixture.build_platform))
        .collect::<Vec<_>>();
    expected_binary_ids.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut actual_binary_ids = result
        .rust_binaries
        .iter()
        .map(|(binary_id, info)| (binary_id.clone(), info.build_platform))
        .collect::<Vec<_>>();
    actual_binary_ids.sort_by(|(a, _), (b, _)| a.cmp(b));

    assert_eq!(
        expected_binary_ids, actual_binary_ids,
        "expected binaries:\n{expected_binary_ids:?}\nactual binaries\n{actual_binary_ids:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum CheckResult {
    Pass,
    Leak,
    Fail,
    Abort,
}

impl CheckResult {
    fn make_regex(self, name: &str) -> Regex {
        let name = regex::escape(name);
        match self {
            CheckResult::Pass => Regex::new(&format!(r"PASS \[.*\] *{name}")).unwrap(),
            CheckResult::Leak => Regex::new(&format!(r"LEAK \[.*\] *{name}")).unwrap(),
            CheckResult::Fail => Regex::new(&format!(r"FAIL \[.*\] *{name}")).unwrap(),
            CheckResult::Abort => Regex::new(&format!(r"(ABORT|SIGSEGV) \[.*\] *{name}")).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u64)]
pub enum RunProperty {
    Relocated = 1,
    WithDefaultFilter = 2,
}

fn debug_run_properties(properties: u64) -> String {
    let mut ret = String::new();
    if properties & RunProperty::Relocated as u64 != 0 {
        ret.push_str("relocated ");
    }
    if properties & RunProperty::WithDefaultFilter as u64 != 0 {
        ret.push_str("with-default-filter ");
    }
    ret
}

#[track_caller]
pub fn check_run_output(stderr: &[u8], properties: u64) {
    // This could be made more robust with a machine-readable output,
    // or maybe using quick-junit output

    let output = String::from_utf8(stderr.to_vec()).unwrap();

    println!("{output}");

    let mut run_count = 0;
    let mut leak_count = 0;
    let mut pass_count = 0;
    let mut fail_count = 0;
    let mut skip_count = 0;

    for (binary_id, fixture) in &*EXPECTED_TEST_SUITES {
        if fixture.has_property(TestSuiteFixtureProperty::NotInDefaultSet)
            && properties & RunProperty::WithDefaultFilter as u64 != 0
        {
            eprintln!("*** skipping {binary_id}");
            for test in &fixture.test_cases {
                let name = format!("{} {}", binary_id, test.name);
                // This binary should be skipped -- ensure that it isn't in the output. If it sh
                assert!(
                    !output.contains(&name),
                    "binary {binary_id} should not be run with default set"
                );
            }
            continue;
        }

        for test in &fixture.test_cases {
            let name = format!("{} {}", binary_id, test.name);

            if test.has_property(TestCaseFixtureProperty::NotInDefaultSet)
                && properties & RunProperty::WithDefaultFilter as u64 != 0
            {
                eprintln!("*** skipping {name}");
                assert!(
                    !output.contains(&name),
                    "test '{name}' should not be run with default set"
                );
                skip_count += 1;
                continue;
            }

            let result = match test.status {
                // This is not a complete accounting -- for example, the needs-same-cwd check should
                // also be repeated for leaky tests in principle. But it's good enough for the test
                // suite that actually exists.
                TestCaseFixtureStatus::Pass => {
                    run_count += 1;
                    if test.has_property(TestCaseFixtureProperty::NeedsSameCwd)
                        && properties & RunProperty::Relocated as u64 != 0
                    {
                        fail_count += 1;
                        CheckResult::Fail
                    } else {
                        pass_count += 1;
                        CheckResult::Pass
                    }
                }
                TestCaseFixtureStatus::Leak => {
                    run_count += 1;
                    pass_count += 1;
                    leak_count += 1;
                    CheckResult::Leak
                }
                TestCaseFixtureStatus::Fail | TestCaseFixtureStatus::Flaky { .. } => {
                    // Flaky tests are not currently retried by this test suite. (They are retried
                    // by the older suite in nextest-runner/tests/integration).
                    run_count += 1;
                    fail_count += 1;
                    CheckResult::Fail
                }
                TestCaseFixtureStatus::Segfault => {
                    run_count += 1;
                    fail_count += 1;
                    CheckResult::Abort
                }
                TestCaseFixtureStatus::IgnoredPass | TestCaseFixtureStatus::IgnoredFail => {
                    // Ignored tests are not currently run by this test suite. (They are run by the
                    // older suite in nextest-runner/tests/integration).
                    skip_count += 1;
                    continue;
                }
            };
            let name = format!("{} {}", binary_id, test.name);
            let reg = result.make_regex(&name);
            assert!(
                reg.is_match(&output),
                "{name}: result didn't match\n\n--- output ---\n{output}\n--- end output ---"
            );
        }
    }

    let summary_regex_str = format!(
        r"Summary \[.*\] *{run_count} tests run: {pass_count} passed \({leak_count} leaky\), {fail_count} failed, {skip_count} skipped"
    );
    let summary_reg = Regex::new(&summary_regex_str).unwrap();
    assert!(
        summary_reg.is_match(&output),
        "summary didn't match regex {summary_regex_str} (actual output: {output}, properties: {})",
        debug_run_properties(properties),
    );
}
