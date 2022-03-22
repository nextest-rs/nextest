// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    // Set the target directory to be within the output directory.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is valid"));
    std::env::set_var("CARGO_TARGET_DIR", out_dir.join("target"));

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command
        .args(["build", "-p", "cdylib-example"])
        .stderr(Stdio::inherit());

    let output = command.output().expect("cargo build execution successful");
    if !output.status.success() {
        panic!("cargo build failed with status {:?}", output.status.code());
    }

    // Rather than trying to parse cargo-metadata which takes a really long time
    // to build, just assume we know where the library path is.
    for (from_name, to_name) in dylib_file_names() {
        let dylib_path = out_dir.join("target/debug/deps").join(from_name);
        eprintln!("dylib path: {}", dylib_path.display());
        std::fs::copy(&dylib_path, &out_dir.join(to_name)).unwrap_or_else(|err| {
            panic!(
                "library {} copied successfully: {}",
                dylib_path.display(),
                err
            )
        });
    }
    println!("cargo:rustc-link-lib=dylib=cdylib_example");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
}

// https://github.com/rust-lang/cargo/blob/5e09899f33744efafb99af7023acfd0a14af1c2f/tests/testsuite/build.rs#L4717-L4727
// map of (from, to)
fn dylib_file_names() -> Vec<(&'static str, &'static str)> {
    if cfg!(windows) {
        if cfg!(target_env = "msvc") {
            vec![
                ("cdylib_example.dll", "cdylib_example.dll"),
                ("cdylib_example.dll.lib", "cdylib_example.lib"),
                ("cdylib_example.dll.exp", "cdylib_example.exp"),
            ]
        } else {
            // TODO: is this correct?
            vec![
                ("cdylib_example.dll.a", "cdylib_example.dll.a"),
                ("cdylib_example.dll", "cdylib_example.dll"),
            ]
        }
    } else if cfg!(target_os = "macos") {
        vec![("libcdylib_example.dylib", "libcdylib_example.dylib")]
    } else {
        vec![("libcdylib_example.so", "libcdylib_example.so")]
    }
}
