// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

extern "C" {
    pub fn multiply_two(a: i32, b: i32) -> i32;
}

#[test]
fn test_multiply_two() {
    assert_eq!(unsafe { multiply_two(3, 3) }, 9);
}
