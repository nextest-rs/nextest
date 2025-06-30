// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A shim for rustc that is used for injecting errors.

use std::{
    env::args,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    // Currently, `--version --verbose` is supported. (This is a bit
    // overdetermined, but it's okay.)
    let args = args().collect::<Vec<_>>();

    if &args[1] == "--version" && &args[2] == "--verbose" {
        match version_verbose_error() {
            Some(VersionVerboseError::NonZeroExitCode) => {
                println!("failure output to stdout");
                eprintln!("failure output to stderr");
                return ExitCode::FAILURE;
            }
            Some(VersionVerboseError::InvalidStdout) => {
                println!("invalid output to stdout");
                eprintln!("invalid output to stderr");
                return ExitCode::SUCCESS;
            }
            Some(VersionVerboseError::InvalidTriple) => {
                println!(
                    "rustc 1.84.1 (e71f9a9a9 2025-01-27)\n\
                     binary: rustc\n\
                     commit-hash: e71f9a9a98b0faf423844bf0ba7438f29dc27d58\n\
                     commit-date: 2025-01-27\n\
                     host: invalid-triple\n\
                     release: 1.84.1\n\
                     LLVM version: 19.1.5\n\
                    "
                );
                return ExitCode::SUCCESS;
            }
            None => {
                // Pass through to the real rustc.
            }
        }
    }

    let code = Command::new(rustc_path())
        .args(&args[1..])
        .status()
        .expect("rustc executed successfully")
        .code();

    std::process::exit(code.unwrap_or(1))
}

#[derive(Clone, Copy, Debug)]
enum VersionVerboseError {
    NonZeroExitCode,
    InvalidStdout,
    InvalidTriple,
}

fn version_verbose_error() -> Option<VersionVerboseError> {
    const VAR: &str = "__NEXTEST_RUSTC_SHIM_VERSION_VERBOSE_ERROR";
    match std::env::var(VAR) {
        Ok(s) => match s.as_str() {
            "non-zero" => Some(VersionVerboseError::NonZeroExitCode),
            "invalid-stdout" => Some(VersionVerboseError::InvalidStdout),
            "invalid-triple" => Some(VersionVerboseError::InvalidTriple),
            _ => panic!("unrecognized value for {VAR}: {s}"),
        },
        Err(_) => None,
    }
}

fn rustc_path() -> String {
    const VAR: &str = "__NEXTEST_RUSTC_SHIM_RUSTC";
    match std::env::var(VAR) {
        Ok(s) => s,
        Err(_) => panic!("{VAR} not set"),
    }
}
