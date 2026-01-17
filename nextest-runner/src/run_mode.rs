// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Run mode for nextest.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The run mode for nextest.
///
/// This is used to distinguish between running tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum NextestRunMode {
    /// Run tests.
    #[default]
    Test,
    /// Run benchmarks.
    Benchmark,
}

impl NextestRunMode {
    /// Returns true if this is benchmark mode.
    pub fn is_benchmark(self) -> bool {
        matches!(self, Self::Benchmark)
    }
}

impl fmt::Display for NextestRunMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Test => write!(f, "test"),
            Self::Benchmark => write!(f, "benchmark"),
        }
    }
}
