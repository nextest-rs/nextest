// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

pub fn add(a: i32, b: i32) -> i32 {
    for (k, v) in std::env::vars() {
        println!("{} = {}", k, v);
    }
    a + b
}
