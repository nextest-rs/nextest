// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Allocate a large amount (1GiB) of memory.

use std::{thread, time::Duration};

fn main() {
    let _buffer = std::hint::black_box(vec![1u8; 1024 * 1024 * 1024]);

    thread::sleep(Duration::from_secs(1));
}
