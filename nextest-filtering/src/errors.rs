// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use miette::{Diagnostic, SourceSpan};
use nom_tracable::TracableInfo;
use std::cell::RefCell;
use thiserror::Error;

/// A set of errors that occurred while parsing a filter expression.
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

/// An individual error that occurred while parsing a filter expression.
#[derive(Clone, Debug, Error, Diagnostic, PartialEq)]
#[non_exhaustive]
pub enum ParseSingleError {
    /// An invalid regex was encountered.
    #[error("invalid regex")]
    InvalidRegex {
        /// The part of the input that failed.
        #[label("{}", message)]
        span: SourceSpan,

        /// A message indicating the failure.
        message: String,
    },

    /// An invalid regex was encountered but we couldn't determine a better error message.
    #[error("invalid regex")]
    InvalidRegexWithoutMessage(#[label("invalid regex")] SourceSpan),

    /// A regex string was not closed.
    #[error("expected close regex")]
    ExpectedCloseRegex(#[label("missing `/`")] SourceSpan),

    /// An unexpected argument was found.
    #[error("unexpected argument")]
    UnexpectedArgument(#[label("this set doesn't take an argument")] SourceSpan),

    /// An invalid string was found.
    #[error("invalid string")]
    InvalidString(#[label("invalid string")] SourceSpan),

    /// An open parenthesis `(` was expected but not found.
    #[error("expected open parenthesis")]
    ExpectedOpenParenthesis(#[label("missing `(`")] SourceSpan),

    /// A close parenthesis `)` was expected but not found.
    #[error("expected close parenthesis")]
    ExpectedCloseParenthesis(#[label("missing `)`")] SourceSpan),

    /// An expression was expected in this position but not found.
    #[error("expected expression")]
    ExpectedExpr(#[label("missing expression")] SourceSpan),

    /// The expression was expected to end here but some extra text was found.
    #[error("expected end of expression")]
    ExpectedEndOfExpression(#[label("unparsed input")] SourceSpan),

    /// An unknown parsing error occurred.
    #[error("unknown parsing error")]
    Unknown,
}

impl ParseSingleError {
    pub(crate) fn invalid_regex(input: &str, start: usize, end: usize) -> Self {
        // Use regex-syntax to parse the input so that we get better error messages.
        match regex_syntax::Parser::new().parse(input) {
            Ok(_) => {
                // It is weird that a regex failed to parse with regex but succeeded with
                // regex-syntax, but we can't do better.
                Self::InvalidRegexWithoutMessage((start, end - start).into())
            }
            Err(err) => {
                let (message, span) = match &err {
                    regex_syntax::Error::Parse(err) => (format!("{}", err.kind()), err.span()),
                    regex_syntax::Error::Translate(err) => (format!("{}", err.kind()), err.span()),
                    _ => return Self::InvalidRegexWithoutMessage((start, end - start).into()),
                };

                // This isn't perfect because it doesn't account for "\/", but it'll do for now.
                let err_start = start + span.start.offset;
                let err_end = start + span.end.offset;

                Self::InvalidRegex {
                    span: (err_start, err_end - err_start).into(),
                    message,
                }
            }
        }
    }
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
