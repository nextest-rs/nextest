// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User config implementation.

use super::{discovery::user_config_paths, elements::UiConfig};
use crate::errors::UserConfigError;
use camino::Utf8Path;
use serde::Deserialize;
use tracing::debug;

/// User-specific configuration.
///
/// This configuration is loaded from the user's config directory and contains
/// personal preferences that shouldn't be version-controlled.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UserConfig {
    /// UI configuration.
    #[serde(default)]
    pub ui: UiConfig,
}

impl UserConfig {
    /// Loads user config from the default location.
    ///
    /// Tries candidate paths in order and returns the first config file found:
    /// - Unix/macOS: `~/.config/nextest/config.toml`
    /// - Windows: `%APPDATA%\nextest\config.toml`, then `~/.config/nextest/config.toml`
    ///
    /// Returns `Ok(None)` if no config file exists at any candidate path.
    /// Returns `Err` if a config file exists but is invalid.
    pub fn from_default_location() -> Result<Option<Self>, UserConfigError> {
        let paths = user_config_paths()?;
        if paths.is_empty() {
            debug!("user config: could not determine config directory");
            return Ok(None);
        }

        for path in &paths {
            match Self::from_path(path)? {
                Some(config) => return Ok(Some(config)),
                None => continue,
            }
        }

        debug!(
            "user config: no config file found at any candidate path: {:?}",
            paths
        );
        Ok(None)
    }

    /// Loads user config from a specific path.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be read or parsed.
    pub fn from_path(path: &Utf8Path) -> Result<Option<Self>, UserConfigError> {
        debug!("user config: attempting to load from {path}");
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("user config: file does not exist at {path}");
                return Ok(None);
            }
            Err(error) => {
                return Err(UserConfigError::Read {
                    path: path.to_owned(),
                    error,
                });
            }
        };

        let config: UserConfig =
            toml::from_str(&contents).map_err(|error| UserConfigError::Parse {
                path: path.to_owned(),
                error,
            })?;

        debug!("user config: loaded successfully from {path}");
        Ok(Some(config))
    }
}
