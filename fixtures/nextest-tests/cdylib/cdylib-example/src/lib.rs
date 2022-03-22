// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[no_mangle]
pub extern "C" fn multiply_two(a: i32, b: i32) -> i32 {
    a * b
}
