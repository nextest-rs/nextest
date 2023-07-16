// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![warn(missing_docs)]

//! Core functionality for [cargo nextest](https://crates.io/crates/cargo-nextest). For a
//! higher-level overview, see that documentation.
//!
//! For the basic flow of operations in nextest, see [this blog
//! post](https://sunshowers.io/posts/nextest-and-tokio/).

pub mod cargo_config;
pub mod cargo_toml;
pub mod config;
#[cfg(feature = "experimental-tokio-console")]
pub mod console;
pub mod double_spawn;
pub mod errors;
mod helpers;
pub mod list;
pub mod partition;
pub mod platform;
pub mod reporter;
pub mod reuse_build;
pub mod runner;
pub mod show_config;
pub mod signal;
pub mod target_runner;
mod test_command;
pub mod test_filter;
mod time;
#[cfg(feature = "self-update")]
pub mod update;
