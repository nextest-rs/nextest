// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for building and querying lists of test instances and test binaries.
//!
//! The main data structures in this module are:
//! * [`TestList`] for test instances
//! * [`BinaryList`] for test binaries

mod binary_list;
mod test_list;
mod output_format;

pub use binary_list::*;
pub use test_list::*;
pub use output_format::*;
