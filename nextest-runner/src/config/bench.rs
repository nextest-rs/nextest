// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Benchmark-specific configuration.

use crate::config::SlowTimeout;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct BenchConfig {
    pub(super) slow_timeout: SlowTimeout,
}
