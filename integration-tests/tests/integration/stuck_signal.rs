// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A test that gets stuck in a signal handler.
//!
//! Meant mostly for debugging. We should also likely have a fixture which does
//! this, but it's hard to do that without pulling in extra dependencies.

#[ignore]
#[tokio::test]
async fn test_stuck_signal() {
    // This test installs a signal handler that gets stuck. Feel free to tweak
    // the loop as needed.
    loop {
        tokio::signal::ctrl_c().await.expect("received signal");
    }
}
