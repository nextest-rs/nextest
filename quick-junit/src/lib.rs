// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Generate and read JUnit reports in Rust.

mod report;
mod serialize;

pub use report::*;

// Re-export `quick_xml::Result` so it can be used by downstream consumers.
pub use quick_xml::Result;
