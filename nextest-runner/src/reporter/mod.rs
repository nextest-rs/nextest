// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Report the results of a test run in human and machine-readable formats.
//!
//! The main type here is [`Reporter`], which is constructed via a [`ReporterBuilder`].

mod aggregator;
mod displayer;
mod error_description;
pub mod events;
mod helpers;
mod imp;
pub mod structured;
#[cfg(test)]
pub(crate) mod test_helpers;

pub(crate) use displayer::{DisplayReporter, DisplayReporterBuilder, StatusLevels};
pub use displayer::{
    FinalStatusLevel, MaxProgressRunning, ShowProgress, ShowTerminalProgress, StatusLevel,
    TestOutputDisplay,
};
pub use error_description::*;
pub use helpers::highlight_end;
pub use imp::*;
