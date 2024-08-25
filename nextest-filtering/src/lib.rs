// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Logic for parsing [filtersets](https://nexte.st/docs/filtersets) used by cargo-nextest.

mod compile;
pub mod errors;
mod expression;
mod parsing;
#[cfg(any(test, feature = "internal-testing"))]
mod proptest_helpers;

pub use expression::{
    BinaryQuery, CompiledExpr, EvalContext, Filterset, FiltersetKind, FiltersetLeaf, NameMatcher,
    ParseContext, TestQuery,
};
pub use parsing::ParsedExpr;
