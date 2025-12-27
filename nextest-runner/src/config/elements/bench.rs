// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Benchmark-specific configuration.

use super::slow_timeout::{SlowTimeout, deserialize_slow_timeout};
use serde::Deserialize;

/// Benchmark-specific configuration for the default profile.
#[derive(Clone, Debug)]
pub(in crate::config) struct DefaultBenchConfig {
    /// Slow timeout for benchmarks.
    pub(in crate::config) slow_timeout: SlowTimeout,
}

impl DefaultBenchConfig {
    /// Creates a `DefaultBenchConfig` from a `BenchConfig`.
    pub(in crate::config) fn for_default_profile(data: BenchConfig) -> Self {
        DefaultBenchConfig {
            slow_timeout: data
                .slow_timeout
                .expect("bench.slow-timeout present in default profile"),
        }
    }
}

/// Benchmark-specific configuration (deserialized form with optional fields).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct BenchConfig {
    /// Slow timeout for benchmarks.
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    pub(in crate::config) slow_timeout: Option<SlowTimeout>,
}
