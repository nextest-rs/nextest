// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![warn(missing_docs)]

//! Core functionality for [cargo nextest](https://crates.io/crates/cargo-nextest). For a
//! higher-level overview, see that documentation.
//!
//! For the basic flow of operations in nextest, see [this blog
//! post](https://sunshowers.io/posts/nextest-and-tokio/).

pub mod cargo_config;
pub mod config;
pub mod double_spawn;
pub mod errors;
mod helpers;
pub mod list;
pub mod partition;
pub mod platform;
pub mod reporter;
pub mod reuse_build;
pub mod runner;
pub mod signal;
mod stopwatch;
pub mod target_runner;
mod test_command;
pub mod test_filter;
#[cfg(feature = "self-update")]
pub mod update;
