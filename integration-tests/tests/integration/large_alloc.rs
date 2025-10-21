// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[test]
#[ignore]
fn large_alloc() {
    // Allocate 1GiB of memory within this process.
    let _buffer = std::hint::black_box(vec![1u8; 1024 * 1024 * 1024]);

    // Create a subprocess which allocates another 1GiB of memory.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_large-alloc"))
        .spawn()
        .expect("spawned child process");

    child.wait().expect("waited for child process");

    // Create that subprocess again.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_large-alloc"))
        .spawn()
        .expect("spawned child process");

    child.wait().expect("waited for child process");
}
