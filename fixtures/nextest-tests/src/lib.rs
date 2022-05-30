// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(dead_code)]

use std::path::Path;

/// This method is called by integration tests and benchmarks to ensure that NEXTEST_BIN_EXE
/// environment variables are properly set.
pub fn test_execute_bin_helper() {
    let binary_path = std::env::var("NEXTEST_BIN_EXE_nextest-tests")
        .expect("NEXTEST_BIN_EXE_nextest-tests should be present");
    assert!(
        Path::new(&binary_path).is_absolute(),
        "binary path {} is absolute",
        binary_path
    );
    let output = std::process::Command::new(&binary_path)
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
        // Check that CARGO_BIN_EXE is not set.
        assert_eq!(
            option_env!("CARGO_BIN_EXE_nextest-tests"),
            None,
            "CARGO_BIN_EXE_nextest-tests not set at compile time"
        );

        let runtime_bin_exe = std::env::var_os("NEXTEST_BIN_EXE_nextest-tests");
        assert_eq!(
            runtime_bin_exe, None,
            "NEXTEST_BIN_EXE_nextest-tests is not set at runtime"
        )
    }

    #[test]
    fn call_dylib_add_two() {
        assert_eq!(dylib_test::add(2, 2), 4);
    }
}
