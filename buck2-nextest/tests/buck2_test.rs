// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Drives `buck2 test` against the example project.
//!
//! This is the test for the way people actually use `buck2-nextest`: Buck2
//! launches it, hands it test targets over gRPC, and renders the results it
//! reports back. Everything is asserted through Buck2's own output and exit
//! code, since that is what a user sees.
//!
//! The test is gated on `buck2` being installed; see [`common`] for what that
//! means for a passing result.

mod common;

use common::{DaemonGuard, assert_success, buck2, buck2_is_installed, example_dir};
use std::{path::Path, process::Output};

/// A private isolation directory, so this never disturbs a developer's daemon.
const ISOLATION_DIR: &str = "nextest-buck2-test";

#[test]
fn buck2_test_runs_tests_with_nextest() {
    if !buck2_is_installed() {
        eprintln!("skipping: `buck2` is not installed, so the example cannot be built");
        return;
    }

    let example = example_dir();
    let _daemon = DaemonGuard::new(&example, ISOLATION_DIR);

    // Everything passes, so Buck2 succeeds and says so.
    let output = run(&example, &["//..."]);
    assert_success(&output, "buck2 test //...");
    let summary = summary_line(&output);
    assert!(
        summary.contains("Pass 5") && summary.contains("Fail 0") && summary.contains("Skip 1"),
        "every test passed and the ignored one was skipped, got:\n{summary}"
    );

    // A filterset after `--` reaches nextest.
    let output = run(
        &example,
        &["//...", "--", "-E", "binary_id(root//:demo-lib-test)"],
    );
    assert_success(&output, "buck2 test with a filterset");
    let summary = summary_line(&output);
    assert!(
        summary.contains("Pass 3"),
        "the filterset selected one binary's tests, got:\n{summary}"
    );

    // `--run-ignored all` reaches nextest too, and picks up the ignored test.
    let output = run(&example, &["//...", "--", "--run-ignored", "all"]);
    assert_success(&output, "buck2 test with --run-ignored");
    let summary = summary_line(&output);
    assert!(
        summary.contains("Pass 6") && summary.contains("Skip 0"),
        "the ignored test ran, got:\n{summary}"
    );

    // A target with no tests in it is not an error: Buck2 chose the target, and
    // nextest's usual "no tests is a failure" rule does not apply.
    let output = run(&example, &["//:demo"]);
    assert_success(&output, "buck2 test on a target with no tests");
}

/// Runs `buck2 test`, pointing Buck2 at the binary this test was built with.
fn run(example: &Path, args: &[&str]) -> Output {
    let executor = env!("CARGO_BIN_EXE_buck2-nextest");
    buck2(
        example,
        ISOLATION_DIR,
        ["test", "-c", &format!("test.v2_test_executor={executor}")],
    )
    .args(args)
    // The outer run is usually nextest, which exports these; the inner one
    // must not inherit them or its profile and output depend on how the outer
    // one was invoked.
    .env_remove("NEXTEST_PROFILE")
    .env_remove("NEXTEST_HIDE_PROGRESS_BAR")
    .output()
    .expect("`buck2 test` starts")
}

/// Extracts Buck2's own summary line, which is where results land.
fn summary_line(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| line.contains("Tests finished:"))
        .unwrap_or_else(|| {
            panic!("buck2 printed a summary\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}")
        })
        .to_owned()
}
