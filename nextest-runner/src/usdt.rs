// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [USDT](usdt) probes for nextest.
//!
//! This module acts as documentation for USDT (Userland Statically Defined
//! Tracing) probes defined by nextest.
//!
//! USDT probes are supported on:
//!
//! * x86_64 Linux, via [bpftrace](https://bpftrace.org/) (aarch64 might work as well)
//! * macOS, illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/)
//!
//! The probes and their contents are not part of nextest's stable API.

use nextest_metadata::RustBinaryId;
use serde::Serialize;

/// Data associated with the `start_test_attempt` probe.
#[derive(Clone, Debug, Serialize)]
pub struct UsdtStartTestAttempt {
    /// The binary ID.
    pub binary_id: RustBinaryId,

    /// The name of the test.
    pub test_name: String,

    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    pub args: Vec<String>,

    /// The attempt number, starting at 1 and <= `total_attempts`.
    pub attempt: u32,

    /// The total number of attempts.
    pub total_attempts: u32,
}

#[usdt::provider(provider = "nextest")]
pub mod usdt_probes {
    use crate::usdt::*;

    pub fn start_test_attempt(attempt: &UsdtStartTestAttempt) {}
}
