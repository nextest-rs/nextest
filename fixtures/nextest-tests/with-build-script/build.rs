// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::PathBuf;

fn main() {
    // Set the target directory to be within the output directory.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is valid"));

    // The presence of this file is checked by a test.
    std::fs::write(out_dir.join("this-is-a-test-file"), "test-contents").unwrap();

    // Needed for 1.79+
    println!("cargo:rustc-check-cfg=cfg(new_format)");

    // The presence of these environment variables is checked by a test.
    if rustc_minor_version().is_some_and(|minor| minor >= 77) {
        println!("cargo::rustc-cfg=new_format");
        println!("cargo::rustc-env=BUILD_SCRIPT_NEW_FMT=new_val");
    }
    println!("cargo:rustc-env=BUILD_SCRIPT_OLD_FMT=old_val");
}

fn rustc_minor_version() -> Option<u32> {
    let rustc = std::env::var_os("RUSTC")?;
    let output = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()?;
    let version = std::str::from_utf8(&output.stdout).ok()?;
    let mut pieces = version.split('.');
    if pieces.next() != Some("rustc 1") {
        return None;
    }
    pieces.next()?.parse().ok()
}
