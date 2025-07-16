// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The run mode of nextest.
//!
//! Nextest can be in a mode where it runs either tests or benchmarks. This
//! module defines the `NextestRunMode` enum to represent these two modes.

/// The mode nextest is running in: test or benchmark.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NextestRunMode {
    /// Nextest is running in test mode.
    Test,

    /// Nextest is running in benchmark mode.
    Benchmark,
}
