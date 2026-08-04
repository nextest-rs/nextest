// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Running as Buck2's test executor.
//!
//! `buck2 test` launches whatever `v2_test_executor` in the `[test]` section of
//! `.buckconfig` points at, and talks to it over gRPC. This module is that
//! program: it receives test targets from Buck2, asks how to run each one, runs
//! them with nextest, and reports results back.
//!
//! The session is collected before it is run, rather than started as targets
//! arrive. Nextest builds one test list and runs it: the list hands out borrows
//! that every event and every reporter carries, and the run's totals are fixed
//! when it starts. Adding binaries mid-run would mean reworking that ownership
//! throughout the runner, so instead the run waits until Buck2 says it has sent
//! everything. The cost is that on a large `buck2 test //...` no test starts
//! until analysis has finished.

mod imp;
mod prepare;
mod service;
mod sink;
mod transport;

pub use imp::{ExecutorOptions, exec};
pub use transport::SocketSpec;
