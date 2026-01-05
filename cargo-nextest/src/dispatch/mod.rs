// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command dispatch and execution.

mod clap_error;
mod cli;
mod commands;
mod execution;
mod helpers;
mod imp;

pub(crate) use clap_error::EarlyArgs;
pub(crate) use commands::ExtractOutputFormat;
pub use imp::main_impl;
