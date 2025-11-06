// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Information about the "nextest-tests" fixture.
//!
//! TODO: need a better name than "nextest-tests".

use crate::models::{
    TestCaseFixture, TestCaseFixtureProperty, TestCaseFixtureStatus, TestSuiteFixture,
    TestSuiteFixtureProperty,
};
use iddqd::{IdOrdMap, id_ord_map};
use nextest_metadata::{BuildPlatform, RustBinaryId};
use std::sync::LazyLock;

pub static EXPECTED_TEST_SUITES: LazyLock<IdOrdMap<TestSuiteFixture>> = LazyLock::new(|| {
    id_ord_map! {
        // Integration tests
        TestSuiteFixture::new(
            "nextest-tests::basic",
            "basic",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("test_cargo_env_vars", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperty::NotInDefaultSetUnix),
                TestCaseFixture::new("test_cwd", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_execute_bin", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_failure_assert", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_failure_error", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_failure_should_panic", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new(
                    "test_flaky_mod_4",
                    TestCaseFixtureStatus::Flaky { pass_attempt: 4 },
                )
                .with_property(TestCaseFixtureProperty::NotInDefaultSet),
                TestCaseFixture::new(
                    "test_flaky_mod_6",
                    TestCaseFixtureStatus::Flaky { pass_attempt: 6 },
                )
                .with_property(TestCaseFixtureProperty::NotInDefaultSet),
                TestCaseFixture::new("test_ignored", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("test_ignored_fail", TestCaseFixtureStatus::IgnoredFail),
                TestCaseFixture::new("test_result_failure", TestCaseFixtureStatus::Fail),
                TestCaseFixture::new("test_slow_timeout", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("test_slow_timeout_2", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new(
                    "test_slow_timeout_subprocess",
                    TestCaseFixtureStatus::IgnoredPass,
                ),
                TestCaseFixture::new(
                    "test_flaky_slow_timeout_mod_3",
                    TestCaseFixtureStatus::IgnoredFail
                ),
                TestCaseFixture::new("test_stdin_closed", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_subprocess_doesnt_exit", TestCaseFixtureStatus::Leak),
                TestCaseFixture::new("test_subprocess_doesnt_exit_fail", TestCaseFixtureStatus::FailLeak),
                TestCaseFixture::new("test_subprocess_doesnt_exit_leak_fail", TestCaseFixtureStatus::LeakFail),
                TestCaseFixture::new("test_success", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_success_should_panic", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "nextest-tests::other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("other_test_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "nextest-tests::segfault",
            "segfault",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("test_segfault", TestCaseFixtureStatus::Segfault),
            },
        ),
        // Unit tests
        TestSuiteFixture::new(
            "nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::call_dylib_add_two", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::unit_test_success", TestCaseFixtureStatus::Pass),
            },
        ),
        // Binary tests
        TestSuiteFixture::new(
            "nextest-tests::bin/nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            id_ord_map! {
                // This is a fake test name produced by wrapper.rs.
                TestCaseFixture::new("fake_test_name", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("tests::bin_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "nextest-tests::bin/other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::other_bin_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "nextest-tests::bin/wrapper",
            "wrapper",
            BuildPlatform::Target,
            IdOrdMap::new(),
        ),
        // Example tests
        TestSuiteFixture::new(
            "nextest-tests::example/nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            id_ord_map! {
                // This is a fake test name produced by wrapper.rs.
                TestCaseFixture::new("fake_test_name", TestCaseFixtureStatus::IgnoredPass),
                TestCaseFixture::new("tests::example_success", TestCaseFixtureStatus::Pass),
            },
        ),
        TestSuiteFixture::new(
            "nextest-tests::example/other",
            "other",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("tests::other_example_success", TestCaseFixtureStatus::Pass),
            },
        ),
        // Benchmarks
        TestSuiteFixture::new(
            "nextest-tests::bench/my-bench",
            "my-bench",
            BuildPlatform::Target,
            id_ord_map! {
                TestCaseFixture::new("bench_add_two", TestCaseFixtureStatus::Pass),
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
                    .with_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo),
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
                    .with_property(TestCaseFixtureProperty::MatchesCdylib)
                    .with_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo),
            },
        )
        .with_property(TestSuiteFixtureProperty::NotInDefaultSet)
        .with_property(TestSuiteFixtureProperty::MatchesCdylibExample),
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

pub fn get_expected_test(binary_id: &RustBinaryId, test_name: &str) -> &'static TestCaseFixture {
    let v = EXPECTED_TEST_SUITES
        .get(binary_id)
        .unwrap_or_else(|| panic!("binary id {binary_id} not found"));
    v.test_cases
        .iter()
        .find(|fixture| fixture.name == test_name)
        .unwrap_or_else(|| panic!("for binary id {binary_id}, test name {test_name} not found"))
}
