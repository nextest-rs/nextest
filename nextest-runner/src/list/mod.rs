// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for building and querying lists of test instances and test binaries.
//!
//! The main data structures in this module are:
//! * [`TestList`] for test instances
//! * [`BinaryList`] for test binaries

mod binary_list;
mod display_filter;
mod output_format;
mod rust_build_meta;
mod test_list;

pub use binary_list::*;
pub(crate) use display_filter::*;
pub use output_format::*;
pub use rust_build_meta::*;
pub use test_list::*;

/// Typestate for [`BinaryList`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryListState {}

/// Typestate for [`TestList`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestListState {}
