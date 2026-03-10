// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(dead_code)]

use std::path::Path;

/// This function is deprecated to generate a compiler warning.
#[deprecated(
    since = "0.1.0",
    note = "this is a test warning for --cargo-message-format"
)]
pub fn deprecated_function() -> u32 {
    42
}

/// This function calls the deprecated function to trigger the warning.
pub fn trigger_warning() -> u32 {
    #[allow(deprecated)]
    deprecated_function()
}

/// This function generates a warning by calling the deprecated function
/// without suppressing the warning.
#[allow(dead_code)]
fn generate_warning_for_tests() -> u32 {
    // This call will generate a deprecation warning.
    deprecated_function()
}

/// This method is called by integration tests and benchmarks to ensure that CARGO_BIN_EXE and
/// NEXTEST_BIN_EXE environment variables are properly set.
pub fn test_execute_bin_helper() {
    let cargo_bin_exe = std::env::var("CARGO_BIN_EXE_nextest-tests")
        .expect("CARGO_BIN_EXE_nextest-tests should be present");
    let nextest_bin_exe = std::env::var("NEXTEST_BIN_EXE_nextest-tests")
        .expect("NEXTEST_BIN_EXE_nextest-tests should be present");
    let with_underscores = std::env::var("NEXTEST_BIN_EXE_nextest_tests")
        .expect("NEXTEST_BIN_EXE_nextest_tests (with underscores) should be present");
    assert_eq!(
        cargo_bin_exe, nextest_bin_exe,
        "CARGO_BIN_EXE and NEXTEST_BIN_EXE should match"
    );
    assert_eq!(nextest_bin_exe, with_underscores);
    let binary_path = &nextest_bin_exe;
    assert!(
        Path::new(binary_path).is_absolute(),
        "binary path {} is absolute",
        binary_path
    );
    let output = std::process::Command::new(binary_path)
        .output()
        .unwrap_or_else(|err| panic!("failed to run binary at {}: {}", binary_path, err));
    assert_eq!(output.status.code(), Some(42), "exit code matches");
    assert_eq!(
        &output.stdout,
        "The answer is 42\n".as_bytes(),
        "stdout matches"
    );
}

/// This is a doctest.
///
/// ```
/// assert!(true, "this always succeeds");
/// ```
fn dummy_fn() {}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_test_success() {
        assert_eq!(2 + 2, 4, "this test should succeed");
        // Check that CARGO_BIN_EXE is not set at compile time (unit tests don't get it).
        assert_eq!(
            option_env!("CARGO_BIN_EXE_nextest-tests"),
            None,
            "CARGO_BIN_EXE_nextest-tests not set at compile time"
        );

        // Neither CARGO_BIN_EXE nor NEXTEST_BIN_EXE should be set at runtime
        // for unit tests.
        assert_eq!(
            std::env::var_os("CARGO_BIN_EXE_nextest-tests"),
            None,
            "CARGO_BIN_EXE_nextest-tests is not set at runtime"
        );
        assert_eq!(
            std::env::var_os("NEXTEST_BIN_EXE_nextest-tests"),
            None,
            "NEXTEST_BIN_EXE_nextest-tests is not set at runtime"
        )
    }

    #[test]
    fn call_dylib_add_two() {
        assert_eq!(dylib_test::add(2, 2), 4);
    }
}
