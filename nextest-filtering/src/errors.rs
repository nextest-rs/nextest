// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use miette::{Diagnostic, SourceSpan};
use nom_tracable::TracableInfo;
use std::cell::RefCell;
use thiserror::Error;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FilterExpressionParseErrors {
    /// The input string.
    pub input: String,

    /// The parse errors returned.
    pub errors: Vec<ParseSingleError>,
}

impl FilterExpressionParseErrors {
    pub(crate) fn new(input: impl Into<String>, errors: Vec<ParseSingleError>) -> Self {
        Self {
            input: input.into(),
            errors,
        }
    }
}

#[derive(Clone, Debug, Error, Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseSingleError {
    #[error("invalid regex")]
    InvalidRegex(#[label("invalid regex")] SourceSpan),
    #[error("expected close regex")]
    ExpectedCloseRegex(#[label("missing '/'")] SourceSpan),
    #[error("expected matcher input")]
    ExpectedMatcherInput(#[label("missing matcher content")] SourceSpan),
    #[error("unexpected name matcher")]
    UnexpectedNameMatcher(#[label("this set doesn't take an argument")] SourceSpan),
    #[error("invalid unicode string")]
    InvalidUnicodeString(#[label("invalid unicode string")] SourceSpan),
    #[error("expected open parenthesis")]
    ExpectedOpenParenthesis(#[label("missing '('")] SourceSpan),
    #[error("expected close parenthesis")]
    ExpectedCloseParenthesis(#[label("missing ')'")] SourceSpan),
    #[error("expected filtering expression")]
    ExpectedExpr(#[label("missing expression")] SourceSpan),
    #[error("expected end of expression")]
    ExpectedEndOfExpression(#[label("unparsed input")] SourceSpan),

    #[error("unknown parsing error")]
    Unknown,
}

#[derive(Debug, Clone)]
pub(crate) struct State<'a> {
    // A `RefCell` is required here because the state must implement `Clone` to work with nom.
    errors: &'a RefCell<Vec<ParseSingleError>>,
    tracable_info: TracableInfo,
}

impl<'a> State<'a> {
    pub fn new(errors: &'a RefCell<Vec<ParseSingleError>>) -> Self {
        let tracable_info = nom_tracable::TracableInfo::new()
            .forward(true)
            .backward(true);
        Self {
            errors,
            tracable_info,
        }
    }

    pub fn report_error(&self, error: ParseSingleError) {
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

pub(crate) trait ToSourceSpan {
    fn to_span(&self) -> SourceSpan;
}
