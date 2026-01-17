// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for SIGTTOU handling when subprocesses grab foreground access.
//!
//! When a test spawns a subprocess that takes over the foreground process
//! group (e.g., an interactive shell), nextest becomes a background process.
//! If nextest then tries to restore terminal state via `tcsetattr`, it will
//! receive SIGTTOU and be suspended.
//!
//! The fix is to block SIGTTOU during `tcsetattr` calls.
//!
//! See <https://github.com/nextest-rs/nextest/issues/2878>.

use integration_tests::env::set_env_vars_for_test;
use std::process::Command;

/// If this test is run under nextest in an interactive terminal, spawning a
/// subprocess that grabs the foreground process would trigger SIGTTOU without
/// the fix.
#[test]
fn test_foreground_grab_does_not_suspend() {
    let env_info = set_env_vars_for_test();

    // This issue could be reproduced with zsh -ic, though not with bash -ic or
    // sh -ic (Ubuntu 24.04). But we don't want to introduce a dependency on zsh
    // in our test suite, so we have a small helper binary which simulates the
    // issue.
    let child = Command::new(&env_info.grab_foreground_bin)
        .spawn()
        .expect("spawned grab-foreground");
    let output = child
        .wait_with_output()
        .expect("waited for grab-foreground");

    assert!(
        output.status.success(),
        "grab-foreground should exit successfully: {:?}",
        output
    );
}
