// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use nextest_runner::{errors::TargetRunnerError, target_runner::TargetRunner};
use once_cell::sync::OnceCell;
use std::{env, sync::Mutex};

use crate::fixtures::workspace_root;

fn env_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceCell<Mutex<()>> = OnceCell::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn with_env(
    vars: impl IntoIterator<Item = (impl Into<String>, impl AsRef<str>)>,
    func: impl FnOnce() -> Result<Option<TargetRunner>, TargetRunnerError>,
) -> Result<Option<TargetRunner>, TargetRunnerError> {
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

#[test]
fn parses_cargo_env() {
    let workspace_root = workspace_root();
    let terminate_search_at = Some(workspace_root.as_path());
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
        || TargetRunner::for_target(None, terminate_search_at),
    )
    .unwrap()
    .unwrap();

    assert_eq!("cargo_with_default", def_runner.binary());
    assert_eq!(
        vec!["--arg", "--arg2"],
        def_runner.args().collect::<Vec<_>>()
    );

    let specific_runner = with_env(
        [(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER",
            "cargo_with_specific",
        )],
        || TargetRunner::for_target(Some("aarch64-linux-android"), terminate_search_at),
    )
    .unwrap()
    .unwrap();

    assert_eq!("cargo_with_specific", specific_runner.binary());
    assert_eq!(0, specific_runner.args().count());
}

/// Use fixtures/nextest-test as the root dir
fn root_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

fn parse_triple(triple: &'static str) -> target_spec::Platform {
    target_spec::Platform::new(triple, target_spec::TargetFeatures::Unknown).unwrap()
}

#[test]
fn parses_cargo_config_exact() {
    let windows = parse_triple("x86_64-pc-windows-gnu");

    let runner = TargetRunner::find_config(windows, root_dir(), Some(workspace_root().as_path()))
        .unwrap()
        .unwrap();

    assert_eq!("wine", runner.binary());
    assert_eq!(0, runner.args().count());
}

#[test]
fn disregards_non_matching() {
    let windows = parse_triple("x86_64-unknown-linux-gnu");
    assert!(
        TargetRunner::find_config(windows, root_dir(), Some(workspace_root().as_path()))
            .unwrap()
            .is_none()
    );
}

#[test]
fn parses_cargo_config_cfg() {
    let workspace_root = workspace_root();
    let terminate_search_at = Some(workspace_root.as_path());
    let android = parse_triple("aarch64-linux-android");
    let runner = TargetRunner::find_config(android, root_dir(), terminate_search_at)
        .unwrap()
        .unwrap();

    assert_eq!("android-runner", runner.binary());
    assert_eq!(vec!["-x"], runner.args().collect::<Vec<_>>());

    let linux = parse_triple("x86_64-unknown-linux-musl");
    let runner = TargetRunner::find_config(linux, root_dir(), terminate_search_at)
        .unwrap()
        .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}

#[test]
fn fallsback_to_cargo_config() {
    let linux = parse_triple("x86_64-unknown-linux-musl");

    let runner = with_env(
        [(
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
            "cargo-runner-windows",
        )],
        || {
            TargetRunner::with_root(
                Some(linux.triple_str()),
                root_dir(),
                Some(workspace_root().as_path()),
            )
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}

#[cfg(unix)]
mod run {
    use super::*;
    use crate::fixtures::*;
    use camino::Utf8Path;
    use color_eyre::Result;
    use nextest_runner::{
        config::NextestConfig,
        runner::TestRunnerBuilder,
        signal::SignalHandler,
        test_filter::{RunIgnored, TestFilterBuilder},
        test_list::TestList,
    };
    use target_spec::Platform;

    fn passthrough_path() -> &'static Utf8Path {
        static PP: once_cell::sync::OnceCell<Utf8PathBuf> = once_cell::sync::OnceCell::new();
        PP.get_or_init(|| {
            Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("fixtures/passthrough")
        })
    }

    fn current_runner_env_var() -> String {
        TargetRunner::runner_env_var(
            &Platform::current().expect("current platform is known to target-spec"),
        )
    }

    #[test]
    fn test_listing_with_target_runner() -> Result<()> {
        let test_filter = TestFilterBuilder::any(RunIgnored::Default);
        let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();

        let test_list = TestList::new(test_bins.clone(), &test_filter, None)?;
        let bin_count = test_list.binary_count();
        let test_count = test_list.test_count();

        {
            let target_runner = with_env(
                [(
                    &current_runner_env_var(),
                    &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
                )],
                || TargetRunner::for_target(None, Some(workspace_root().as_path())),
            )?
            .unwrap();

            let test_list = TestList::new(test_bins.clone(), &test_filter, Some(&target_runner))?;

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
        let test_filter = TestFilterBuilder::any(RunIgnored::Default);
        let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();

        let target_runner = with_env(
            [(
                &current_runner_env_var(),
                &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
            )],
            || TargetRunner::for_target(None, Some(workspace_root().as_path())),
        )?
        .unwrap();

        assert_eq!(passthrough_path(), target_runner.binary());

        let test_list = TestList::new(test_bins, &test_filter, Some(&target_runner))?;

        let config =
            NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
        let profile = config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default config is valid");

        let mut runner = TestRunnerBuilder::default();
        runner.set_target_runner(target_runner);
        let runner = runner.build(&test_list, &profile, SignalHandler::noop());

        let (instance_statuses, run_stats) = execute_collect(&runner);

        for (name, expected) in &*EXPECTED_TESTS {
            let test_binary = FIXTURE_TARGETS
                .get(*name)
                .unwrap_or_else(|| panic!("unexpected test name {}", name));
            for fixture in expected {
                let instance_value =
                    &instance_statuses[&(test_binary.binary_path.as_path(), fixture.name)];
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
}
