// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An end-to-end test over the example Buck2 project in `example/`.
//!
//! This is the only coverage that checks `buck2-nextest` against a spec a real
//! Buck2 prelude produced, rather than one written by hand: it runs the BXL
//! script to generate the spec, parses it with the crate's own parser, and then
//! lists and runs the tests.
//!
//! The test is gated on `buck2` being installed. Nextest has no skip state, so
//! when `buck2` is missing this test passes without checking anything -- a pass
//! here is not on its own evidence that the example works.

mod common;

use buck2_nextest::spec::parse_spec;
use common::{DaemonGuard, assert_success, buck2, buck2_is_installed, example_dir};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// A private isolation directory, so the test never disturbs -- or is disturbed
/// by -- a Buck2 daemon a developer is using in the same project.
const ISOLATION_DIR: &str = "nextest-example-test";

/// The target pattern the spec is generated for.
const TARGET_PATTERN: &str = "//...";

#[test]
fn example_project_runs_end_to_end() {
    if !buck2_is_installed() {
        eprintln!("skipping: `buck2` is not installed, so the example cannot be built");
        return;
    }

    let example = example_dir();
    // Kills the daemon this test started, even if an assertion below panics.
    let _daemon = DaemonGuard::new(&example, ISOLATION_DIR);

    let spec_path = generate_spec(&example);
    check_spec_matches_the_crates_model(&example, &spec_path);
    check_list(&example, &spec_path);
    check_run(&example, &spec_path);
    check_filterset_narrows_to_one_binary(&example, &spec_path);
}

/// Checks that the spec the prelude produced parses into what the crate expects.
///
/// This is the assertion that catches a drift between `spec::types` and what
/// Buck2 actually emits; the checks below would fail too, but with an error far
/// removed from the cause.
fn check_spec_matches_the_crates_model(example: &Path, spec_path: &Path) {
    let contents = fs::read_to_string(spec_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", spec_path.display()));
    // Debug rather than Display: `ExpectedError`'s message is a summary, and
    // the serde path underneath it is what names the offending field.
    let targets = parse_spec(&contents, "the generated spec")
        .unwrap_or_else(|error| panic!("the generated spec parses: {error:?}\n{contents}"));

    let labels: Vec<_> = targets.iter().map(|target| target.label.as_str()).collect();
    assert_eq!(
        labels,
        ["root//:demo-integration-test", "root//:demo-lib-test"],
        "both test targets are in the spec, and the library is not"
    );

    let integration = &targets[0];
    // A substring rather than a suffix, since Windows adds an `.exe`.
    assert!(
        integration.program.contains("demo_integration_test"),
        "the command's program is the test binary, got {}",
        integration.program
    );
    assert!(
        integration.leading_args.is_empty(),
        "the prelude passes no leading arguments, got {:?}",
        integration.leading_args
    );

    // The greeting path is a Buck2-built artifact, so this also confirms that
    // `write_json(with_inputs = True)` materialized it.
    let greeting = integration
        .env
        .get("DEMO_GREETING_PATH")
        .expect("the integration test's env made it into the spec");
    let greeting = example.join(greeting);
    assert_eq!(
        fs::read_to_string(&greeting)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", greeting.display()))
            .trim(),
        "hello from buck2",
    );

    assert!(
        targets[1].env.is_empty(),
        "the unit test target has no env, got {:?}",
        targets[1].env
    );
}

fn check_list(example: &Path, spec_path: &Path) {
    let output = run_nextest(example, spec_path, ["list", "--format", "oneline"]);
    assert_success(&output, "list");

    let stdout = String::from_utf8(output.stdout).expect("list output is UTF-8");
    let mut listed: Vec<_> = stdout.lines().collect();
    listed.sort_unstable();
    assert_eq!(
        listed,
        [
            "root//:demo-integration-test add_across_crates",
            "root//:demo-integration-test greeting_comes_from_buck2",
            "root//:demo-lib-test tests::add_identity",
            "root//:demo-lib-test tests::add_is_commutative",
            "root//:demo-lib-test tests::add_two_numbers",
        ],
        "the ignored test is not listed, and binary IDs are Buck2 labels"
    );
}

fn check_run(example: &Path, spec_path: &Path) {
    let output = run_nextest(example, spec_path, ["run"]);
    assert_success(&output, "run");

    // The reporter writes to standard error.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("5 tests run: 5 passed, 1 skipped"),
        "all five tests ran and passed, got:\n{stderr}"
    );
}

fn check_filterset_narrows_to_one_binary(example: &Path, spec_path: &Path) {
    let output = run_nextest(
        example,
        spec_path,
        ["run", "-E", "binary_id(root//:demo-lib-test)"],
    );
    assert_success(&output, "filtered run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("3 tests run: 3 passed"),
        "the filterset selected only the unit test binary, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("demo-integration-test"),
        "no test from the other binary ran, got:\n{stderr}"
    );
}

// ---
// Helpers
// ---

/// Runs the BXL script, returning the path of the spec it wrote.
fn generate_spec(example: &Path) -> PathBuf {
    let output = buck2(
        example,
        ISOLATION_DIR,
        [
            "bxl",
            "//bxl/nextest.bxl:generate",
            "--",
            "--target",
            TARGET_PATTERN,
        ],
    )
    .output()
    .expect("`buck2 bxl` starts");
    assert_success(&output, "buck2 bxl");

    // The script prints one line: the absolute path of the spec. Everything
    // else buck2 emits goes to standard error.
    let stdout = String::from_utf8(output.stdout).expect("bxl output is UTF-8");
    let path = PathBuf::from(stdout.trim());
    assert!(
        path.is_absolute(),
        "the BXL script printed an absolute path, got {}",
        path.display()
    );
    path
}

fn run_nextest<I, S>(example: &Path, spec_path: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_buck2-nextest"))
        .args(args)
        .arg("--spec")
        .arg(spec_path)
        .arg("--project-root")
        .arg(example)
        // This test suite is itself usually run under nextest, which exports
        // `NEXTEST_PROFILE`. `buck2-nextest` reads it as `--profile`, so it is
        // cleared to keep the assertions independent of how the outer run was
        // invoked.
        .env_remove("NEXTEST_PROFILE")
        .output()
        .expect("`buck2-nextest` starts")
}
