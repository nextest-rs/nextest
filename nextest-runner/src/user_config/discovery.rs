// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Discovery of user config file location.

use crate::errors::UserConfigError;
use camino::Utf8PathBuf;
use etcetera::{BaseStrategy, HomeDirError, base_strategy::Xdg};

/// Returns candidate paths for the user config file, in order of priority.
///
/// On Unix/macOS, returns the XDG path:
/// - `$XDG_CONFIG_HOME/nextest/config.toml`
/// - `~/.config/nextest/config.toml` (fallback if XDG_CONFIG_HOME unset)
///
/// On Windows, returns two candidates in order:
/// 1. Native path: `%APPDATA%\nextest\config.toml`
/// 2. XDG path: `~/.config/nextest/config.toml` (for dotfiles portability)
///
/// The caller should check each path in order and use the first one that exists.
pub fn user_config_paths() -> Result<Vec<Utf8PathBuf>, UserConfigError> {
    let mut paths = Vec::new();

    // On Windows, try native path first.
    #[cfg(windows)]
    if let Some(path) = native_config_path()? {
        paths.push(path);
    }

    // Always include XDG path (primary on Unix/macOS, fallback on Windows).
    if let Some(path) = xdg_config_path()? {
        paths.push(path);
    }

    Ok(paths)
}

/// Returns the XDG config path.
///
/// Uses `Xdg` strategy explicitly to get `~/.config/nextest/config.toml` on all
/// platforms. This is the primary path on Unix/macOS, and a fallback on Windows
/// for users who manage dotfiles across platforms.
fn xdg_config_path() -> Result<Option<Utf8PathBuf>, UserConfigError> {
    let strategy = match Xdg::new() {
        Ok(s) => s,
        Err(HomeDirError) => return Ok(None),
    };

    let config_dir = strategy.config_dir().join("nextest");
    let config_path = config_dir.join("config.toml");

    Utf8PathBuf::try_from(config_path)
        .map(Some)
        .map_err(|error| UserConfigError::NonUtf8Path { error })
}

/// Returns the native Windows config path (%APPDATA%).
#[cfg(windows)]
fn native_config_path() -> Result<Option<Utf8PathBuf>, UserConfigError> {
    use etcetera::base_strategy::Windows;

    let strategy = match Windows::new() {
        Ok(s) => s,
        Err(HomeDirError) => return Ok(None),
    };

    let config_dir = strategy.config_dir().join("nextest");
    let config_path = config_dir.join("config.toml");

    Utf8PathBuf::try_from(config_path)
        .map(Some)
        .map_err(|error| UserConfigError::NonUtf8Path { error })
}
