// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use color_eyre::eyre::Result;
use fixture_data::{
    models::{TestNameAndFilterMatch, TestSuiteFixture},
    nextest_tests::EXPECTED_TEST_SUITES,
};
use iddqd::IdOrdMap;
use nextest_filtering::{Filterset, FiltersetKind, ParseContext};
use nextest_runner::{
    config::{core::NextestConfig, elements::SlowTimeoutResult},
    double_spawn::DoubleSpawnInfo,
    input::InputHandlerKind,
    list::BinaryList,
    platform::BuildPlatforms,
    reporter::events::ExecutionResult,
    run_mode::NextestRunMode,
    runner::TestRunnerBuilder,
    signal::SignalHandlerKind,
    target_runner::TargetRunner,
    test_filter::{RunIgnored, TestFilterBuilder, TestFilterPatterns},
};
use std::{io::Cursor, time::Duration};

#[test]
fn test_list_binaries() -> Result<()> {
    test_init();

    let graph = &*PACKAGE_GRAPH;
    let build_platforms = BuildPlatforms::new_with_no_target()?;
    let binary_list = BinaryList::from_messages(
        Cursor::new(&*FIXTURE_RAW_CARGO_TEST_OUTPUT),
        graph,
        build_platforms,
    )?;

    for TestSuiteFixture {
        binary_id,
        binary_name,
        build_platform,
        ..
    } in EXPECTED_TEST_SUITES.iter()
    {
        let bin = binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id == *binary_id)
            .unwrap();
        // With Rust 1.79 and later, the actual name has - replaced with _. Just check for either.
        assert!(
            bin.name.as_str() == *binary_name || bin.name.as_str() == binary_name.replace('-', "_"),
            "binary name matches (expected: {binary_name}, actual: {})",
            bin.name,
        );
        assert_eq!(*build_platform, bin.build_platform);
    }
    Ok(())
}

#[test]
fn test_list_tests() -> Result<()> {
    test_init();

    let test_filter = TestFilterBuilder::default_set(NextestRunMode::Test, RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(
        NextestConfig::DEFAULT_PROFILE,
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let mut summary = test_list.to_summary();

    for expected in &*EXPECTED_TEST_SUITES {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
            .get(&expected.binary_id)
            .unwrap_or_else(|| panic!("unexpected binary ID {}", expected.binary_id));
        let info = summary
            .rust_suites
            .remove(&test_binary.binary_id)
            .unwrap_or_else(|| panic!("test list not found for {}", test_binary.binary_path));
        let tests: IdOrdMap<_> = info
            .test_cases
            .iter()
            .map(|(name, info)| TestNameAndFilterMatch {
                name: name.as_str(),
                filter_match: info.filter_match,
            })
            .collect();
        expected.assert_test_cases_match(&tests);
    }

    // Are there any remaining tests?
    if !summary.rust_suites.is_empty() {
        let mut err_msg = "actual output has test suites missing in expected output:\n".to_owned();
        for missing_suite in summary.rust_suites.keys() {
            err_msg.push_str("  - ");
            err_msg.push_str(missing_suite.as_str());
            err_msg.push('\n');
        }
        panic!("{}", err_msg);
    }

    Ok(())
}

#[test]
fn test_termination() -> Result<()> {
    test_init();

    let pcx = ParseContext::new(&PACKAGE_GRAPH);
    let expr = Filterset::parse(
        "test(/^test_slow_timeout/)".to_owned(),
        &pcx,
        FiltersetKind::Test,
    )
    .unwrap();
    let test_filter = TestFilterBuilder::new(
        NextestRunMode::Test,
        RunIgnored::Only,
        None,
        TestFilterPatterns::default(),
        vec![expr],
    )
    .unwrap();

    let test_list =
        FIXTURE_TARGETS.make_test_list("with-termination", &test_filter, &TargetRunner::empty())?;
    let config = load_config();
    let profile = config
        .profile("with-termination")
        .expect("with-termination config is valid");
    let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
    let profile = profile.apply_build_platforms(&build_platforms);

    let runner = TestRunnerBuilder::default()
        .build(
            &test_list,
            &profile,
            vec![],
            SignalHandlerKind::Noop,
            InputHandlerKind::Noop,
            DoubleSpawnInfo::disabled(),
            TargetRunner::empty(),
        )
        .unwrap();

    let (instance_statuses, run_stats) = execute_collect(runner);
    assert_eq!(run_stats.failed_timed_out, 3, "3 tests timed out");
    for test_name in [
        "test_slow_timeout",
        "test_slow_timeout_2",
        "test_slow_timeout_subprocess",
    ] {
        let (_, instance_value) = instance_statuses
            .iter()
            .find(|&(&(_, name), _)| name == test_name)
            .unwrap_or_else(|| panic!("{test_name} should be present"));
        let valid = match &instance_value.status {
            InstanceStatus::Skipped(_) => panic!("{test_name} should have been run"),
            InstanceStatus::Finished(run_statuses) => {
                // This test should not have been retried since retries aren't configured.
                assert_eq!(
                    run_statuses.len(),
                    1,
                    "{test_name} should have been run exactly once",
                );
                let run_status = run_statuses.last_status();
                // The test should have taken less than 5 seconds (most relevant for
                // test_slow_timeout_subprocess -- without job objects it gets stuck on Windows
                // until the subprocess exits.)
                assert!(
                    run_status.time_taken < Duration::from_secs(5),
                    "{test_name} should have taken less than 5 seconds, actually took {:?}",
                    run_status.time_taken
                );
                run_status.result
                    == ExecutionResult::Timeout {
                        result: SlowTimeoutResult::Fail,
                    }
            }
        };
        if !valid {
            panic!(
                "for test_slow_timeout, mismatch in status: expected timeout, actual {:?}",
                instance_value.status
            );
        }
    }

    Ok(())
}

#[test]
fn test_override_timeout_result() -> Result<()> {
    test_init();

    let pcx = ParseContext::new(&PACKAGE_GRAPH);
    let expr = Filterset::parse(
        "test(/^test_slow_timeout/)".to_owned(),
        &pcx,
        FiltersetKind::Test,
    )
    .unwrap();
    let test_filter = TestFilterBuilder::new(
        NextestRunMode::Test,
        RunIgnored::Only,
        None,
        TestFilterPatterns::default(),
        vec![expr],
    )
    .unwrap();

    let test_list = FIXTURE_TARGETS.make_test_list(
        "with-timeout-success",
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let config = load_config();
    let profile = config
        .profile("with-timeout-success")
        .expect("with-timeout-success config is valid");
    let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
    let profile = profile.apply_build_platforms(&build_platforms);

    let runner = TestRunnerBuilder::default()
        .build(
            &test_list,
            &profile,
            vec![],
            SignalHandlerKind::Noop,
            InputHandlerKind::Noop,
            DoubleSpawnInfo::disabled(),
            TargetRunner::empty(),
        )
        .unwrap();

    let (instance_statuses, run_stats) = execute_collect(runner);

    println!("{instance_statuses:?}");
    assert_eq!(run_stats.finished_count, 3, "3 tests should have finished");
    assert_eq!(
        run_stats.passed_timed_out, 1,
        "1 test should pass with timeout"
    );
    assert_eq!(run_stats.failed_timed_out, 2, "2 tests should fail");

    for test_name in [
        "test_slow_timeout",
        "test_slow_timeout_2",
        "test_slow_timeout_subprocess",
    ] {
        let (_, instance_value) = instance_statuses
            .iter()
            .find(|&(&(_, name), _)| name == test_name)
            .unwrap_or_else(|| panic!("{test_name} should be present"));
        let (expected, actual) = match &instance_value.status {
            InstanceStatus::Skipped(_) => panic!("{test_name} should have been run"),
            InstanceStatus::Finished(run_statuses) => {
                assert_eq!(
                    run_statuses.len(),
                    1,
                    "{test_name} should have been run exactly once",
                );
                let run_status = run_statuses.last_status();
                assert!(
                    run_status.time_taken < Duration::from_secs(5),
                    "{test_name} should have taken less than 5 seconds, actually took {:?}",
                    run_status.time_taken
                );
                if matches!(test_name, "test_slow_timeout") {
                    (
                        ExecutionResult::Timeout {
                            result: SlowTimeoutResult::Pass,
                        },
                        run_status.result,
                    )
                } else {
                    (
                        ExecutionResult::Timeout {
                            result: SlowTimeoutResult::Fail,
                        },
                        run_status.result,
                    )
                }
            }
        };

        assert_eq!(
            expected, actual,
            "for test_slow_timeout, mismatch in status: expected {:?}, actual {:?}",
            expected, actual
        );
    }

    Ok(())
}
