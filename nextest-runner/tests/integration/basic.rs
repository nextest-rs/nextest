// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use color_eyre::eyre::Result;
use nextest_filtering::FilteringExpr;
use nextest_metadata::BuildPlatform;
use nextest_runner::{
    config::NextestConfig,
    list::BinaryList,
    runner::{ExecutionDescription, ExecutionResult, TestRunnerBuilder},
    signal::SignalHandler,
    target_runner::TargetRunner,
    test_filter::{RunIgnored, TestFilterBuilder},
};
use pretty_assertions::assert_eq;
use std::io::Cursor;

#[test]
fn test_list_binaries() -> Result<()> {
    set_rustflags();

    let graph = &*PACKAGE_GRAPH;
    let binary_list =
        BinaryList::from_messages(Cursor::new(&*FIXTURE_RAW_CARGO_TEST_OUTPUT), graph)?;

    for (id, name, platform_is_target) in &EXPECTED_BINARY_LIST {
        let bin = binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == *id)
            .unwrap();
        assert_eq!(*name, bin.name.as_str());
        if *platform_is_target {
            assert_eq!(BuildPlatform::Target, bin.build_platform);
        } else {
            assert_eq!(BuildPlatform::Host, bin.build_platform);
        }
    }
    Ok(())
}

#[test]
fn test_list_tests() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    let mut summary = test_list.to_summary();

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        let info = summary
            .rust_suites
            .remove(&test_binary.binary_id)
            .unwrap_or_else(|| panic!("test list not found for {}", test_binary.binary_path));
        let tests: Vec<_> = info
            .testcases
            .iter()
            .map(|(name, info)| (name.as_str(), info.filter_match))
            .collect();
        assert_eq!(expected, &tests, "test list matches");
    }

    // Are there any remaining tests?
    if !summary.rust_suites.is_empty() {
        let mut err_msg = "actual output has test suites missing in expected output:\n".to_owned();
        for missing_suite in summary.rust_suites.keys() {
            err_msg.push_str("  - ");
            err_msg.push_str(missing_suite);
            err_msg.push('\n');
        }
        panic!("{}", err_msg);
    }

    Ok(())
}

#[test]
fn test_run() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");

    let runner = TestRunnerBuilder::default().build(
        &test_list,
        &profile,
        SignalHandler::noop(),
        TargetRunner::empty(),
    );

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

#[test]
fn test_run_ignored() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::IgnoredOnly);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");

    let runner = TestRunnerBuilder::default().build(
        &test_list,
        &profile,
        SignalHandler::noop(),
        TargetRunner::empty(),
    );

    let (instance_statuses, run_stats) = execute_collect(&runner);

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
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

/// Test that filter expressions without name matches behave as expected.
#[test]
fn test_filter_expr_without_name_matches() -> Result<()> {
    set_rustflags();

    let expr = FilteringExpr::parse(
        "test(test_multiply_two) | test(=tests::call_dylib_add_two)",
        &*PACKAGE_GRAPH,
    )
    .expect("filter expression is valid");

    let test_filter = TestFilterBuilder::new(RunIgnored::Default, None, &[] as &[&str], vec![expr]);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    for test in test_list.iter_tests() {
        if test.name.contains("test_multiply_two") || test.name == "tests::call_dylib_add_two" {
            assert!(
                test.test_info.filter_match.is_match(),
                "expected test {test:?} to be a match, but it isn't"
            );
        } else {
            assert!(
                !test.test_info.filter_match.is_match(),
                "expected test {test:?} to not be a match, but it is"
            )
        }
    }

    Ok(())
}

#[test]
fn test_name_match_without_filter_expr() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::new(
        RunIgnored::Default,
        None,
        &["test_multiply_two", "tests::call_dylib_add_two"],
        vec![],
    );
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    for test in test_list.iter_tests() {
        if test.name.contains("test_multiply_two")
            || test.name.contains("tests::call_dylib_add_two")
        {
            assert!(
                test.test_info.filter_match.is_match(),
                "expected test {test:?} to be a match, but it isn't"
            );
        } else {
            assert!(
                !test.test_info.filter_match.is_match(),
                "expected test {test:?} to not be a match, but it is"
            )
        }
    }

    Ok(())
}

#[test]
fn test_retries() -> Result<()> {
    set_rustflags();

    let test_filter = TestFilterBuilder::any(RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(&test_filter, &TargetRunner::empty());
    let config =
        NextestConfig::from_sources(&workspace_root(), None).expect("loaded fixture config");
    let profile = config
        .profile("with-retries")
        .expect("with-retries config is valid");

    let retries = profile.retries();
    assert_eq!(retries, 2, "retries set in with-retries profile");

    let runner = TestRunnerBuilder::default().build(
        &test_list,
        &profile,
        SignalHandler::noop(),
        TargetRunner::empty(),
    );

    let (instance_statuses, run_stats) = execute_collect(&runner);

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
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
