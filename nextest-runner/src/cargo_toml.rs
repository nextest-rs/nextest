// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for reading `Cargo.toml` files.
//!
//! This package contains logic to partially read and understand `Cargo.toml` files: just enough for
//! nextest's needs.

use crate::errors::RootCargoTomlError;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

/// Represents a workspace's root Cargo.toml.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RootCargoToml {
    /// The workspace key.
    pub workspace: RootCargoTomlWorkspace,
}

impl RootCargoToml {
    /// Reads the root Cargo.toml into this file.
    pub fn read_file(path: &Utf8Path) -> Result<Self, RootCargoTomlError> {
        let contents =
            std::fs::read_to_string(&path).map_err(|error| RootCargoTomlError::ReadError {
                path: path.to_owned(),
                error,
            })?;

        toml_edit::easy::from_str(&contents).map_err(|error| RootCargoTomlError::ParseError {
            path: path.to_owned(),
            error,
        })
    }

    /// Returns true if the workspace has default members.
    pub fn has_default_members(&self) -> bool {
        self.workspace.default_members.is_some()
    }
}

/// The `[workspace]` section of a [`RootCargoToml`].
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RootCargoTomlWorkspace {
    /// The default members of the workspace.
    #[serde(default)]
    pub default_members: Option<Vec<Utf8PathBuf>>,
}
