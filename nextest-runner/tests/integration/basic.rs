// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use cfg_if::cfg_if;
use color_eyre::eyre::{Result, ensure};
use fixture_data::{
    models::{TestCaseFixtureStatus, TestNameAndFilterMatch, TestSuiteFixture},
    nextest_tests::EXPECTED_TEST_SUITES,
};
use iddqd::IdOrdMap;
use nextest_filtering::{Filterset, FiltersetKind, ParseContext};
use nextest_runner::{
    config::{
        core::NextestConfig,
        elements::{LeakTimeoutResult, RetryPolicy, SlowTimeoutResult},
    },
    double_spawn::DoubleSpawnInfo,
    input::InputHandlerKind,
    list::BinaryList,
    platform::BuildPlatforms,
    reporter::{
        UnitErrorDescription,
        events::{
            ExecutionDescription, ExecutionResult, FinalRunStats, RunStatsFailureKind, UnitKind,
        },
    },
    run_mode::NextestRunMode,
    runner::TestRunnerBuilder,
    signal::SignalHandlerKind,
    target_runner::TargetRunner,
    test_filter::{RunIgnored, TestFilterBuilder, TestFilterPatterns},
    test_output::{ChildExecutionOutput, ChildOutput},
};
use pretty_assertions::assert_eq;
use std::{io::Cursor, time::Duration};
use test_case::test_case;

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
fn test_timeout_with_retries() -> Result<()> {
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
        "with-timeout-retries-success",
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let config = load_config();
    let profile = config
        .profile("with-timeout-retries-success")
        .expect("with-timeout-retries-success config is valid");
    let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
    let profile = profile.apply_build_platforms(&build_platforms);

    let profile_retries = profile.retries();
    assert_eq!(
        profile_retries,
        RetryPolicy::new_without_delay(2),
        "retries set in with-timeout-retries-success profile"
    );

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

    let (instance_statuses, _run_stats) = execute_collect(runner);

    // With retries and on-timeout=pass, timed out tests should not be retried.
    for test_name in [
        "test_slow_timeout",
        "test_slow_timeout_2",
        "test_slow_timeout_subprocess",
    ] {
        let (_, instance_value) = instance_statuses
            .iter()
            .find(|&(&(_, name), _)| name == test_name)
            .unwrap_or_else(|| panic!("{test_name} should be present"));

        match &instance_value.status {
            InstanceStatus::Skipped(_) => panic!("{test_name} should have been run"),
            InstanceStatus::Finished(run_statuses) => {
                assert_eq!(
                    run_statuses.len(),
                    1,
                    "{test_name} should have been run exactly once \
                     (timed out tests that pass are not retried)",
                );

                let status = run_statuses.last_status();
                assert_eq!(
                    status.result,
                    ExecutionResult::Timeout {
                        result: SlowTimeoutResult::Pass
                    },
                    "{test_name} should have timed out with on-timeout=pass"
                );
            }
        };
    }

    Ok(())
}

#[test]
fn test_timeout_with_flaky() -> Result<()> {
    test_init();

    let pcx = ParseContext::new(&PACKAGE_GRAPH);
    let expr = Filterset::parse(
        "test(test_flaky_slow_timeout_mod_3)".to_owned(),
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
        "with-timeout-retries-success",
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let config = load_config();
    let profile = config
        .profile("with-timeout-retries-success")
        .expect("with-timeout-retries-success config is valid");
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

    let (instance_statuses, _run_stats) = execute_collect(runner);

    let (_, instance_value) = instance_statuses
        .iter()
        .find(|&(&(_, name), _)| name == "test_flaky_slow_timeout_mod_3")
        .unwrap_or_else(|| panic!("test_flaky_slow_timeout_mod_3 should be present"));

    match &instance_value.status {
        InstanceStatus::Skipped(_) => panic!("test_flaky_slow_timeout_mod_3 should have been run"),
        InstanceStatus::Finished(run_statuses) => {
            eprintln!("test_flaky_slow_timeout_mod_3 run statuses: {run_statuses:#?}");
            assert!(
                run_statuses.len() == 3,
                "test_flaky_slow_timeout_mod_3 should have been run 3 times, was run {} times",
                run_statuses.len()
            );

            match run_statuses.describe() {
                ExecutionDescription::Flaky {
                    last_status,
                    prior_statuses,
                } => {
                    for (i, prior_status) in prior_statuses.iter().enumerate() {
                        assert!(
                            matches!(prior_status.result, ExecutionResult::Fail { .. }),
                            "prior attempt {} should be fail, got {:?}",
                            i + 1,
                            prior_status.result
                        );
                    }
                    assert!(
                        matches!(
                            last_status.result,
                            ExecutionResult::Timeout {
                                result: SlowTimeoutResult::Pass
                            }
                        ),
                        "last attempt should be a successful timeout, was {:?}",
                        last_status.result
                    );
                }
                other => panic!("test_flaky_slow_timeout_mod_3 should be flaky, found {other:?}"),
            }
        }
    };

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
fn test_run() -> Result<()> {
    test_init();

    let test_filter = TestFilterBuilder::default_set(NextestRunMode::Test, RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(
        NextestConfig::DEFAULT_PROFILE,
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let config = load_config();
    let profile = config
        .profile(NextestConfig::DEFAULT_PROFILE)
        .expect("default config is valid");
    let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
    let profile = profile.apply_build_platforms(&build_platforms);

    let runner = TestRunnerBuilder::default()
        .build(
            &test_list,
            &profile,
            vec![], // we aren't testing CLI args at the moment
            SignalHandlerKind::Noop,
            InputHandlerKind::Noop,
            DoubleSpawnInfo::disabled(),
            TargetRunner::empty(),
        )
        .unwrap();

    let (instance_statuses, run_stats) = execute_collect(runner);

    for expected in &*EXPECTED_TEST_SUITES {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
            .get(&expected.binary_id)
            .unwrap_or_else(|| panic!("unexpected binary ID {}", expected.binary_id));
        for fixture in &expected.test_cases {
            let instance_value = instance_statuses
                .get(&(test_binary.binary_path.as_path(), fixture.name))
                .unwrap_or_else(|| {
                    panic!(
                        "no instance status found for key ({}, {})",
                        test_binary.binary_path.as_path(),
                        fixture.name
                    )
                });
            let valid = || {
                match &instance_value.status {
                    InstanceStatus::Skipped(_) => {
                        ensure!(fixture.status.is_ignored(), "test should be skipped");
                        Ok(())
                    }
                    InstanceStatus::Finished(run_statuses) => {
                        // This test should not have been retried since retries aren't configured.
                        assert_eq!(
                            run_statuses.len(),
                            1,
                            "test {} should have been run exactly once",
                            fixture.name
                        );
                        let run_status = run_statuses.last_status();

                        ensure_execution_result(&run_status.result, fixture.status, 1)?;
                        // Extracting descriptions works for segfaults on Unix but not on Windows.
                        #[cfg_attr(not(unix), expect(unused_mut))]
                        let mut can_extract_description = fixture.status
                            == TestCaseFixtureStatus::Fail
                            || fixture.status == TestCaseFixtureStatus::IgnoredFail;
                        cfg_if! {
                            if #[cfg(unix)] {
                                can_extract_description |= fixture.status == TestCaseFixtureStatus::Segfault;
                            }
                        }

                        if can_extract_description {
                            // Check that stderr can be parsed heuristically.
                            let ChildExecutionOutput::Output {
                                output: ChildOutput::Split(split),
                                ..
                            } = &run_status.output
                            else {
                                panic!("this test should always use split output")
                            };
                            let stdout = split.stdout.as_ref().expect("stdout should be captured");
                            let stderr = split.stderr.as_ref().expect("stderr should be captured");

                            println!("stderr: {}", stderr.as_str_lossy());
                            let desc =
                                UnitErrorDescription::new(UnitKind::Test, &run_status.output);
                            assert!(
                                desc.child_process_error_list().is_some(),
                                "failed to extract description from {}\n*** stdout:\n{}\n*** stderr:\n{}\n",
                                fixture.name,
                                stdout.as_str_lossy(),
                                stderr.as_str_lossy(),
                            );
                        }

                        Ok(())
                    }
                }
            };
            if let Err(error) = valid() {
                panic!(
                    "for test {}, mismatch in status: expected {:?}, actual {:?}, error: {}",
                    fixture.name, fixture.status, instance_value.status, error,
                );
            }
        }
    }

    // Note: can't compare not_run because its exact value would depend on the number of threads on
    // the machine.
    assert!(
        matches!(
            run_stats.summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::Test { .. })
        ),
        "run should be marked failed, but got {:?}",
        run_stats.summarize_final(),
    );
    Ok(())
}

#[test_case(
    None
    ; "retry overrides obeyed"
)]
#[test_case(
    Some(RetryPolicy::new_without_delay(2))
    ; "retry overrides ignored"
)]
fn test_retries(retries: Option<RetryPolicy>) -> Result<()> {
    test_init();

    let test_filter = TestFilterBuilder::default_set(NextestRunMode::Test, RunIgnored::Default);
    let test_list = FIXTURE_TARGETS.make_test_list(
        NextestConfig::DEFAULT_PROFILE,
        &test_filter,
        &TargetRunner::empty(),
    )?;
    let config = load_config();
    let profile = config
        .profile("with-retries")
        .expect("with-retries config is valid");
    let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
    let profile = profile.apply_build_platforms(&build_platforms);

    let profile_retries = profile.retries();
    assert_eq!(
        profile_retries,
        RetryPolicy::new_without_delay(2),
        "retries set in with-retries profile"
    );

    let mut builder = TestRunnerBuilder::default();
    if let Some(retries) = retries {
        builder.set_retries(retries);
    }
    let runner = builder
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

    for expected in &*EXPECTED_TEST_SUITES {
        let test_binary = FIXTURE_TARGETS
            .test_artifacts
            .get(&expected.binary_id)
            .unwrap_or_else(|| panic!("unexpected binary ID {}", expected.binary_id));
        for fixture in &expected.test_cases {
            let instance_value =
                &instance_statuses[&(test_binary.binary_path.as_path(), fixture.name)];
            let valid = match &instance_value.status {
                InstanceStatus::Skipped(_) => fixture.status.is_ignored(),
                InstanceStatus::Finished(run_statuses) => {
                    let expected_len = match fixture.status {
                        TestCaseFixtureStatus::Flaky { pass_attempt } => {
                            if retries.is_some() {
                                pass_attempt.min(profile_retries.count() + 1)
                            } else {
                                pass_attempt
                            }
                        }
                        TestCaseFixtureStatus::Pass | TestCaseFixtureStatus::Leak => 1,
                        // Note that currently only the flaky test fixtures are controlled by overrides.
                        // If more tests are controlled by retry overrides, this may need to be updated.
                        TestCaseFixtureStatus::LeakFail
                        | TestCaseFixtureStatus::Fail
                        | TestCaseFixtureStatus::FailLeak
                        | TestCaseFixtureStatus::Segfault => profile_retries.count() + 1,
                        TestCaseFixtureStatus::IgnoredPass | TestCaseFixtureStatus::IgnoredFail => {
                            unreachable!("ignored tests should be skipped")
                        }
                    };
                    assert_eq!(
                        run_statuses.len(),
                        expected_len as usize,
                        "test {} should be run {} times",
                        fixture.name,
                        expected_len,
                    );

                    match run_statuses.describe() {
                        ExecutionDescription::Success { single_status } => {
                            if fixture.status == TestCaseFixtureStatus::Leak {
                                single_status.result
                                    == ExecutionResult::Leak {
                                        result: LeakTimeoutResult::Pass,
                                    }
                            } else {
                                single_status.result == ExecutionResult::Pass
                            }
                        }
                        ExecutionDescription::Flaky {
                            last_status,
                            prior_statuses,
                        } => {
                            assert_eq!(
                                prior_statuses.len(),
                                (expected_len - 1) as usize,
                                "correct length for prior statuses"
                            );
                            for prior_status in prior_statuses {
                                assert!(
                                    matches!(prior_status.result, ExecutionResult::Fail { .. }),
                                    "prior status {} should be fail",
                                    prior_status.retry_data.attempt
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
                                (expected_len - 1) as usize,
                                "correct length for retries"
                            );
                            for retry in retries {
                                assert!(
                                    matches!(
                                        retry.result,
                                        ExecutionResult::Fail { .. }
                                            | ExecutionResult::Leak {
                                                result: LeakTimeoutResult::Fail
                                            }
                                    ),
                                    "retry {} should be fail or leak => fail",
                                    retry.retry_data.attempt
                                );
                            }
                            matches!(
                                first_status.result,
                                ExecutionResult::Fail { .. }
                                    | ExecutionResult::Leak {
                                        result: LeakTimeoutResult::Fail
                                    }
                            )
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

    // Note: can't compare not_run because its exact value would depend on the number of threads on
    // the machine.
    assert!(
        matches!(
            run_stats.summarize_final(),
            FinalRunStats::Failed(RunStatsFailureKind::Test { .. })
        ),
        "run should be marked failed, but got {:?}",
        run_stats.summarize_final(),
    );
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
