// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reporting of data in a streaming, structured fashion.
//!
//! Currently, the only output supported is a compatibility layer with libtest.
//! At some point it would be worth designing a full-fidelity structured output.

mod imp;
mod libtest;

pub use imp::*;
pub use libtest::*;
