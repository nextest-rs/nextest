// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The test runner.
//!
//! The main structure in this module is [`TestRunner`].

mod imp;

#[cfg(unix)]
#[path = "unix.rs"]
mod os;

#[cfg(windows)]
#[path = "windows.rs"]
mod os;

pub use imp::*;
