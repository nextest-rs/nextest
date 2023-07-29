// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A new, faster test runner for Rust.
//!
//! For documentation and usage, see [the nextest site](https://nexte.st).
//!
//! # Installation
//!
//! While you can install cargo-nextest from source, using the pre-built binaries is recommended.
//! See [Pre-built binaries](https://nexte.st/book/pre-built-binaries) on the nextest site
//! for more information.

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

#[doc(hidden)]
pub use dispatch::*;
#[doc(hidden)]
pub use errors::*;
#[doc(hidden)]
pub use output::OutputWriter;
