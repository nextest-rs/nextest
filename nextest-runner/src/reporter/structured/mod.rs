// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reporting of data in a streaming, structured fashion.
//!
//! This module provides structured output reporters:
//!
//! - [`LibtestReporter`]: Compatibility layer with libtest JSON output.
//! - [`RecordReporter`]: Records test runs to disk for later inspection.

mod imp;
mod libtest;
mod recorder;

pub use imp::*;
pub use libtest::*;
pub use recorder::RecordReporter;
