// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::PathBuf;

fn main() {
    // Set the target directory to be within the output directory.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is valid"));

    // The presence of this file is checked by a test.
    std::fs::write(out_dir.join("this-is-a-test-file"), "test-contents").unwrap();
}
