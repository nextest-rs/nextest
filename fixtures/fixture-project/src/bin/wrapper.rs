// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A wrapper script used to run tests.
//!
//! This script outputs information to standard error, which is then captured by
//! nextest's tests.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    eprintln!("[wrapper] args: {args:?}");

    // Expects the command line environment variable to be set.
    let _ = std::env::var("WRAPPER_CMD_ENV_VAR").expect("WRAPPER_CMD_ENV_VAR set by command.env");

    // If this is the list phase, also produce a fake test name.
    let phase = std::env::var("NEXTEST_TEST_PHASE").expect("NEXTEST_TEST_PHASE must be set");
    if phase == "list" {
        println!("fake_test_name: test");
    }

    // Execute the test binary with the arguments.
    let status = std::process::Command::new(&args[2])
        .args(&args[3..])
        .status()
        .expect("failed to execute test binary");

    std::process::exit(status.code().unwrap_or(1));
}
