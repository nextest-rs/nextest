// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Benchmark-specific configuration.

use super::{
    global_timeout::GlobalTimeout,
    slow_timeout::{SlowTimeout, deserialize_slow_timeout},
};
use serde::Deserialize;

/// Benchmark-specific configuration for the default profile.
#[derive(Clone, Debug)]
pub(in crate::config) struct DefaultBenchConfig {
    /// Time after which benchmarks are considered slow, plus optional
    /// termination policy. Replaces `slow-timeout` when running
    /// `cargo nextest bench`.
    pub(in crate::config) slow_timeout: SlowTimeout,
    /// Global timeout for the entire benchmark run. Replaces `global-timeout`
    /// when running `cargo nextest bench`.
    pub(in crate::config) global_timeout: GlobalTimeout,
}

impl DefaultBenchConfig {
    /// Creates a `DefaultBenchConfig` from a `BenchConfig`.
    pub(in crate::config) fn for_default_profile(data: BenchConfig) -> Self {
        DefaultBenchConfig {
            slow_timeout: data
                .slow_timeout
                .expect("bench.slow-timeout present in default profile"),
            global_timeout: data
                .global_timeout
                .expect("bench.global-timeout present in default profile"),
        }
    }
}

/// Benchmark-specific timeout overrides used when running
/// `cargo nextest bench`.
///
/// Each field, if set, replaces its non-`bench` counterpart for benchmark
/// runs only.
#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "config-schema", schemars(deny_unknown_fields))]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct BenchConfig {
    /// Time after which benchmarks are considered slow, plus optional
    /// termination policy. Replaces `slow-timeout` when running
    /// `cargo nextest bench`.
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    pub(in crate::config) slow_timeout: Option<SlowTimeout>,
    /// Global timeout for the entire benchmark run. Replaces `global-timeout`
    /// when running `cargo nextest bench`.
    #[serde(default)]
    pub(in crate::config) global_timeout: Option<GlobalTimeout>,
}
