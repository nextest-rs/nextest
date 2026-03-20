// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[test]
fn test_segfault() {
    println!("going to perform a segfault shortly");
    let p: *mut i32 = std::ptr::null::<i32>() as *mut i32;
    unsafe { *p = 42 };
}
