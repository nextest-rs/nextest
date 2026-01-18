// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command dispatch and execution.

mod app;
mod clap_error;
mod common;
mod core;
mod helpers;
mod imp;
mod utility;

pub(crate) use clap_error::EarlyArgs;
pub use imp::main_impl;
pub(crate) use utility::ExtractOutputFormat;
