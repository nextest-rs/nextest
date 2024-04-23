// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for emulating Cargo's configuration file discovery.
//!
//! Since `cargo config get` is not stable as of Rust 1.61, nextest must do its own config file
//! search.

mod custom_platform;
mod discovery;
mod env;
mod target_triple;
#[cfg(test)]
mod test_helpers;

pub use custom_platform::*;
pub use discovery::*;
pub use env::*;
pub use target_triple::*;
