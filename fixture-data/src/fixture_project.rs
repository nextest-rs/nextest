// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Information about the "fixture-project" fixture.

use crate::models::{
    TestCaseFixture, TestCaseFixtureProperties, TestCaseFixtureStatus, TestSuiteFixture,
    TestSuiteFixtureProperties,
};
use iddqd::{IdOrdMap, id_ord_map};
use nextest_metadata::BuildPlatform;
use std::sync::LazyLock;

pub static EXPECTED_TEST_SUITES: LazyLock<IdOrdMap<TestSuiteFixture>> = LazyLock::new(|| {
    id_ord_map! {
        // Integration tests
        TestSuiteFixture::new(
            "fixture-project::basic",
            "basic",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("test_cargo_env_vars", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET_UNIX),
                TestCaseFixture::new("test_overrides_wrapper_env", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_cwd", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_execute_bin", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_failure_assert", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_failure_error", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_failure_should_panic", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new(
                    "test_flaky_mod_3",
                    TestCaseFixtureStatus::Flaky { pass_attempt: 3 },
                )
                .with_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET)
                .with_property(TestCaseFixtureProperties::FLAKY_RESULT_FAIL_JUNIT_SUCCESS),
                TestCaseFixture::new(
                    "test_flaky_mod_4",
                    TestCaseFixtureStatus::Flaky { pass_attempt: 4 },
                )
                .with_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET)
                .with_property(TestCaseFixtureProperties::FLAKY_RESULT_FAIL),
                TestCaseFixture::new(
                    "test_flaky_mod_6",
                    TestCaseFixtureStatus::Flaky { pass_attempt: 6 },
                )
                .with_property(TestCaseFixtureProperties::NOT_IN_DEFAULT_SET),
                TestCaseFixture::new("test_ignored", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("test_ignored_fail", TestCaseFixtureStatus::IgnoredFail),
                TestCaseFixture::new("test_result_failure", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_slow_timeout", TestCaseFixtureStatus::IgnoredPass)
                    .with_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING)
                    .with_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING)
                    .with_property(TestCaseFixtureProperties::EXACT_TEST_SLOW_TIMEOUT),
                TestCaseFixture::new("test_slow_timeout_2", TestCaseFixtureStatus::IgnoredPass)
                    .with_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING)
                    .with_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING),
                TestCaseFixture::new(
                    "test_slow_timeout_subprocess",
                    TestCaseFixtureStatus::IgnoredPass,
                )
                    .with_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING)
                    .with_property(TestCaseFixtureProperties::TEST_SLOW_TIMEOUT_SUBSTRING),
                TestCaseFixture::new(
                    "test_flaky_slow_timeout_mod_3",
                    TestCaseFixtureStatus::IgnoredFlaky { pass_attempt: 3 },
                )
                    .with_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING)
                    .with_property(TestCaseFixtureProperties::FLAKY_SLOW_TIMEOUT_SUBSTRING),
                TestCaseFixture::new("test_stdin_closed", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_subprocess_doesnt_exit", TestCaseFixtureStatus::Leak),
                TestCaseFixture::new("test_subprocess_doesnt_exit_fail", TestCaseFixtureStatus::FailLeak),
                TestCaseFixture::new("test_subprocess_doesnt_exit_leak_fail", TestCaseFixtureStatus::LeakFail),
                TestCaseFixture::new("test_success", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_success_should_panic", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "fixture-project::other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("other_test_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "fixture-project::segfault",
            "segfault",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("test_segfault", TestCaseFixtureStatus::Segfault),
            },
        ),
        // Unit tests
        TestSuiteFixture::new(
            "fixture-project",
            "fixture-project",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::call_dylib_add_two", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::unit_test_success", TestCaseFixtureStatus::Pass),
            },
        ),
        // Binary tests
        TestSuiteFixture::new(
            "fixture-project::bin/fixture-project",
            "fixture-project",
            BuildPlatform::Target,
            id_ord_map! {
                // This is a fake test name produced by wrapper.rs.
                TestCaseFixture::new("fake_test_name", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("tests::bin_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "fixture-project::bin/other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::other_bin_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "fixture-project::bin/wrapper",
            "wrapper",
            BuildPlatform::Target,
            IdOrdMap::new(),
        ),
        // Example tests
        TestSuiteFixture::new(
            "fixture-project::example/fixture-project",
            "fixture-project",
            BuildPlatform::Target,
            id_ord_map! {
                // This is a fake test name produced by wrapper.rs.
                TestCaseFixture::new("fake_test_name", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("tests::example_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "fixture-project::example/other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::other_example_success", TestCaseFixtureStatus::Pass),
            },
        ),
        // Benchmarks
        TestSuiteFixture::new(
            "fixture-project::bench/my-bench",
            "my-bench",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("bench_add_two", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperties::IS_BENCHMARK),
                TestCaseFixture::new("bench_ignored", TestCaseFixtureStatus::IgnoredPass)
                    .with_property(TestCaseFixtureProperties::IS_BENCHMARK),
                TestCaseFixture::new("bench_slow_timeout", TestCaseFixtureStatus::IgnoredPass)
                    .with_property(TestCaseFixtureProperties::IS_BENCHMARK)
                    .with_property(TestCaseFixtureProperties::BENCH_OVERRIDE_TIMEOUT)
                    .with_property(TestCaseFixtureProperties::BENCH_TERMINATION)
                    .with_property(TestCaseFixtureProperties::BENCH_IGNORES_TEST_TIMEOUT)
                    .with_property(TestCaseFixtureProperties::SLOW_TIMEOUT_SUBSTRING),
                TestCaseFixture::new("tests::test_execute_bin", TestCaseFixtureStatus::Pass),
            },
        ),
        // Proc-macro tests
        TestSuiteFixture::new(
            "nextest-derive",
            "nextest-derive",
            BuildPlatform::Host,
            id_ord_map! {
                TestCaseFixture::new("it_works", TestCaseFixtureStatus::Pass),
            },
        ),
        // Dynamic library tests
        TestSuiteFixture::new(
            "cdylib-link",
            "cdylib-link",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("test_multiply_two", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperties::MATCHES_TEST_MULTIPLY_TWO),
            },
        ),
        TestSuiteFixture::new(
            "dylib-test",
            "dylib-test",
            BuildPlatform::Target,
            IdOrdMap::new(),
        ),
        TestSuiteFixture::new(
            "cdylib-example",
            "cdylib-example",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::test_multiply_two_cdylib", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperties::MATCHES_CDYLIB)
                    .with_property(TestCaseFixtureProperties::MATCHES_TEST_MULTIPLY_TWO),
            },
        )
        .with_property(TestSuiteFixtureProperties::NOT_IN_DEFAULT_SET)
        .with_property(TestSuiteFixtureProperties::MATCHES_CDYLIB_EXAMPLE),
        // Build script tests
        TestSuiteFixture::new(
            "with-build-script",
            "with-build-script",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::test_build_script_vars_set", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::test_out_dir_present", TestCaseFixtureStatus::Pass),
            }
        ),
        TestSuiteFixture::new(
            "proc-macro-test",
            "proc-macro-test",
            BuildPlatform::Host,
            IdOrdMap::new(),
        ),
    }
});
