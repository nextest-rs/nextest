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
#[cfg(feature = "experimental-tokio-console")]
pub mod console;
pub mod double_spawn;
pub mod errors;
pub mod helpers;
pub mod indenter;
pub mod input;
pub mod list;
pub mod pager;
pub mod partition;
pub mod platform;
pub mod record;
pub mod redact;
pub mod reporter;
pub mod reuse_build;
pub mod run_mode;
pub mod runner;
// TODO: move this module to the cargo-nextest crate and make it a private module once we get rid of
// the tests in nextest-runner/tests/integration which depend on this to provide correct host and
// target libdir.
mod rustc_cli;
pub mod show_config;
pub mod signal;
pub mod target_runner;
mod test_command;
pub mod test_filter;
pub mod test_output;
mod time;
#[cfg(feature = "self-update")]
pub mod update;
pub mod usdt;
pub mod user_config;
pub mod write_str;

pub use rustc_cli::RustcCli;
