// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::expression::FilteringExprKind;
use miette::{Diagnostic, SourceSpan};
use std::fmt;
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
#[derive(Clone, Debug, Error, Diagnostic, PartialEq, Eq)]
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

    /// An invalid glob pattern was encountered.
    #[error("invalid glob")]
    InvalidGlob {
        /// The part of the input that failed.
        #[label("{}", error)]
        span: SourceSpan,

        /// The underlying error.
        error: GlobConstructError,
    },

    /// A banned predicate was encountered.
    #[error("predicate not allowed in `{kind}` expressions")]
    BannedPredicate {
        /// The kind of expression.
        kind: FilteringExprKind,

        /// The span of the banned predicate.
        #[label("this predicate causes {reason}")]
        span: SourceSpan,

        /// The reason why the predicate is banned.
        reason: BannedPredicateReason,
    },

    /// An invalid regex was encountered but we couldn't determine a better error message.
    #[error("invalid regex")]
    InvalidRegexWithoutMessage(#[label("invalid regex")] SourceSpan),

    /// A regex string was not closed.
    #[error("expected close regex")]
    ExpectedCloseRegex(#[label("missing `/`")] SourceSpan),

    /// An unexpected OR operator was found.
    #[error("invalid OR operator")]
    InvalidOrOperator(#[label("expected `|`, `+`, or `or`")] SourceSpan),

    /// An unexpected AND operator was found.
    #[error("invalid AND operator")]
    InvalidAndOperator(#[label("expected `&` or `and`")] SourceSpan),

    /// An unexpected argument was found.
    #[error("unexpected argument")]
    UnexpectedArgument(#[label("this set doesn't take an argument")] SourceSpan),

    /// An unexpected comma was found.
    #[error("unexpected comma")]
    UnexpectedComma(#[label("this set doesn't take multiple arguments")] SourceSpan),

    /// An invalid string was found.
    #[error("invalid string")]
    InvalidString(#[label("invalid string")] SourceSpan),

    /// An open parenthesis `(` was expected but not found.
    #[error("expected open parenthesis")]
    ExpectedOpenParenthesis(#[label("missing `(`")] SourceSpan),

    /// A close parenthesis `)` was expected but not found.
    #[error("expected close parenthesis")]
    ExpectedCloseParenthesis(#[label("missing `)`")] SourceSpan),

    /// An invalid escape character was found.
    #[error("invalid escape character")]
    InvalidEscapeCharacter(#[label("invalid escape character")] SourceSpan),

    /// An expression was expected in this position but not found.
    #[error("expected expression")]
    ExpectedExpr(#[label("missing expression")] SourceSpan),

    /// The expression was expected to end here but some extra text was found.
    #[error("expected end of expression")]
    ExpectedEndOfExpression(#[label("unparsed input")] SourceSpan),

    /// This matcher didn't match any packages.
    #[error("operator didn't match any packages")]
    NoPackageMatch(#[label("no packages matched this")] SourceSpan),

    /// Expected "host" or "target" for a `platform()` predicate.
    #[error("invalid argument for platform")]
    InvalidPlatformArgument(#[label("expected \"target\" or \"host\"")] SourceSpan),

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

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum GlobConstructError {
    #[error("{}", .0.kind())]
    InvalidGlob(globset::Error),

    #[error("{}", .0)]
    RegexError(String),
}

#[derive(Debug)]
pub(crate) struct State<'a> {
    // A `RefCell` is required here because the state must implement `Clone` to work with nom.
    errors: &'a mut Vec<ParseSingleError>,
}

impl<'a> State<'a> {
    pub fn new(errors: &'a mut Vec<ParseSingleError>) -> Self {
        Self { errors }
    }

    pub fn report_error(&mut self, error: ParseSingleError) {
        self.errors.push(error);
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BannedPredicateReason {
    /// This predicate causes infinite recursion.
    InfiniteRecursion,
}

impl fmt::Display for BannedPredicateReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BannedPredicateReason::InfiniteRecursion => {
                write!(f, "infinite recursion")
            }
        }
    }
}
