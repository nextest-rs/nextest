// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(dead_code)]

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
    }

    #[test]
    fn call_dylib_add_two() {
        assert_eq!(dylib_test::add(2, 2), 4);
    }
}
