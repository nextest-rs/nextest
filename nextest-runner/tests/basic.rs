// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Basic tests for the test runner.

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::Result;
use duct::cmd;
use guppy::{graph::PackageGraph, MetadataCommand};
use maplit::btreemap;
use nextest_metadata::{FilterMatch, MismatchReason};
use nextest_runner::{
    config::NextestConfig,
    reporter::TestEvent,
    runner::{
        ExecutionDescription, ExecutionResult, ExecutionStatuses, RunStats, TestRunner,
        TestRunnerBuilder,
    },
    signal::SignalHandler,
    target_runner::TargetRunner,
    test_filter::{RunIgnored, TestFilterBuilder},
    test_list::{RustTestArtifact, TestList},
};
use once_cell::sync::Lazy;
use pretty_assertions::assert_eq;
use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    io::Cursor,
};

#[derive(Copy, Clone, Debug)]
struct TestFixture {
    name: &'static str,
    status: FixtureStatus,
}

impl PartialEq<(&str, FilterMatch)> for TestFixture {
    fn eq(&self, (name, filter_match): &(&str, FilterMatch)) -> bool {
        &self.name == name && self.status.is_ignored() != filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum FixtureStatus {
    Pass,
    Fail,
    Flaky { pass_attempt: usize },
    IgnoredPass,
    IgnoredFail,
}

impl FixtureStatus {
    fn to_test_status(self, total_attempts: usize) -> ExecutionResult {
        match self {
            FixtureStatus::Pass | FixtureStatus::IgnoredPass => ExecutionResult::Pass,
            FixtureStatus::Flaky { pass_attempt } => {
                if pass_attempt <= total_attempts {
                    ExecutionResult::Pass
                } else {
                    ExecutionResult::Fail
                }
            }
            FixtureStatus::Fail | FixtureStatus::IgnoredFail => ExecutionResult::Fail,
        }
    }

    fn is_ignored(self) -> bool {
        matches!(
            self,
            FixtureStatus::IgnoredPass | FixtureStatus::IgnoredFail
        )
    }
}

static EXPECTED_TESTS: Lazy<BTreeMap<&'static str, Vec<TestFixture>>> = Lazy::new(|| {
    btreemap! {
        "nextest-tests::basic" => vec![
            TestFixture { name: "test_cargo_env_vars", status: FixtureStatus::Pass },
            TestFixture { name: "test_cwd", status: FixtureStatus::Pass },
            TestFixture { name: "test_failure_assert", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_error", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_should_panic", status: FixtureStatus::Fail },
            TestFixture { name: "test_flaky_mod_2", status: FixtureStatus::Flaky { pass_attempt: 2 } },
            TestFixture { name: "test_flaky_mod_3", status: FixtureStatus::Flaky { pass_attempt: 3 } },
            TestFixture { name: "test_ignored", status: FixtureStatus::IgnoredPass },
            TestFixture { name: "test_ignored_fail", status: FixtureStatus::IgnoredFail },
            TestFixture { name: "test_success", status: FixtureStatus::Pass },
            TestFixture { name: "test_success_should_panic", status: FixtureStatus::Pass },
        ],
        "nextest-tests" => vec![
            TestFixture { name: "tests::unit_test_success", status: FixtureStatus::Pass },
        ],
    }
});

fn workspace_root() -> Utf8PathBuf {
    // one level up from the manifest dir -> into fixtures/nextest-tests
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

static PACKAGE_GRAPH: Lazy<PackageGraph> = Lazy::new(|| {
    let mut metadata_command = MetadataCommand::new();
    // Construct a package graph with --no-deps since we don't need full dependency
    // information.
    metadata_command
        .manifest_path(workspace_root().join("Cargo.toml"))
        .no_deps()
        .build_graph()
        .expect("building package graph failed")
});

static FIXTURE_TARGETS: Lazy<BTreeMap<String, RustTestArtifact<'static>>> =
    Lazy::new(init_fixture_targets);

fn init_fixture_targets() -> BTreeMap<String, RustTestArtifact<'static>> {
    // TODO: actually productionize this, probably requires moving x into this repo
    let cmd_name = match env::var("CARGO") {
        Ok(v) => v,
        Err(env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {}", err),
    };

    let graph = &*PACKAGE_GRAPH;

    let expr = cmd!(
        cmd_name,
        "test",
        "--no-run",
        "--message-format",
        "json-render-diagnostics"
    )
    .dir(workspace_root())
    .stdout_capture();

    let output = expr.run().expect("cargo test --no-run failed");
    let test_artifacts =
        RustTestArtifact::from_messages(graph, Cursor::new(output.stdout)).unwrap();

    test_artifacts
        .into_iter()
        .map(|bin| (bin.binary_id.clone(), bin))
        .inspect(|(k, _)| println!("{}", k))
        .collect()
}

#[test]
fn test_list_tests() -> Result<()> {
    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter, None)?;

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        let info = test_list
            .get(&test_binary.binary_path)
            .unwrap_or_else(|| panic!("test list not found for {}", test_binary.binary_path));
        let tests: Vec<_> = info
            .testcases
            .iter()
            .map(|(name, info)| (name.as_str(), info.filter_match))
            .collect();
        assert_eq!(expected, &tests, "test list matches");
    }

    Ok(())
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct InstanceValue<'a> {
    binary_id: &'a str,
    cwd: &'a Utf8Path,
    status: InstanceStatus,
}

#[derive(Clone)]
enum InstanceStatus {
    Skipped(MismatchReason),
    Finished(ExecutionStatuses),
}

impl fmt::Debug for InstanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            InstanceStatus::Skipped(reason) => write!(f, "skipped: {}", reason),
            InstanceStatus::Finished(run_statuses) => {
                for run_status in run_statuses.iter() {
                    write!(
                        f,
                        "({}/{}) {:?}\n---STDOUT---\n{}\n\n---STDERR---\n{}\n\n",
                        run_status.attempt,
                        run_status.total_attempts,
                        run_status.result,
                        String::from_utf8_lossy(run_status.stdout()),
                        String::from_utf8_lossy(run_status.stderr())
                    )?;
                }
                Ok(())
            }
        }
    }
}

#[test]
fn test_run() -> Result<()> {
    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter, None)?;
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");

    let runner = TestRunnerBuilder::default().build(&test_list, &profile, SignalHandler::noop());

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

#[test]
fn test_run_ignored() -> Result<()> {
    let test_filter = TestFilterBuilder::any(RunIgnored::IgnoredOnly);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter, None)?;
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");

    let runner = TestRunnerBuilder::default().build(&test_list, &profile, SignalHandler::noop());

    let (instance_statuses, run_stats) = execute_collect(&runner);

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        for fixture in expected {
            let instance_value =
                &instance_statuses[&(test_binary.binary_path.as_path(), fixture.name)];
            let valid = match &instance_value.status {
                InstanceStatus::Skipped(_) => !fixture.status.is_ignored(),
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

#[test]
fn test_retries() -> Result<()> {
    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter, None)?;
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile("with-retries")
        .expect("with-retries config is valid");

    let retries = profile.retries();
    assert_eq!(retries, 2, "retries set in with-retries profile");

    let runner = TestRunnerBuilder::default().build(&test_list, &profile, SignalHandler::noop());

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
                    let expected_len = match fixture.status {
                        FixtureStatus::Flaky { pass_attempt } => pass_attempt,
                        FixtureStatus::Pass => 1,
                        FixtureStatus::Fail => retries + 1,
                        FixtureStatus::IgnoredPass | FixtureStatus::IgnoredFail => {
                            unreachable!("ignored tests should be skipped")
                        }
                    };
                    assert_eq!(
                        run_statuses.len(),
                        expected_len,
                        "test {} should be run {} times",
                        fixture.name,
                        expected_len,
                    );

                    match run_statuses.describe() {
                        ExecutionDescription::Success { single_status } => {
                            single_status.result == ExecutionResult::Pass
                        }
                        ExecutionDescription::Flaky {
                            last_status,
                            prior_statuses,
                        } => {
                            assert_eq!(
                                prior_statuses.len(),
                                expected_len - 1,
                                "correct length for prior statuses"
                            );
                            for prior_status in prior_statuses {
                                assert_eq!(
                                    prior_status.result,
                                    ExecutionResult::Fail,
                                    "prior status {} should be fail",
                                    prior_status.attempt
                                );
                            }
                            last_status.result == ExecutionResult::Pass
                        }
                        ExecutionDescription::Failure {
                            first_status,
                            retries,
                            ..
                        } => {
                            assert_eq!(
                                retries.len(),
                                expected_len - 1,
                                "correct length for retries"
                            );
                            for retry in retries {
                                assert_eq!(
                                    retry.result,
                                    ExecutionResult::Fail,
                                    "retry {} should be fail",
                                    retry.attempt
                                );
                            }
                            first_status.result == ExecutionResult::Fail
                        }
                    }
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

fn execute_collect<'a>(
    runner: &TestRunner<'a>,
) -> (
    HashMap<(&'a Utf8Path, &'a str), InstanceValue<'a>>,
    RunStats,
) {
    let mut instance_statuses = HashMap::new();
    let run_stats = runner.execute(|event| {
        let (test_instance, status) = match event {
            TestEvent::TestSkipped {
                test_instance,
                reason,
            } => (test_instance, InstanceStatus::Skipped(reason)),
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
            } => (test_instance, InstanceStatus::Finished(run_statuses)),
            _ => return,
        };

        instance_statuses.insert(
            (test_instance.binary, test_instance.name),
            InstanceValue {
                binary_id: test_instance.bin_info.binary_id.as_str(),
                cwd: test_instance.bin_info.cwd.as_path(),
                status,
            },
        );
    });

    (instance_statuses, run_stats)
}

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
mod target_runner;
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
use target_runner::with_env;

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
fn passthrough_path() -> &'static Utf8Path {
    static PP: once_cell::sync::OnceCell<Utf8PathBuf> = once_cell::sync::OnceCell::new();
    PP.get_or_init(|| {
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fixtures/passthrough")
    })
}

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
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
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER",
                &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
            )],
            || TargetRunner::for_target(None),
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

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
#[test]
fn runs_tests() -> Result<()> {
    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();

    let target_runner = with_env(
        [(
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER",
            &format!("{} --ensure-this-arg-is-sent", passthrough_path()),
        )],
        || TargetRunner::for_target(None),
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
    runner.set_target_runner(Some(target_runner));
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
