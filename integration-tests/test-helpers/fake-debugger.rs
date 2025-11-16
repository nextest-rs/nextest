// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A fake debugger binary for testing debugger integration.
//!
//! This simulates what a real debugger like GDB does:
//! - Takes program and args
//! - Validates it has terminal/stdin access
//! - Prints diagnostic info to stderr
//! - Execs the actual test binary
//!
//! Unlike a real debugger, it doesn't actually debug - it just validates
//! that nextest set up the environment correctly for debugger mode.

use std::{
    env,
    process::{Command, exit},
};

fn main() {
    let args: Vec<String> = env::args().collect();
    eprintln!("[fake-debugger] invoked with {} args", args.len() - 1);
    eprintln!("[fake-debugger] args: {:?}", &args[1..]);

    // Validate we have at least one argument (the program to debug).
    if args.len() < 2 {
        eprintln!("[fake-debugger] ERROR: no program to debug");
        eprintln!("[fake-debugger] usage: fake-debugger [debugger-args] <program> [program-args]");
        exit(100);
    }

    // Find where the program starts (after debugger args).
    //
    // For this fake debugger, we'll use a simple convention:
    // - If we see "--debugger-arg", skip it and the next arg.
    // - Otherwise, the first non-flag arg is the program.
    let mut program_idx = 1;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--debugger-arg" {
            eprintln!(
                "[fake-debugger] found debugger arg: {}",
                args.get(i + 1).unwrap_or(&String::from("<missing>"))
            );
            i += 2;
            program_idx = i;
        } else if args[i].starts_with("--") {
            eprintln!("[fake-debugger] found debugger flag: {}", args[i]);
            i += 1;
            program_idx = i;
        } else {
            // Found the program.
            break;
        }
    }

    if program_idx >= args.len() {
        eprintln!("[fake-debugger] ERROR: no program found after debugger args");
        exit(101);
    }

    let program = &args[program_idx];
    let program_args = &args[program_idx + 1..];

    eprintln!("[fake-debugger] program: {}", program);
    eprintln!("[fake-debugger] program args: {:?}", program_args);

    // Check for the NEXTEST_EXECUTION_MODE environment variable to ensure that
    // we're in a nextest context.
    assert_eq!(
        std::env::var("NEXTEST_EXECUTION_MODE").as_deref(),
        Ok("process-per-test"),
        "NEXTEST_EXECUTION_MODE set to process-per-test"
    );

    // Run the test.
    eprintln!("[fake-debugger] execing: {} {:?}", program, program_args);
    match Command::new(program).args(program_args).status() {
        Ok(status) => match status.code() {
            Some(code) => {
                eprintln!("[fake-debugger] program exited with code: {}", code);
                exit(code);
            }
            None => {
                eprintln!("[fake-debugger] program terminated by signal");
                exit(102);
            }
        },
        Err(err) => {
            eprintln!(
                "[fake-debugger] failed to spawn program '{}': {}",
                program, err
            );
            exit(103);
        }
    }
}
