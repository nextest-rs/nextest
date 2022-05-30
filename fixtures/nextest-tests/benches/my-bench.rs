// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TODO: add benchmarks

#[cfg(test)]
mod tests {
    /// Test that a binary can be successfully executed.
    #[test]
    fn test_execute_bin() {
        nextest_tests::test_execute_bin_helper();
    }
}
