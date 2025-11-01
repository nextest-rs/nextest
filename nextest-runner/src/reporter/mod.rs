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

pub use displayer::{
    FinalStatusLevel, MaxProgressRunning, PROGRESS_REFRESH_RATE_HZ, ShowProgress, StatusLevel,
    TICK_INTERVAL, TestOutputDisplay,
};
pub use error_description::*;
pub use helpers::highlight_end;
pub use imp::*;
