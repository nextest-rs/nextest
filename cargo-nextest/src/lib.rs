// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A new, faster test runner for Rust.
//!
//! For documentation and usage, see [the nextest site](https://nexte.st).

#![warn(missing_docs)]

mod cargo_cli;
mod dispatch;
mod errors;
mod output;

#[doc(hidden)]
pub use dispatch::*;
#[doc(hidden)]
pub use errors::*;
