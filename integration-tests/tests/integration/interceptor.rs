// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for debugger and tracer modes.

use super::{fixtures::*, temp_project::TempProject};
use integration_tests::{env::set_env_vars, nextest_cli::CargoNextestCli};
use nextest_metadata::NextestExitCode;

fn fake_interceptor_path() -> String {
    std::env::var("NEXTEST_BIN_EXE_fake_interceptor")
        .expect("NEXTEST_BIN_EXE_fake_interceptor should be set by nextest")
}

#[test]
fn test_debugger_integration() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    save_binaries_metadata(&p);
    let fake_interceptor = fake_interceptor_path();
    let fake_debugger = shell_words::join([fake_interceptor.as_str(), "--mode=debugger"]);

    // Test: Too many tests selected: select exactly 2 tests with "multiply" filter.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--debugger",
            &fake_debugger,
            "-E",
            "test(~multiply)",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "should fail with SETUP_ERROR when multiple tests selected"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("--debugger requires exactly one test, but 2 tests were selected:"),
        "stderr should contain error message with count: {stderr}"
    );

    // Verify both multiply tests are listed.
    assert!(
        stderr.contains("cdylib-link test_multiply_two"),
        "stderr should list cdylib-link test_multiply_two: {stderr}"
    );
    assert!(
        stderr.contains("cdylib-example tests::test_multiply_two_cdylib"),
        "stderr should list cdylib-example test_multiply_two_cdylib: {stderr}"
    );

    // Should not have "... and X more tests" since we're showing both.
    assert!(
        !stderr.contains("more tests"),
        "stderr should not show 'more tests' when showing all: {stderr}"
    );

    // Test: No tests selected.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--debugger",
            &fake_debugger,
            "-E",
            "none()",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "should fail with SETUP_ERROR when no tests selected"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("no tests were selected"),
        "stderr should contain 'no tests' message: {stderr}"
    );

    // Test: Debugger runs successfully with exactly one test.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--debugger",
            &fake_debugger,
            "-E",
            "test(=test_multiply_two)",
        ])
        .output();

    assert!(
        output.exit_status.success(),
        "should succeed with debugger: {output}"
    );

    let stderr = output.stderr_as_str();

    // Verify the fake-interceptor ran in debugger mode.
    assert!(
        stderr.contains("[fake-interceptor] mode: debugger"),
        "stderr should show debugger mode was used: {stderr}"
    );

    // Verify debugger-specific properties.
    assert!(
        stderr.contains("[fake-debugger] stdin check:"),
        "stderr should contain stdin verification: {stderr}"
    );
    #[cfg(unix)]
    {
        assert!(
            stderr.contains(
                "[fake-debugger] process group check: ok (not in separate process group)"
            ),
            "stderr should show debugger is not in separate process group: {stderr}"
        );
    }

    // Test: --debugger conflicts with --no-run.
    let fake_debugger = shell_words::join([fake_interceptor.as_str(), "--mode=debugger"]);
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--debugger",
            &fake_debugger,
            "--no-run",
        ])
        .unchecked(true)
        .output();

    // clap should reject this with an error.
    assert!(
        !output.exit_status.success(),
        "should fail when --debugger and --no-run are both specified"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("the argument '--debugger <DEBUGGER>' cannot be used with '--no-run'"),
        "stderr should contain conflict error message: {stderr}"
    );
}

#[test]
fn test_tracer_integration() {
    set_env_vars();

    let p = TempProject::new().unwrap();
    save_binaries_metadata(&p);
    let fake_interceptor = fake_interceptor_path();
    let fake_tracer = shell_words::join([fake_interceptor.as_str(), "--mode=tracer"]);

    // Test: Too many tests selected: select exactly 2 tests with "multiply" filter.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--tracer",
            &fake_tracer,
            "-E",
            "test(~multiply)",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "should fail with SETUP_ERROR when multiple tests selected"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("--tracer requires exactly one test, but 2 tests were selected:"),
        "stderr should contain error message with count: {stderr}"
    );

    // Verify both multiply tests are listed.
    assert!(
        stderr.contains("cdylib-link test_multiply_two"),
        "stderr should list cdylib-link test_multiply_two: {stderr}"
    );
    assert!(
        stderr.contains("cdylib-example tests::test_multiply_two_cdylib"),
        "stderr should list cdylib-example test_multiply_two_cdylib: {stderr}"
    );

    // Should not have "... and X more tests" since we're showing both.
    assert!(
        !stderr.contains("more tests"),
        "stderr should not show 'more tests' when showing all: {stderr}"
    );

    // Test: No tests selected.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--tracer",
            &fake_tracer,
            "-E",
            "none()",
        ])
        .unchecked(true)
        .output();

    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "should fail with SETUP_ERROR when no tests selected"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("no tests were selected"),
        "stderr should contain 'no tests' message: {stderr}"
    );

    // Test: Tracer runs successfully with exactly one test.
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--tracer",
            &fake_tracer,
            "-E",
            "test(=test_multiply_two)",
        ])
        .output();

    assert!(
        output.exit_status.success(),
        "should succeed with tracer: {output}"
    );

    let stderr = output.stderr_as_str();

    // Verify the fake-interceptor ran in tracer mode.
    assert!(
        stderr.contains("[fake-interceptor] mode: tracer"),
        "stderr should show tracer mode was used: {stderr}"
    );

    // Verify tracer-specific properties.
    #[cfg(unix)]
    {
        assert!(
            stderr.contains("[fake-tracer] stdin is /dev/null (expected for tracer)"),
            "stderr should show tracer has stdin as /dev/null: {stderr}"
        );
        assert!(
            stderr.contains("[fake-tracer] process group check: ok (in own process group)"),
            "stderr should show tracer is in its own process group: {stderr}"
        );
    }

    // Test: --tracer conflicts with --no-run.
    let fake_tracer = shell_words::join([fake_interceptor.as_str(), "--mode=tracer"]);
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--tracer",
            &fake_tracer,
            "--no-run",
        ])
        .unchecked(true)
        .output();

    // clap should reject this with an error.
    assert!(
        !output.exit_status.success(),
        "should fail when --tracer and --no-run are both specified"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains("the argument '--tracer <TRACER>' cannot be used with '--no-run'"),
        "stderr should contain conflict error message: {stderr}"
    );

    // Test: --tracer conflicts with --debugger.
    let fake_debugger = shell_words::join([fake_interceptor.as_str(), "--mode=debugger"]);
    let output = CargoNextestCli::for_test()
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--workspace",
            "--tracer",
            &fake_tracer,
            "--debugger",
            &fake_debugger,
            "-E",
            "test(=test_multiply_two)",
        ])
        .unchecked(true)
        .output();

    // clap should reject this with an error.
    assert!(
        !output.exit_status.success(),
        "should fail when --tracer and --debugger are both specified"
    );

    let stderr = output.stderr_as_str();
    assert!(
        stderr.contains(
            "the argument '--tracer <TRACER>' cannot be used with '--debugger <DEBUGGER>'"
        ),
        "stderr should contain conflict error message: {stderr}"
    );
}
