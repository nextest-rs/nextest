// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Report the results of a test run in human and machine-readable formats.
//!
//! The main type here is [`TestReporter`], which is constructed via a [`TestReporterBuilder`].

mod aggregator;
mod displayer;
mod error_description;
pub mod events;
mod helpers;
pub mod structured;

pub use displayer::*;
pub use error_description::*;
pub use helpers::highlight_end;
