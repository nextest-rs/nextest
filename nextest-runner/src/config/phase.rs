// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Settings for the runtime environment.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct DeserializedPhase {
    pub(crate) run: DeserializedPhaseSettings,
}

/// Configuration for the test runtime environment.
///
/// In the future, this will support target runners and other similar settings.
///
/// Overrides are per-setting, not for the entire environment. TODO: ensure this in tests.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct DeserializedPhaseSettings {
    pub(super) extra_args: Option<Vec<String>>,
}
