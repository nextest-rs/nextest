// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Logic for parsing [filter expressions](https://nexte.st/book/filter-expressions) used by
//! cargo-nextest.

mod compile;
pub mod errors;
mod expression;
mod parsing;
#[cfg(any(test, feature = "internal-testing"))]
mod proptest_helpers;

pub use expression::{BinaryQuery, FilteringExpr, FilteringSet, NameMatcher, TestQuery};
#[doc(hidden)]
pub use parsing::Expr;
