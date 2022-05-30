// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use camino::Utf8Path;
use color_eyre::Result;
use nextest_runner::{
    config::NextestConfig,
    errors::TargetRunnerError,
    runner::TestRunnerBuilder,
    signal::SignalHandler,
    target_runner::{PlatformRunner, TargetRunner},
    test_filter::{RunIgnored, TestFilterBuilder},
};
use once_cell::sync::OnceCell;
use std::{env, sync::Mutex};
use target_spec::Platform;

fn env_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceCell<Mutex<()>> = OnceCell::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn with_env(
    vars: impl IntoIterator<Item = (impl Into<String>, impl AsRef<str>)>,
    func: impl FnOnce() -> Result<TargetRunner, TargetRunnerError>,
) -> Result<TargetRunner, TargetRunnerError> {
    let lock = env_mutex().lock().unwrap();

    let keys: Vec<_> = vars
        .into_iter()
        .map(|(key, val)| {
            let key = key.into();
            env::set_var(&key, val.as_ref());
            key
        })
        .collect();

    let res = func();

    for key in keys {
        env::remove_var(key);
    }
    drop(lock);

    res
}

fn default() -> &'static target_spec::Platform {
    static DEF: OnceCell<target_spec::Platform> = OnceCell::new();
    DEF.get_or_init(|| target_spec::Platform::current().unwrap())
}

fn runner_for_target(triple: Option<&str>) -> Result<TargetRunner, TargetRunnerError> {
    TargetRunner::with_isolation(triple, &workspace_root(), &workspace_root())
}

#[test]
fn parses_cargo_env() {
    set_rustflags();

    let def_runner = with_env(
        [(
            format!(
                "CARGO_TARGET_{}_RUNNER",
                default()
                    .triple_str()
                    .to_ascii_uppercase()
                    .replace('-', "_")
            ),
            "cargo_with_default --arg --arg2",
        )],
        || runner_for_target(None),
    )
    .unwrap();

    for (_, platform_runner) in def_runner.all_build_platforms() {
        let platform_runner = platform_runner.expect("env var means runner should be defined");
        assert_eq!("cargo_with_default", platform_runner.binary());
        assert_eq!(
            vec!["--arg", "--arg2"],
            platform_runner.args().collect::<Vec<_>>()
        );
    }

    let specific_runner = with_env(
        [(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER",
            "cargo_with_specific",
        )],
        || runner_for_target(Some("aarch64-linux-android")),
    )
    .unwrap();

    let platform_runner = specific_runner.target().unwrap();
    assert_eq!("cargo_with_specific", platform_runner.binary());
    assert_eq!(0, platform_runner.args().count());
}

fn parse_triple(triple: &'static str) -> target_spec::Platform {
    target_spec::Platform::new(triple, target_spec::TargetFeatures::Unknown).unwrap()
}

#[test]
fn parses_cargo_config_exact() {
    let windows = parse_triple("x86_64-pc-windows-gnu");

    let runner = PlatformRunner::find_config(windows, &workspace_root(), Some(&workspace_root()))
        .unwrap()
        .unwrap();

    assert_eq!("wine", runner.binary());
    assert_eq!(0, runner.args().count());
}

#[test]
fn disregards_non_matching() {
    let windows = parse_triple("x86_64-unknown-linux-gnu");
    assert!(
        PlatformRunner::find_config(windows, &workspace_root(), Some(&workspace_root()))
            .unwrap()
            .is_none()
    );
}

#[test]
fn parses_cargo_config_cfg() {
    let workspace_root = workspace_root();
    let terminate_search_at = Some(workspace_root.as_path());
    let android = parse_triple("aarch64-linux-android");
    let runner = PlatformRunner::find_config(android, &workspace_root, terminate_search_at)
        .unwrap()
        .unwrap();

    assert_eq!("android-runner", runner.binary());
    assert_eq!(vec!["-x"], runner.args().collect::<Vec<_>>());

    let linux = parse_triple("x86_64-unknown-linux-musl");
    let runner = PlatformRunner::find_config(linux, &workspace_root, terminate_search_at)
        .unwrap()
        .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}

#[test]
fn falls_back_to_cargo_config() {
    let linux = parse_triple("x86_64-unknown-linux-musl");

    let target_runner = with_env(
        [(
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
            "cargo-runner-windows",
        )],
        || {
            TargetRunner::with_isolation(
                Some(linux.triple_str()),
                &workspace_root(),
                &workspace_root(),
            )
        },
    )
    .unwrap();

    let platform_runner = target_runner.target().unwrap();

    assert_eq!("passthrough", platform_runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        platform_runner.args().collect::<Vec<_>>()
    );
}

fn passthrough_path() -> &'static Utf8Path {
    Utf8Path::new(env!("CARGO_BIN_EXE_passthrough"))
}

fn current_runner_env_var() -> String {
    PlatformRunner::runner_env_var(
        &Platform::current().expect("current platform is known to target-spec"),
    )
}

#[test]
fn test_listing_with_target_runner() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());

    let bin_count = test_list.binary_count();
    let test_count = test_list.test_count();

    {
        let target_runner = with_env(
            [(
                &current_runner_env_var(),
                &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
            )],
            || runner_for_target(None),
        )?;

        let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &target_runner);

        assert_eq!(bin_count, test_list.binary_count());
        assert_eq!(test_count, test_list.test_count());
    }

    {
        // cargo unfortunately doesn't handle relative paths for runner binaries,
        // it will just assume they are in PATH if they are not absolute paths,
        // and thus makes testing it a bit annoying, so we just punt and rely
        // on the tests for parsing the runner in the proper precedence
    }

    Ok(())
}

#[test]
fn test_run_with_target_runner() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::Default);

    let target_runner = with_env(
        [(
            &current_runner_env_var(),
            &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
        )],
        || runner_for_target(None),
    )?;

    for (_, platform_runner) in target_runner.all_build_platforms() {
        let runner = platform_runner.expect("current platform runner was set through env var");
        assert_eq!(passthrough_path(), runner.binary());
    }

    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &target_runner);

    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");

    let runner = TestRunnerBuilder::default();
    let runner = runner.build(&test_list, &profile, SignalHandler::noop(), target_runner);

    let (instance_statuses, run_stats) = execute_collect(&runner);

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        for fixture in expected {
            let instance_value = instance_statuses
                .get(&(test_binary.binary_path.as_path(), fixture.name))
                .unwrap_or_else(|| {
                    panic!(
                        "no instance status found for key ({}, {})",
                        test_binary.binary_path.as_path(),
                        fixture.name
                    )
                });
            let valid = match &instance_value.status {
                InstanceStatus::Skipped(_) => fixture.status.is_ignored(),
                InstanceStatus::Finished(run_statuses) => {
                    // This test should not have been retried since retries aren't configured.
                    assert_eq!(
                        run_statuses.len(),
                        1,
                        "test {} should have been run exactly once",
                        fixture.name
                    );
                    let run_status = run_statuses.last_status();
                    run_status.result == fixture.status.to_test_status(1)
                }
            };
            if !valid {
                panic!(
                    "for test {}, mismatch in status: expected {:?}, actual {:?}",
                    fixture.name, fixture.status, instance_value.status
                );
            }
        }
    }

    assert!(!run_stats.is_success(), "run should be marked failed");

    Ok(())
}
