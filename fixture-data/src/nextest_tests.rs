// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Information about the "nextest-tests" fixture.
//!
//! TODO: need a better name than "nextest-tests".

use crate::models::{
    TestCaseFixture, TestCaseFixtureProperty, TestCaseFixtureStatus, TestSuiteFixture,
    TestSuiteFixtureProperty,
};
use maplit::btreemap;
use nextest_metadata::{BuildPlatform, RustBinaryId};
use once_cell::sync::Lazy;
use std::collections::BTreeMap;

pub static EXPECTED_TEST_SUITES: Lazy<BTreeMap<RustBinaryId, TestSuiteFixture>> = Lazy::new(|| {
    btreemap! {
        // Integration tests
        "nextest-tests::basic".into() => TestSuiteFixture::new(
            "nextest-tests::basic",
            "basic",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("test_cargo_env_vars", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperty::NotInDefaultSetUnix),
                TestCaseFixture::new("test_cwd", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperty::NeedsSameCwd),
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
                TestCaseFixture::new("test_stdin_closed", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_subprocess_doesnt_exit", TestCaseFixtureStatus::Leak),
                TestCaseFixture::new("test_success", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("test_success_should_panic", TestCaseFixtureStatus::Pass),
            ],
        ),
        "nextest-tests::other".into() => TestSuiteFixture::new(
            "nextest-tests::other",
            "other",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("other_test_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        "nextest-tests::segfault".into() => TestSuiteFixture::new(
            "nextest-tests::segfault",
            "segfault",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("test_segfault", TestCaseFixtureStatus::Segfault),
            ],
        ),
        // Unit tests
        "nextest-tests".into() => TestSuiteFixture::new(
            "nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::call_dylib_add_two", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::unit_test_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        // Binary tests
        "nextest-tests::bin/nextest-tests".into() => TestSuiteFixture::new(
            "nextest-tests::bin/nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::bin_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        "nextest-tests::bin/other".into() => TestSuiteFixture::new(
            "nextest-tests::bin/other",
            "other",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::other_bin_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        // Example tests
        "nextest-tests::example/nextest-tests".into() => TestSuiteFixture::new(
            "nextest-tests::example/nextest-tests",
            "nextest-tests",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::example_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        "nextest-tests::example/other".into() => TestSuiteFixture::new(
            "nextest-tests::example/other",
            "other",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::other_example_success", TestCaseFixtureStatus::Pass),
            ],
        ),
        // Benchmarks
        "nextest-tests::bench/my-bench".into() => TestSuiteFixture::new(
            "nextest-tests::bench/my-bench",
            "my-bench",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("bench_add_two", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::test_execute_bin", TestCaseFixtureStatus::Pass),
            ],
        ),
        // Proc-macro tests
        "nextest-derive".into() => TestSuiteFixture::new(
            "nextest-derive",
            "nextest-derive",
            BuildPlatform::Host,
            vec![
                TestCaseFixture::new("it_works", TestCaseFixtureStatus::Pass),
            ],
        ),
        // Dynamic library tests
        "cdylib-link".into() => TestSuiteFixture::new(
            "cdylib-link",
            "cdylib-link",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("test_multiply_two", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo),
            ],
        ),
        "dylib-test".into() => TestSuiteFixture::new(
            "dylib-test",
            "dylib-test",
            BuildPlatform::Target,
            vec![],
        ),
        "cdylib-example".into() => TestSuiteFixture::new(
            "cdylib-example",
            "cdylib-example",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::test_multiply_two_cdylib", TestCaseFixtureStatus::Pass)
                    .with_property(TestCaseFixtureProperty::MatchesCdylib)
                    .with_property(TestCaseFixtureProperty::MatchesTestMultiplyTwo),
            ],
        )
        .with_property(TestSuiteFixtureProperty::NotInDefaultSet),
        // Build script tests
        "with-build-script".into() => TestSuiteFixture::new(
            "with-build-script",
            "with-build-script",
            BuildPlatform::Target,
            vec![
                TestCaseFixture::new("tests::test_build_script_vars_set", TestCaseFixtureStatus::Pass),
                TestCaseFixture::new("tests::test_out_dir_present", TestCaseFixtureStatus::Pass),
            ],
        ),
        "proc-macro-test".into() => TestSuiteFixture::new(
            "proc-macro-test",
            "proc-macro-test",
            BuildPlatform::Host,
            vec![],
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
