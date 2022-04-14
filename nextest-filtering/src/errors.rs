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

#[derive(Clone, Debug, Error, Diagnostic, PartialEq)]
#[non_exhaustive]
pub enum ParseSingleError {
    #[error("invalid regex")]
    InvalidRegex {
        #[label("{}", message)]
        span: SourceSpan,
        message: String,
    },
    /// An invalid regex was encountered but we couldn't determine a better error message.
    #[error("invalid regex")]
    InvalidRegexWithoutMessage(#[label("invalid regex")] SourceSpan),
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
