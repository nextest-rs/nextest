// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! cargo-nextest is a next-generation test runner for Rust.
//!
//! For documentation and usage, see [the nextest site](https://nexte.st).
//!
//! # Installation
//!
//! To install nextest binaries (quicker), see [_Pre-built
//! binaries_](https://nexte.st/docs/installation/pre-built-binaries).
//!
//! To install from source, run:
//!
//! ```sh
//! cargo install --locked cargo-nextest
//! ```
//!
//! **The `--locked` flag is required.** Builds without `--locked` are, and will
//! remain, broken.
//!
//! # Minimum supported Rust versions
//!
//! Nextest has two minimum supported Rust versions (MSRVs): one for _building_
//! nextest itself, and one for _running tests_ with `cargo nextest run`.
//!
//! For more information about the MSRVs and the stability policy around them,
//! see [_Minimum supported Rust
//! versions_](https://nexte.st/docs/stability/#minimum-supported-rust-versions)
//! on the nextest site.

#![warn(missing_docs)]

mod cargo_cli;
mod dispatch;
#[cfg(unix)]
mod double_spawn;
mod errors;
mod helpers;
mod output;
mod reuse_build;
#[cfg(feature = "self-update")]
mod update;
mod version;

pub(crate) use dispatch::ExtractOutputFormat;
pub use dispatch::main_impl;
pub(crate) use errors::*;
pub(crate) use output::OutputWriter;
