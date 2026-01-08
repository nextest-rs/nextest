// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User-specific configuration for nextest.
//!
//! User config stores per-user preferences that shouldn't be version-controlled,
//! like UI preferences and default output settings. This is separate from the
//! repository config (`.config/nextest.toml`) which controls test execution
//! behavior.
//!
//! ## Config file location
//!
//! The user config file is searched for in the following locations:
//!
//! - **Unix/macOS**: `$XDG_CONFIG_HOME/nextest/config.toml` or
//!   `~/.config/nextest/config.toml`
//! - **Windows**: `%APPDATA%\nextest\config.toml`, with fallback to
//!   `~/.config/nextest/config.toml` for dotfiles portability
//!
//! On Windows, both locations are checked in order, and the first existing
//! config file is used. This allows users to share dotfiles across platforms.
//!
//! ## Configuration hierarchy
//!
//! Settings are resolved in the following order (highest priority first):
//!
//! 1. CLI arguments (e.g., `--show-progress=bar`)
//! 2. Environment variables (e.g., `NEXTEST_SHOW_PROGRESS=bar`)
//! 3. User overrides (first matching `[[overrides]]` for each setting)
//! 4. User base config (`[ui]` section)
//! 5. Built-in defaults

mod discovery;
mod early;
pub mod elements;
mod experimental;
mod helpers;
mod imp;

pub use discovery::*;
pub use early::*;
pub use experimental::*;
pub use imp::*;
