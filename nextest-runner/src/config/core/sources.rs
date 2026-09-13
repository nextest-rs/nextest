// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration path display helpers.

use camino::Utf8Path;

/// Displays a config path relative to the workspace root if possible.
pub(crate) fn display_config_path<'a>(
    config_file: &'a Utf8Path,
    workspace_root: &Utf8Path,
) -> &'a Utf8Path {
    config_file
        .strip_prefix(workspace_root)
        .unwrap_or(config_file)
}
