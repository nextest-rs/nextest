// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use miette::{Diagnostic, SourceSpan};
use nom_tracable::TracableInfo;
use std::cell::RefCell;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic, PartialEq, Eq)]
pub enum Error {
    #[error("Invalid regex")]
    InvalidRegex(#[label("Invalid regex")] SourceSpan),
    #[error("Expected close regex")]
    ExpectedCloseRegex(#[label("Missing '/'")] SourceSpan),
    #[error("Expected matcher input")]
    ExpectedMatcherInput(#[label("Missing matcher content")] SourceSpan),
    #[error("Unexpected name matcher")]
    UnexpectedNameMatcher(#[label("This set doesn't take en argument")] SourceSpan),
    #[error("Invalid unicode string")]
    InvalidUnicodeString(#[label("This is not a valid unicode string")] SourceSpan),
    #[error("Expected open parentheses")]
    ExpectedOpenParenthesis(#[label("Missing '('")] SourceSpan),
    #[error("Expected close parentheses")]
    ExpectedCloseParenthesis(#[label("Missing ')'")] SourceSpan),
    #[error("Expected filtering expression")]
    ExpectedExpr(#[label("Missing expression")] SourceSpan),
    #[error("Expected end of expression")]
    ExpectedEndOfExpression(#[label("Unparsed input")] SourceSpan),

    #[error("Unknown parsing error")]
    Unknown,
}

#[derive(Debug, Clone)]
pub struct State<'a> {
    // A `RefCell` is required here because the state must implement `Clone` to work with nom.
    errors: &'a RefCell<Vec<Error>>,
    tracable_info: TracableInfo,
}

impl<'a> State<'a> {
    pub fn new(errors: &'a RefCell<Vec<Error>>) -> Self {
        let tracable_info = nom_tracable::TracableInfo::new()
            .forward(true)
            .backward(true);
        Self {
            errors,
            tracable_info,
        }
    }

    pub fn report_error(&self, error: Error) {
        self.errors.borrow_mut().push(error);
    }
}

impl<'a> nom_tracable::HasTracableInfo for State<'a> {
    fn get_tracable_info(&self) -> TracableInfo {
        self.tracable_info.get_tracable_info()
    }

    fn set_tracable_info(mut self, info: TracableInfo) -> Self {
        self.tracable_info = self.tracable_info.set_tracable_info(info);
        self
    }
}

#[derive(Debug)]
pub struct FilteringExprParsingError(pub String);

impl std::fmt::Display for FilteringExprParsingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Failed to parse the filtering expression \"{}\"", self.0)
    }
}

impl std::error::Error for FilteringExprParsingError {}

pub(crate) trait ToSourceSpan {
    fn to_span(&self) -> SourceSpan;
}
