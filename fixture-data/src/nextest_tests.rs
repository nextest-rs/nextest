// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Information about the "nextest-tests" fixture.
//!
//! TODO: need a better name than "nextest-tests".

use crate::models::{BinaryFixture, FixtureStatus, TestFixture};
use maplit::btreemap;
use nextest_metadata::{BuildPlatform, RustBinaryId};
use once_cell::sync::Lazy;
use std::collections::BTreeMap;

pub static EXPECTED_TESTS: Lazy<BTreeMap<RustBinaryId, Vec<TestFixture>>> = Lazy::new(|| {
    btreemap! {
        // Integration tests
        "nextest-tests::basic".into() => vec![
            TestFixture { name: "test_cargo_env_vars", status: FixtureStatus::Pass },
            TestFixture { name: "test_cwd", status: FixtureStatus::Pass },
            TestFixture { name: "test_execute_bin", status: FixtureStatus::Pass },
            TestFixture { name: "test_failure_assert", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_error", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_should_panic", status: FixtureStatus::Fail },
            TestFixture { name: "test_flaky_mod_4", status: FixtureStatus::Flaky { pass_attempt: 4 } },
            TestFixture { name: "test_flaky_mod_6", status: FixtureStatus::Flaky { pass_attempt: 6 } },
            TestFixture { name: "test_ignored", status: FixtureStatus::IgnoredPass },
            TestFixture { name: "test_ignored_fail", status: FixtureStatus::IgnoredFail },
            TestFixture { name: "test_result_failure", status: FixtureStatus::Fail },
            TestFixture { name: "test_slow_timeout", status: FixtureStatus::IgnoredPass },
            TestFixture { name: "test_slow_timeout_2", status: FixtureStatus::IgnoredPass },
            TestFixture { name: "test_slow_timeout_subprocess", status: FixtureStatus::IgnoredPass },
            TestFixture { name: "test_stdin_closed", status: FixtureStatus::Pass },
            TestFixture { name: "test_subprocess_doesnt_exit", status: FixtureStatus::Leak },
            TestFixture { name: "test_success", status: FixtureStatus::Pass },
            TestFixture { name: "test_success_should_panic", status: FixtureStatus::Pass },
        ],
        "nextest-tests::other".into() => vec![
            TestFixture { name: "other_test_success", status: FixtureStatus::Pass },
        ],
        "nextest-tests::segfault".into() => vec![
            TestFixture { name: "test_segfault", status: FixtureStatus::Segfault },
        ],
        // Unit tests
        "nextest-tests".into() => vec![
            TestFixture { name: "tests::call_dylib_add_two", status: FixtureStatus::Pass },
            TestFixture { name: "tests::unit_test_success", status: FixtureStatus::Pass },
        ],
        // Binary tests
        "nextest-tests::bin/nextest-tests".into() => vec![
            TestFixture { name: "tests::bin_success", status: FixtureStatus::Pass },
        ],
        "nextest-tests::bin/other".into() => vec![
            TestFixture { name: "tests::other_bin_success", status: FixtureStatus::Pass },
        ],
        // Example tests
        "nextest-tests::example/nextest-tests".into() => vec![
            TestFixture { name: "tests::example_success", status: FixtureStatus::Pass },
        ],
        "nextest-tests::example/other".into() => vec![
            TestFixture { name: "tests::other_example_success", status: FixtureStatus::Pass },
        ],
        // Benchmarks
        "nextest-tests::bench/my-bench".into() => vec![
            TestFixture { name: "bench_add_two", status: FixtureStatus::Pass },
            TestFixture { name: "tests::test_execute_bin", status: FixtureStatus::Pass },
        ],
        // Proc-macro tests
        "nextest-derive".into() => vec![
            TestFixture { name: "it_works", status: FixtureStatus::Pass },
        ],
        // Dynamic library tests
        "cdylib-link".into() => vec![
            TestFixture { name: "test_multiply_two", status: FixtureStatus::Pass },
        ],
        "dylib-test".into() => vec![],
        "cdylib-example".into() => vec![
            TestFixture { name: "tests::test_multiply_two_cdylib", status: FixtureStatus::Pass },
        ],
        // Build script tests
        "with-build-script".into() => vec![
            TestFixture { name: "tests::test_out_dir_present", status: FixtureStatus::Pass },
        ],
        "proc-macro-test".into() => vec![],
    }
});

pub fn get_expected_test(binary_id: &RustBinaryId, test_name: &str) -> &'static TestFixture {
    let v = EXPECTED_TESTS
        .get(binary_id)
        .unwrap_or_else(|| panic!("binary id {binary_id} not found"));
    v.iter()
        .find(|fixture| fixture.name == test_name)
        .unwrap_or_else(|| panic!("for binary id {binary_id}, test name {test_name} not found"))
}

pub static EXPECTED_BINARY_LIST: &[BinaryFixture] = &[
    BinaryFixture {
        binary_id: "nextest-derive",
        binary_name: "nextest-derive",
        build_platform: BuildPlatform::Host,
    },
    BinaryFixture {
        binary_id: "nextest-tests",
        binary_name: "nextest-tests",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::basic",
        binary_name: "basic",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::bin/nextest-tests",
        binary_name: "nextest-tests",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::bin/other",
        binary_name: "other",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::example/nextest-tests",
        binary_name: "nextest-tests",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::example/other",
        binary_name: "other",
        build_platform: BuildPlatform::Target,
    },
    BinaryFixture {
        binary_id: "nextest-tests::other",
        binary_name: "other",
        build_platform: BuildPlatform::Target,
    },
];
