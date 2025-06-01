// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parsing for filtersets.
//!
//! The parsing strategy is based on the following blog post:
//! `<https://eyalkalderon.com/blog/nom-error-recovery/>`
//!
//! All high level parsing functions should:
//! - always return Ok(_)
//! - on error:
//!     - consume as much input as it makes sense so that we can try to resume parsing
//!     - return an error/none variant of the expected result type
//!     - push an error in the parsing state (in span.state)

use guppy::graph::cargo::BuildPlatform;
use miette::SourceSpan;
use recursion::CollapsibleExt;
use std::fmt;
use winnow::{
    LocatingSlice, ModalParser, Parser,
    ascii::line_ending,
    combinator::{alt, delimited, eof, peek, preceded, repeat, terminated, trace},
    stream::{Location, SliceLen, Stream},
    token::{literal, take_till},
};

mod glob;
mod unicode_string;
use crate::{
    NameMatcher,
    errors::*,
    expression::{ExprFrame, Wrapped},
};
pub(crate) use glob::GenericGlob;
pub(crate) use unicode_string::DisplayParsedString;

pub(crate) type Span<'a> = winnow::Stateful<LocatingSlice<&'a str>, State<'a>>;
type Error = ();
type PResult<T> = winnow::ModalResult<T, Error>;

pub(crate) fn new_span<'a>(input: &'a str, errors: &'a mut Vec<ParseSingleError>) -> Span<'a> {
    Span {
        input: LocatingSlice::new(input),
        state: State::new(errors),
    }
}

/// A filterset that has been parsed but not yet compiled against a package
/// graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedLeaf<S = SourceSpan> {
    Package(NameMatcher, S),
    Deps(NameMatcher, S),
    Rdeps(NameMatcher, S),
    Kind(NameMatcher, S),
    Binary(NameMatcher, S),
    BinaryId(NameMatcher, S),
    Platform(BuildPlatform, S),
    Test(NameMatcher, S),
    Default(S),
    All,
    None,
}

impl ParsedLeaf {
    /// Returns true if this leaf can only be evaluated at runtime, i.e. it
    /// requires test names to be available.
    ///
    /// Currently, this also returns true (conservatively) for the `Default`
    /// leaf, which is used to represent the default set of tests to run.
    pub fn is_runtime_only(&self) -> bool {
        matches!(self, Self::Test(_, _) | Self::Default(_))
    }

    #[cfg(test)]
    fn drop_source_span(self) -> ParsedLeaf<()> {
        match self {
            Self::Package(matcher, _) => ParsedLeaf::Package(matcher, ()),
            Self::Deps(matcher, _) => ParsedLeaf::Deps(matcher, ()),
            Self::Rdeps(matcher, _) => ParsedLeaf::Rdeps(matcher, ()),
            Self::Kind(matcher, _) => ParsedLeaf::Kind(matcher, ()),
            Self::Binary(matcher, _) => ParsedLeaf::Binary(matcher, ()),
            Self::BinaryId(matcher, _) => ParsedLeaf::BinaryId(matcher, ()),
            Self::Platform(platform, _) => ParsedLeaf::Platform(platform, ()),
            Self::Test(matcher, _) => ParsedLeaf::Test(matcher, ()),
            Self::Default(_) => ParsedLeaf::Default(()),
            Self::All => ParsedLeaf::All,
            Self::None => ParsedLeaf::None,
        }
    }
}

impl<S> fmt::Display for ParsedLeaf<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(matcher, _) => write!(f, "package({matcher})"),
            Self::Deps(matcher, _) => write!(f, "deps({matcher})"),
            Self::Rdeps(matcher, _) => write!(f, "rdeps({matcher})"),
            Self::Kind(matcher, _) => write!(f, "kind({matcher})"),
            Self::Binary(matcher, _) => write!(f, "binary({matcher})"),
            Self::BinaryId(matcher, _) => write!(f, "binary_id({matcher})"),
            Self::Platform(platform, _) => write!(f, "platform({platform})"),
            Self::Test(matcher, _) => write!(f, "test({matcher})"),
            Self::Default(_) => write!(f, "default()"),
            Self::All => write!(f, "all()"),
            Self::None => write!(f, "none()"),
        }
    }
}

/// A filterset that hasn't been compiled against a package graph.
///
/// XXX: explain why `S` is required (for equality checking w/tests), or replace it with its own
/// structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedExpr<S = SourceSpan> {
    Not(NotOperator, Box<ParsedExpr<S>>),
    Union(OrOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Intersection(AndOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Difference(DifferenceOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Parens(Box<ParsedExpr<S>>),
    Set(ParsedLeaf<S>),
}

impl ParsedExpr {
    pub fn parse(input: &str) -> Result<Self, Vec<ParseSingleError>> {
        let mut errors = Vec::new();
        let span = new_span(input, &mut errors);
        match parse(span).unwrap() {
            ExprResult::Valid(expr) => Ok(expr),
            ExprResult::Error => Err(errors),
        }
    }

    /// Returns runtime-only leaves, i.e. ones that require test names to be
    /// available.
    pub fn runtime_only_leaves(&self) -> Vec<&ParsedLeaf> {
        use ExprFrame::*;

        let mut leaves = Vec::new();
        Wrapped(self).collapse_frames(|layer: ExprFrame<&ParsedLeaf, ()>| match layer {
            Set(leaf) => {
                if leaf.is_runtime_only() {
                    leaves.push(leaf);
                }
            }
            Not(_) | Union(_, _) | Intersection(_, _) | Difference(_, _) | Parens(_) => (),
        });

        leaves
    }

    fn boxed(self) -> Box<Self> {
        Box::new(self)
    }

    fn not(self, op: NotOperator) -> Self {
        ParsedExpr::Not(op, self.boxed())
    }

    fn union(op: OrOperator, expr_1: Self, expr_2: Self) -> Self {
        ParsedExpr::Union(op, expr_1.boxed(), expr_2.boxed())
    }

    fn intersection(op: AndOperator, expr_1: Self, expr_2: Self) -> Self {
        ParsedExpr::Intersection(op, expr_1.boxed(), expr_2.boxed())
    }

    fn difference(op: DifferenceOperator, expr_1: Self, expr_2: Self) -> Self {
        ParsedExpr::Difference(op, expr_1.boxed(), expr_2.boxed())
    }

    fn parens(self) -> Self {
        ParsedExpr::Parens(self.boxed())
    }

    #[cfg(test)]
    fn all() -> ParsedExpr {
        ParsedExpr::Set(ParsedLeaf::All)
    }

    #[cfg(test)]
    fn none() -> ParsedExpr {
        ParsedExpr::Set(ParsedLeaf::None)
    }

    #[cfg(test)]
    fn drop_source_span(self) -> ParsedExpr<()> {
        match self {
            Self::Not(op, expr) => ParsedExpr::Not(op, Box::new(expr.drop_source_span())),
            Self::Union(op, a, b) => ParsedExpr::Union(
                op,
                Box::new(a.drop_source_span()),
                Box::new(b.drop_source_span()),
            ),
            Self::Intersection(op, a, b) => ParsedExpr::Intersection(
                op,
                Box::new(a.drop_source_span()),
                Box::new(b.drop_source_span()),
            ),
            Self::Difference(op, a, b) => ParsedExpr::Difference(
                op,
                Box::new(a.drop_source_span()),
                Box::new(b.drop_source_span()),
            ),
            Self::Parens(a) => ParsedExpr::Parens(Box::new(a.drop_source_span())),
            Self::Set(set) => ParsedExpr::Set(set.drop_source_span()),
        }
    }
}

impl<S> fmt::Display for ParsedExpr<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Not(op, expr) => write!(f, "{op} {expr}"),
            Self::Union(op, expr_1, expr_2) => write!(f, "{expr_1} {op} {expr_2}"),
            Self::Intersection(op, expr_1, expr_2) => write!(f, "{expr_1} {op} {expr_2}"),
            Self::Difference(op, expr_1, expr_2) => write!(f, "{expr_1} {op} {expr_2}"),
            Self::Parens(expr) => write!(f, "({expr})"),
            Self::Set(set) => write!(f, "{set}"),
        }
    }
}

pub(crate) enum ExprResult {
    Valid(ParsedExpr),
    Error,
}

impl ExprResult {
    fn combine(self, op: impl FnOnce(ParsedExpr, ParsedExpr) -> ParsedExpr, other: Self) -> Self {
        match (self, other) {
            (Self::Valid(expr_1), Self::Valid(expr_2)) => Self::Valid(op(expr_1, expr_2)),
            _ => Self::Error,
        }
    }

    fn negate(self, op: NotOperator) -> Self {
        match self {
            Self::Valid(expr) => Self::Valid(expr.not(op)),
            _ => Self::Error,
        }
    }

    fn parens(self) -> Self {
        match self {
            Self::Valid(expr) => Self::Valid(expr.parens()),
            _ => Self::Error,
        }
    }
}

enum SpanLength {
    Unknown,
    Exact(usize),
    Offset(isize, usize),
}

fn expect_inner<'a, F, T>(
    mut parser: F,
    make_err: fn(SourceSpan) -> ParseSingleError,
    limit: SpanLength,
) -> impl ModalParser<Span<'a>, Option<T>, Error>
where
    F: ModalParser<Span<'a>, T, Error>,
{
    move |input: &mut _| match parser.parse_next(input) {
        Ok(out) => Ok(Some(out)),
        Err(winnow::error::ErrMode::Backtrack(_)) | Err(winnow::error::ErrMode::Cut(_)) => {
            let fragment_start = input.current_token_start();
            let fragment_length = input.slice_len();
            let span = match limit {
                SpanLength::Unknown => (fragment_start, fragment_length).into(),
                SpanLength::Exact(x) => (fragment_start, x.min(fragment_length)).into(),
                SpanLength::Offset(offset, x) => {
                    // e.g. fragment_start = 5, fragment_length = 2, offset = -1, x = 3.
                    // Here, start = 4.
                    let effective_start = fragment_start.saturating_add_signed(offset);
                    // end = 6.
                    let effective_end = effective_start + fragment_length;
                    // len = min(3, 6 - 4) = 2.
                    let len = (effective_end - effective_start).min(x);
                    (effective_start, len).into()
                }
            };
            let err = make_err(span);
            input.state.report_error(err);
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn expect<'a, F, T>(
    parser: F,
    make_err: fn(SourceSpan) -> ParseSingleError,
) -> impl ModalParser<Span<'a>, Option<T>, Error>
where
    F: ModalParser<Span<'a>, T, Error>,
{
    expect_inner(parser, make_err, SpanLength::Unknown)
}

fn expect_n<'a, F, T>(
    parser: F,
    make_err: fn(SourceSpan) -> ParseSingleError,
    limit: SpanLength,
) -> impl ModalParser<Span<'a>, Option<T>, Error>
where
    F: ModalParser<Span<'a>, T, Error>,
{
    expect_inner(parser, make_err, limit)
}

fn expect_char<'a>(
    c: char,
    make_err: fn(SourceSpan) -> ParseSingleError,
) -> impl ModalParser<Span<'a>, Option<char>, Error> {
    expect_inner(ws(c), make_err, SpanLength::Exact(0))
}

fn silent_expect<'a, F, T>(mut parser: F) -> impl ModalParser<Span<'a>, Option<T>, Error>
where
    F: ModalParser<Span<'a>, T, Error>,
{
    move |input: &mut _| match parser.parse_next(input) {
        Ok(out) => Ok(Some(out)),
        Err(winnow::error::ErrMode::Backtrack(_)) | Err(winnow::error::ErrMode::Cut(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

fn ws<'a, T, P: ModalParser<Span<'a>, T, Error>>(
    mut inner: P,
) -> impl ModalParser<Span<'a>, T, Error> {
    move |input: &mut Span<'a>| {
        let start = input.checkpoint();
        () = repeat(
            0..,
            alt((
                // Match individual space characters.
                ' '.void(),
                // Match CRLF and LF line endings. This allows filters to be specified as multiline TOML
                // strings.
                line_ending.void(),
            )),
        )
        .parse_next(input)?;
        match inner.parse_next(input) {
            Ok(res) => Ok(res),
            Err(winnow::error::ErrMode::Backtrack(err)) => {
                input.reset(&start);
                Err(winnow::error::ErrMode::Backtrack(err))
            }
            Err(winnow::error::ErrMode::Cut(err)) => {
                input.reset(&start);
                Err(winnow::error::ErrMode::Cut(err))
            }
            Err(err) => Err(err),
        }
    }
}

// This parse will never fail
fn parse_matcher_text<'i>(input: &mut Span<'i>) -> PResult<Option<String>> {
    trace("parse_matcher_text", |input: &mut Span<'i>| {
        let res = match expect(
            unicode_string::parse_string,
            ParseSingleError::InvalidString,
        )
        .parse_next(input)
        {
            Ok(res) => res.flatten(),
            Err(_) => unreachable!(),
        };

        if res.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            let start = input.current_token_start();
            input
                .state
                .report_error(ParseSingleError::InvalidString((start..0).into()));
        }

        Ok(res)
    })
    .parse_next(input)
}

fn parse_contains_matcher(input: &mut Span<'_>) -> PResult<Option<NameMatcher>> {
    trace(
        "parse_contains_matcher",
        preceded('~', parse_matcher_text).map(|res: Option<String>| {
            res.map(|value| NameMatcher::Contains {
                value,
                implicit: false,
            })
        }),
    )
    .parse_next(input)
}

fn parse_equal_matcher(input: &mut Span<'_>) -> PResult<Option<NameMatcher>> {
    trace(
        "parse_equal_matcher",
        ws(
            preceded('=', parse_matcher_text).map(|res: Option<String>| {
                res.map(|value| NameMatcher::Equal {
                    value,
                    implicit: false,
                })
            }),
        ),
    )
    .parse_next(input)
}

fn parse_regex_inner(input: &mut Span<'_>) -> PResult<String> {
    trace("parse_regex_inner", |input: &mut _| {
        enum Frag<'a> {
            Literal(&'a str),
            Escape(char),
        }

        let parse_escape = alt((r"\/".value('/'), '\\')).map(Frag::Escape);
        let parse_literal = take_till(1.., ('\\', '/'))
            .verify(|s: &str| !s.is_empty())
            .map(|s: &str| Frag::Literal(s));
        let parse_frag = alt((parse_escape, parse_literal));

        let res = repeat(0.., parse_frag)
            .fold(String::new, |mut string, frag| {
                match frag {
                    Frag::Escape(c) => string.push(c),
                    Frag::Literal(s) => string.push_str(s),
                }
                string
            })
            .parse_next(input)?;

        let _ = peek('/').parse_next(input)?;

        Ok(res)
    })
    .parse_next(input)
}

// This should match parse_regex_inner above.
pub(crate) struct DisplayParsedRegex<'a>(pub(crate) &'a regex::Regex);

impl fmt::Display for DisplayParsedRegex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let regex = self.0.as_str();
        let mut escaped = false;
        for c in regex.chars() {
            if escaped {
                escaped = false;
                write!(f, "{c}")?;
            } else if c == '\\' {
                escaped = true;
                write!(f, "{c}")?;
            } else if c == '/' {
                // '/' is the only additional escape.
                write!(f, "\\/")?;
            } else {
                write!(f, "{c}")?;
            }
        }
        Ok(())
    }
}

fn parse_regex<'i>(input: &mut Span<'i>) -> PResult<Option<NameMatcher>> {
    trace("parse_regex", |input: &mut Span<'i>| {
        let start = input.checkpoint();
        let res = match parse_regex_inner.parse_next(input) {
            Ok(res) => res,
            Err(_) => {
                input.reset(&start);
                match take_till::<_, _, Error>(0.., ')').parse_next(input) {
                    Ok(_) => {
                        let start = input.current_token_start();
                        let err = ParseSingleError::ExpectedCloseRegex((start, 0).into());
                        input.state.report_error(err);
                        return Ok(None);
                    }
                    Err(_) => unreachable!(),
                }
            }
        };
        match regex::Regex::new(&res).map(NameMatcher::Regex) {
            Ok(res) => Ok(Some(res)),
            Err(_) => {
                let end = input.checkpoint();

                input.reset(&start);
                let start = input.current_token_start();

                input.reset(&end);
                let end = input.current_token_start();

                let err = ParseSingleError::invalid_regex(&res, start, end);
                input.state.report_error(err);
                Ok(None)
            }
        }
    })
    .parse_next(input)
}

fn parse_regex_matcher(input: &mut Span<'_>) -> PResult<Option<NameMatcher>> {
    trace(
        "parse_regex_matcher",
        ws(delimited('/', parse_regex, silent_expect(ws('/')))),
    )
    .parse_next(input)
}

fn parse_glob_matcher(input: &mut Span<'_>) -> PResult<Option<NameMatcher>> {
    trace(
        "parse_glob_matcher",
        ws(preceded('#', glob::parse_glob(false))),
    )
    .parse_next(input)
}

// This parse will never fail (because default_matcher won't)
fn set_matcher<'a>(
    default_matcher: DefaultMatcher,
) -> impl ModalParser<Span<'a>, Option<NameMatcher>, Error> {
    ws(alt((
        parse_regex_matcher,
        parse_glob_matcher,
        parse_equal_matcher,
        parse_contains_matcher,
        default_matcher.into_parser(),
    )))
}

fn recover_unexpected_comma<'i>(input: &mut Span<'i>) -> PResult<()> {
    trace("recover_unexpected_comma", |input: &mut Span<'i>| {
        let start = input.checkpoint();
        match peek(ws(',')).parse_next(input) {
            Ok(_) => {
                let pos = input.current_token_start();
                input
                    .state
                    .report_error(ParseSingleError::UnexpectedComma((pos..0).into()));
                match take_till::<_, _, Error>(0.., ')').parse_next(input) {
                    Ok(_) => Ok(()),
                    Err(_) => unreachable!(),
                }
            }
            Err(_) => {
                input.reset(&start);
                Ok(())
            }
        }
    })
    .parse_next(input)
}

fn nullary_set_def<'a>(
    name: &'static str,
    make_set: fn(SourceSpan) -> ParsedLeaf,
) -> impl ModalParser<Span<'a>, Option<ParsedLeaf>, Error> {
    move |i: &mut Span<'_>| {
        let start = i.current_token_start();
        let _ = literal(name).parse_next(i)?;
        let _ = expect_char('(', ParseSingleError::ExpectedOpenParenthesis).parse_next(i)?;
        let err_loc = i.current_token_start();
        match take_till::<_, _, Error>(0.., ')').parse_next(i) {
            Ok(res) => {
                if !res.trim().is_empty() {
                    let span = (err_loc, res.len()).into();
                    let err = ParseSingleError::UnexpectedArgument(span);
                    i.state.report_error(err);
                }
            }
            Err(_) => unreachable!(),
        };
        let _ = expect_char(')', ParseSingleError::ExpectedCloseParenthesis).parse_next(i)?;
        let end = i.current_token_start();
        Ok(Some(make_set((start, end - start).into())))
    }
}

#[derive(Copy, Clone, Debug)]
enum DefaultMatcher {
    // Equal is no longer used and glob is always favored.
    Equal,
    Contains,
    Glob,
}

impl DefaultMatcher {
    fn into_parser<'a>(self) -> impl ModalParser<Span<'a>, Option<NameMatcher>, Error> {
        move |input: &mut _| match self {
            Self::Equal => parse_matcher_text
                .map(|res: Option<String>| res.map(NameMatcher::implicit_equal))
                .parse_next(input),
            Self::Contains => parse_matcher_text
                .map(|res: Option<String>| res.map(NameMatcher::implicit_contains))
                .parse_next(input),
            Self::Glob => glob::parse_glob(true).parse_next(input),
        }
    }
}

fn unary_set_def<'a>(
    name: &'static str,
    default_matcher: DefaultMatcher,
    make_set: fn(NameMatcher, SourceSpan) -> ParsedLeaf,
) -> impl ModalParser<Span<'a>, Option<ParsedLeaf>, Error> {
    move |i: &mut _| {
        let _ = literal(name).parse_next(i)?;
        let _ = expect_char('(', ParseSingleError::ExpectedOpenParenthesis).parse_next(i)?;
        let start = i.current_token_start();
        let res = set_matcher(default_matcher).parse_next(i)?;
        let end = i.current_token_start();
        recover_unexpected_comma.parse_next(i)?;
        let _ = expect_char(')', ParseSingleError::ExpectedCloseParenthesis).parse_next(i)?;
        Ok(res.map(|matcher| make_set(matcher, (start, end - start).into())))
    }
}

fn platform_def(i: &mut Span<'_>) -> PResult<Option<ParsedLeaf>> {
    let _ = "platform".parse_next(i)?;
    let _ = expect_char('(', ParseSingleError::ExpectedOpenParenthesis).parse_next(i)?;
    let start = i.current_token_start();
    // Try parsing the argument as a string for better error messages.
    let res = ws(parse_matcher_text).parse_next(i)?;
    let end = i.current_token_start();
    recover_unexpected_comma.parse_next(i)?;
    let _ = expect_char(')', ParseSingleError::ExpectedCloseParenthesis).parse_next(i)?;

    // The returned string will include leading and trailing whitespace.
    let platform = match res.as_deref().map(|res| res.trim()) {
        Some("host") => Some(BuildPlatform::Host),
        Some("target") => Some(BuildPlatform::Target),
        Some(_) => {
            i.state
                .report_error(ParseSingleError::InvalidPlatformArgument(
                    (start, end - start).into(),
                ));
            None
        }
        None => {
            // This was already reported above.
            None
        }
    };
    Ok(platform.map(|platform| ParsedLeaf::Platform(platform, (start, end - start).into())))
}

fn parse_set_def(input: &mut Span<'_>) -> PResult<Option<ParsedLeaf>> {
    trace(
        "parse_set_def",
        ws(alt((
            unary_set_def("package", DefaultMatcher::Glob, ParsedLeaf::Package),
            unary_set_def("deps", DefaultMatcher::Glob, ParsedLeaf::Deps),
            unary_set_def("rdeps", DefaultMatcher::Glob, ParsedLeaf::Rdeps),
            unary_set_def("kind", DefaultMatcher::Equal, ParsedLeaf::Kind),
            // binary_id must go above binary, otherwise we'll parse the opening predicate wrong.
            unary_set_def("binary_id", DefaultMatcher::Glob, ParsedLeaf::BinaryId),
            unary_set_def("binary", DefaultMatcher::Glob, ParsedLeaf::Binary),
            unary_set_def("test", DefaultMatcher::Contains, ParsedLeaf::Test),
            platform_def,
            nullary_set_def("default", ParsedLeaf::Default),
            nullary_set_def("all", |_| ParsedLeaf::All),
            nullary_set_def("none", |_| ParsedLeaf::None),
        ))),
    )
    .parse_next(input)
}

fn expect_expr<'a, P: ModalParser<Span<'a>, ExprResult, Error>>(
    inner: P,
) -> impl ModalParser<Span<'a>, ExprResult, Error> {
    expect(inner, ParseSingleError::ExpectedExpr).map(|res| res.unwrap_or(ExprResult::Error))
}

fn parse_parentheses_expr(input: &mut Span<'_>) -> PResult<ExprResult> {
    trace(
        "parse_parentheses_expr",
        delimited(
            '(',
            expect_expr(parse_expr),
            expect_char(')', ParseSingleError::ExpectedCloseParenthesis),
        )
        .map(|expr| expr.parens()),
    )
    .parse_next(input)
}

fn parse_basic_expr(input: &mut Span<'_>) -> PResult<ExprResult> {
    trace(
        "parse_basic_expr",
        ws(alt((
            parse_set_def.map(|set| {
                set.map(|set| ExprResult::Valid(ParsedExpr::Set(set)))
                    .unwrap_or(ExprResult::Error)
            }),
            parse_expr_not,
            parse_parentheses_expr,
        ))),
    )
    .parse_next(input)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    any(test, feature = "internal-testing"),
    derive(test_strategy::Arbitrary)
)]
pub enum NotOperator {
    LiteralNot,
    Exclamation,
}

impl fmt::Display for NotOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotOperator::LiteralNot => f.write_str("not"),
            NotOperator::Exclamation => f.write_str("!"),
        }
    }
}

fn parse_expr_not(input: &mut Span<'_>) -> PResult<ExprResult> {
    trace(
        "parse_expr_not",
        (
            alt((
                "not ".value(NotOperator::LiteralNot),
                '!'.value(NotOperator::Exclamation),
            )),
            expect_expr(ws(parse_basic_expr)),
        )
            .map(|(op, expr)| expr.negate(op)),
    )
    .parse_next(input)
}

// ---

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    any(test, feature = "internal-testing"),
    derive(test_strategy::Arbitrary)
)]
pub enum OrOperator {
    LiteralOr,
    Pipe,
    Plus,
}

impl fmt::Display for OrOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrOperator::LiteralOr => f.write_str("or"),
            OrOperator::Pipe => f.write_str("|"),
            OrOperator::Plus => f.write_str("+"),
        }
    }
}

fn parse_expr(input: &mut Span<'_>) -> PResult<ExprResult> {
    trace("parse_expr", |input: &mut _| {
        // "or" binds less tightly than "and", so parse and within or.
        let expr = expect_expr(parse_and_or_difference_expr).parse_next(input)?;

        let ops = repeat(
            0..,
            (parse_or_operator, expect_expr(parse_and_or_difference_expr)),
        )
        .fold(Vec::new, |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        })
        .parse_next(input)?;

        let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| {
            if let Some(op) = op {
                expr_1.combine(
                    |expr_1, expr_2| ParsedExpr::union(op, expr_1, expr_2),
                    expr_2,
                )
            } else {
                ExprResult::Error
            }
        });

        Ok(expr)
    })
    .parse_next(input)
}

fn parse_or_operator<'i>(input: &mut Span<'i>) -> PResult<Option<OrOperator>> {
    trace(
        "parse_or_operator",
        ws(alt((
            |input: &mut Span<'i>| {
                let start = input.current_token_start();
                // This is not a valid OR operator in this position, but catch it to provide a better
                // experience.
                let op = alt(("||", "OR ")).parse_next(input)?;
                // || is not supported in filtersets: suggest using | instead.
                let length = op.len();
                let err = ParseSingleError::InvalidOrOperator((start, length).into());
                input.state.report_error(err);
                Ok(None)
            },
            "or ".value(Some(OrOperator::LiteralOr)),
            '|'.value(Some(OrOperator::Pipe)),
            '+'.value(Some(OrOperator::Plus)),
        ))),
    )
    .parse_next(input)
}

// ---

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    any(test, feature = "internal-testing"),
    derive(test_strategy::Arbitrary)
)]
pub enum AndOperator {
    LiteralAnd,
    Ampersand,
}

impl fmt::Display for AndOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AndOperator::LiteralAnd => f.write_str("and"),
            AndOperator::Ampersand => f.write_str("&"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    any(test, feature = "internal-testing"),
    derive(test_strategy::Arbitrary)
)]
pub enum DifferenceOperator {
    Minus,
}

impl fmt::Display for DifferenceOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DifferenceOperator::Minus => f.write_str("-"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AndOrDifferenceOperator {
    And(AndOperator),
    Difference(DifferenceOperator),
}

fn parse_and_or_difference_expr(input: &mut Span<'_>) -> PResult<ExprResult> {
    trace("parse_and_or_difference_expr", |input: &mut _| {
        let expr = expect_expr(parse_basic_expr).parse_next(input)?;

        let ops = repeat(
            0..,
            (
                parse_and_or_difference_operator,
                expect_expr(parse_basic_expr),
            ),
        )
        .fold(Vec::new, |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        })
        .parse_next(input)?;

        let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| match op {
            Some(AndOrDifferenceOperator::And(op)) => expr_1.combine(
                |expr_1, expr_2| ParsedExpr::intersection(op, expr_1, expr_2),
                expr_2,
            ),
            Some(AndOrDifferenceOperator::Difference(op)) => expr_1.combine(
                |expr_1, expr_2| ParsedExpr::difference(op, expr_1, expr_2),
                expr_2,
            ),
            None => ExprResult::Error,
        });

        Ok(expr)
    })
    .parse_next(input)
}

fn parse_and_or_difference_operator<'i>(
    input: &mut Span<'i>,
) -> PResult<Option<AndOrDifferenceOperator>> {
    trace(
        "parse_and_or_difference_operator",
        ws(alt((
            |input: &mut Span<'i>| {
                let start = input.current_token_start();
                let op = alt(("&&", "AND ")).parse_next(input)?;
                // && is not supported in filtersets: suggest using & instead.
                let length = op.len();
                let err = ParseSingleError::InvalidAndOperator((start, length).into());
                input.state.report_error(err);
                Ok(None)
            },
            "and ".value(Some(AndOrDifferenceOperator::And(AndOperator::LiteralAnd))),
            '&'.value(Some(AndOrDifferenceOperator::And(AndOperator::Ampersand))),
            '-'.value(Some(AndOrDifferenceOperator::Difference(
                DifferenceOperator::Minus,
            ))),
        ))),
    )
    .parse_next(input)
}

// ---

pub(crate) fn parse(input: Span<'_>) -> Result<ExprResult, winnow::error::ErrMode<Error>> {
    let (_, expr) = terminated(
        parse_expr,
        expect(ws(eof), ParseSingleError::ExpectedEndOfExpression),
    )
    .parse_peek(input)?;
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn parse_regex(input: &str) -> NameMatcher {
        let mut errors = Vec::new();
        let span = new_span(input, &mut errors);
        parse_regex_matcher.parse_peek(span).unwrap().1.unwrap()
    }

    #[test]
    fn test_parse_regex() {
        assert_eq!(
            NameMatcher::Regex(regex::Regex::new(r"some.*").unwrap()),
            parse_regex(r"/some.*/")
        );

        assert_eq!(
            NameMatcher::Regex(regex::Regex::new(r"a/a").unwrap()),
            parse_regex(r"/a\/a/")
        );

        assert_eq!(
            NameMatcher::Regex(regex::Regex::new(r"\w/a").unwrap()),
            parse_regex(r"/\w\/a/")
        );

        assert_eq!(
            NameMatcher::Regex(regex::Regex::new(r"\w\\/a").unwrap()),
            parse_regex(r"/\w\\\/a/")
        );

        assert_eq!(
            NameMatcher::Regex(regex::Regex::new(r"\p{Greek}\\/a").unwrap()),
            parse_regex(r"/\p{Greek}\\\/a/")
        );
    }

    #[track_caller]
    fn parse_glob(input: &str) -> NameMatcher {
        let mut errors = Vec::new();
        let span = new_span(input, &mut errors);
        let matcher = parse_glob_matcher
            .parse_peek(span)
            .unwrap_or_else(|error| {
                panic!("for input {input}, parse_glob_matcher returned an error: {error}")
            })
            .1
            .unwrap_or_else(|| {
                panic!(
                    "for input {input}, parse_glob_matcher returned None \
                     (reported errors: {errors:?})"
                )
            });
        if !errors.is_empty() {
            panic!("for input {input}, parse_glob_matcher reported errors: {errors:?}");
        }

        matcher
    }

    fn make_glob_matcher(glob: &str, implicit: bool) -> NameMatcher {
        NameMatcher::Glob {
            glob: GenericGlob::new(glob.to_owned()).unwrap(),
            implicit,
        }
    }

    #[test]
    fn test_parse_glob_matcher() {
        #[track_caller]
        fn assert_glob(input: &str, expected: &str) {
            assert_eq!(
                make_glob_matcher(expected, false),
                parse_glob(input),
                "expected matches actual for input {input:?}",
            );
        }

        // Need the closing ) since that's used as the delimiter.
        assert_glob(r"#something)", "something");
        assert_glob(r"#something*)", "something*");
        assert_glob(r"#something?)", "something?");
        assert_glob(r"#something[abc])", "something[abc]");
        assert_glob(r"#something[!abc])", "something[!abc]");
        assert_glob(r"#something[a-c])", "something[a-c]");
        assert_glob(r"#foobar\b)", "foobar\u{08}");
        assert_glob(r"#foobar\\b)", "foobar\\b");
        assert_glob(r"#foobar\))", "foobar)");
    }

    #[track_caller]
    fn parse_set(input: &str) -> ParsedLeaf {
        let mut errors = Vec::new();
        let span = new_span(input, &mut errors);
        parse_set_def.parse_peek(span).unwrap().1.unwrap()
    }

    macro_rules! assert_parsed_leaf {
        ($input: expr, $name:ident, $matches:expr) => {
            assert!(matches!($input, ParsedLeaf::$name(x, _) if x == $matches));
        };
    }

    #[test]
    fn test_parse_name_matcher() {
        // Basic matchers
        assert_parsed_leaf!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: false,
            }
        );

        assert_parsed_leaf!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(/some.*/)"),
            Test,
            NameMatcher::Regex(regex::Regex::new("some.*").unwrap())
        );
        assert_parsed_leaf!(
            parse_set("test(#something)"),
            Test,
            make_glob_matcher("something", false)
        );
        assert_parsed_leaf!(
            parse_set("test(#something*)"),
            Test,
            make_glob_matcher("something*", false)
        );
        assert_parsed_leaf!(
            parse_set(r"test(#something/[?])"),
            Test,
            make_glob_matcher("something/[?]", false)
        );

        // Default matchers
        assert_parsed_leaf!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: true,
            }
        );
        assert_parsed_leaf!(
            parse_set("package(something)"),
            Package,
            make_glob_matcher("something", true)
        );

        // Explicit contains matching
        assert_parsed_leaf!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(~~something)"),
            Test,
            NameMatcher::Contains {
                value: "~something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(~=something)"),
            Test,
            NameMatcher::Contains {
                value: "=something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(~/something/)"),
            Test,
            NameMatcher::Contains {
                value: "/something/".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(~#something)"),
            Test,
            NameMatcher::Contains {
                value: "#something".to_string(),
                implicit: false,
            }
        );

        // Explicit equals matching.
        assert_parsed_leaf!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(=~something)"),
            Test,
            NameMatcher::Equal {
                value: "~something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(==something)"),
            Test,
            NameMatcher::Equal {
                value: "=something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(=/something/)"),
            Test,
            NameMatcher::Equal {
                value: "/something/".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("test(=#something)"),
            Test,
            NameMatcher::Equal {
                value: "#something".to_string(),
                implicit: false,
            }
        );

        // Explicit glob matching.
        assert_parsed_leaf!(
            parse_set("test(#~something)"),
            Test,
            make_glob_matcher("~something", false)
        );
        assert_parsed_leaf!(
            parse_set("test(#=something)"),
            Test,
            make_glob_matcher("=something", false)
        );
        assert_parsed_leaf!(
            parse_set("test(#/something/)"),
            Test,
            make_glob_matcher("/something/", false)
        );
        assert_parsed_leaf!(
            parse_set("test(##something)"),
            Test,
            make_glob_matcher("#something", false)
        );
    }

    #[test]
    fn test_parse_name_matcher_quote() {
        assert_parsed_leaf!(
            parse_set(r"test(some'thing)"),
            Test,
            NameMatcher::Contains {
                value: r"some'thing".to_string(),
                implicit: true,
            }
        );
        assert_parsed_leaf!(
            parse_set(r"test(some(thing\))"),
            Test,
            NameMatcher::Contains {
                value: r"some(thing)".to_string(),
                implicit: true,
            }
        );
        assert_parsed_leaf!(
            parse_set(r"test(some \u{55})"),
            Test,
            NameMatcher::Contains {
                value: r"some U".to_string(),
                implicit: true,
            }
        );
    }

    #[test]
    fn test_parse_set_def() {
        assert_eq!(ParsedLeaf::All, parse_set("all()"));
        assert_eq!(ParsedLeaf::All, parse_set(" all ( ) "));

        assert_eq!(ParsedLeaf::None, parse_set("none()"));

        assert_parsed_leaf!(
            parse_set("package(=something)"),
            Package,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_parsed_leaf!(
            parse_set("deps(something)"),
            Deps,
            make_glob_matcher("something", true)
        );
        assert_parsed_leaf!(
            parse_set("rdeps(something)"),
            Rdeps,
            make_glob_matcher("something", true)
        );
        assert_parsed_leaf!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: true,
            }
        );
        assert_parsed_leaf!(parse_set("platform(host)"), Platform, BuildPlatform::Host);
        assert_parsed_leaf!(
            parse_set("platform(target)"),
            Platform,
            BuildPlatform::Target
        );
        assert_parsed_leaf!(
            parse_set("platform(    host    )"),
            Platform,
            BuildPlatform::Host
        );
    }

    #[track_caller]
    fn parse(input: &str) -> ParsedExpr {
        match ParsedExpr::parse(input) {
            Ok(expr) => expr,
            Err(errors) => {
                for single_error in &errors {
                    let report = miette::Report::new(single_error.clone())
                        .with_source_code(input.to_owned());
                    eprintln!("{report:?}");
                }
                panic!("Not a valid expression!")
            }
        }
    }

    #[test]
    fn test_parse_expr_set() {
        let expr = ParsedExpr::all();
        assert_eq!(expr, parse("all()"));
        assert_eq!(expr, parse("  all ( ) "));
        assert_eq!(format!("{expr}"), "all()");
    }

    #[test]
    fn test_parse_expr_not() {
        let expr = ParsedExpr::all().not(NotOperator::LiteralNot);
        assert_eq_both_ways(&expr, "not all()");
        assert_eq!(expr, parse("not  all()"));

        let expr = ParsedExpr::all().not(NotOperator::Exclamation);
        assert_eq_both_ways(&expr, "! all()");
        assert_eq!(expr, parse("!all()"));

        let expr = ParsedExpr::all()
            .not(NotOperator::LiteralNot)
            .not(NotOperator::LiteralNot);
        assert_eq_both_ways(&expr, "not not all()");
    }

    #[test]
    fn test_parse_expr_intersection() {
        let expr = ParsedExpr::intersection(
            AndOperator::LiteralAnd,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        assert_eq_both_ways(&expr, "all() and none()");
        assert_eq!(expr, parse("all()and none()"));

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        assert_eq_both_ways(&expr, "all() & none()");
        assert_eq!(expr, parse("all()&none()"));
    }

    #[test]
    fn test_parse_expr_union() {
        let expr = ParsedExpr::union(OrOperator::LiteralOr, ParsedExpr::all(), ParsedExpr::none());
        assert_eq_both_ways(&expr, "all() or none()");
        assert_eq!(expr, parse("all()or none()"));

        let expr = ParsedExpr::union(OrOperator::Pipe, ParsedExpr::all(), ParsedExpr::none());
        assert_eq_both_ways(&expr, "all() | none()");
        assert_eq!(expr, parse("all()|none()"));

        let expr = ParsedExpr::union(OrOperator::Plus, ParsedExpr::all(), ParsedExpr::none());
        assert_eq_both_ways(&expr, "all() + none()");
        assert_eq!(expr, parse("all()+none()"));
    }

    #[test]
    fn test_parse_expr_difference() {
        let expr = ParsedExpr::difference(
            DifferenceOperator::Minus,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        assert_eq_both_ways(&expr, "all() - none()");
        assert_eq!(expr, parse("all()-none()"));
    }

    #[test]
    fn test_parse_expr_precedence() {
        let expr = ParsedExpr::intersection(
            AndOperator::LiteralAnd,
            ParsedExpr::all().not(NotOperator::LiteralNot),
            ParsedExpr::none(),
        );
        assert_eq_both_ways(&expr, "not all() and none()");

        let expr = ParsedExpr::intersection(
            AndOperator::LiteralAnd,
            ParsedExpr::all(),
            ParsedExpr::none().not(NotOperator::LiteralNot),
        );
        assert_eq_both_ways(&expr, "all() and not none()");

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        let expr = ParsedExpr::union(OrOperator::Pipe, expr, ParsedExpr::all());
        assert_eq_both_ways(&expr, "all() & none() | all()");

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::none(),
            ParsedExpr::all(),
        );
        let expr = ParsedExpr::union(OrOperator::Pipe, ParsedExpr::all(), expr);
        assert_eq_both_ways(&expr, "all() | none() & all()");

        let expr =
            ParsedExpr::union(OrOperator::Pipe, ParsedExpr::all(), ParsedExpr::none()).parens();
        let expr = ParsedExpr::intersection(AndOperator::Ampersand, expr, ParsedExpr::all());
        assert_eq_both_ways(&expr, "(all() | none()) & all()");

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::none(),
            ParsedExpr::all(),
        )
        .parens();
        let expr = ParsedExpr::union(OrOperator::Pipe, ParsedExpr::all(), expr);
        assert_eq_both_ways(&expr, "all() | (none() & all())");

        let expr = ParsedExpr::difference(
            DifferenceOperator::Minus,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        let expr = ParsedExpr::intersection(AndOperator::Ampersand, expr, ParsedExpr::all());
        assert_eq_both_ways(&expr, "all() - none() & all()");

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::all(),
            ParsedExpr::none(),
        );
        let expr = ParsedExpr::difference(DifferenceOperator::Minus, expr, ParsedExpr::all());
        assert_eq_both_ways(&expr, "all() & none() - all()");

        let expr = ParsedExpr::intersection(
            AndOperator::Ampersand,
            ParsedExpr::none(),
            ParsedExpr::all(),
        )
        .parens()
        .not(NotOperator::LiteralNot);
        assert_eq_both_ways(&expr, "not (none() & all())");
    }

    #[test]
    fn test_parse_comma() {
        // accept escaped comma
        let expr = ParsedExpr::Set(ParsedLeaf::Test(
            NameMatcher::Contains {
                value: "a,".to_string(),
                implicit: false,
            },
            (5, 4).into(),
        ));
        assert_eq_both_ways(&expr, r"test(~a\,)");

        // string parsing is compatible with possible future syntax
        fn parse_future_syntax(
            input: &mut Span<'_>,
        ) -> PResult<(Option<NameMatcher>, Option<NameMatcher>)> {
            let _ = "something".parse_next(input)?;
            let _ = '('.parse_next(input)?;
            let n1 = set_matcher(DefaultMatcher::Contains).parse_next(input)?;
            let _ = ws(',').parse_next(input)?;
            let n2 = set_matcher(DefaultMatcher::Contains).parse_next(input)?;
            let _ = ')'.parse_next(input)?;
            Ok((n1, n2))
        }

        let mut errors = Vec::new();
        let mut span = new_span("something(aa, bb)", &mut errors);
        if parse_future_syntax.parse_next(&mut span).is_err() {
            panic!("Failed to parse comma separated matchers");
        }
    }

    #[track_caller]
    fn parse_err(input: &str) -> Vec<ParseSingleError> {
        let mut errors = Vec::new();
        let span = new_span(input, &mut errors);
        super::parse(span).unwrap();
        errors
    }

    macro_rules! assert_error {
        ($error:ident, $name:ident, $start:literal, $end:literal) => {{
            let matches = matches!($error, ParseSingleError::$name(span) if span == ($start, $end).into());
            assert!(
                matches,
                "expected: {:?}, actual: error: {:?}",
                ParseSingleError::$name(($start, $end).into()),
                $error,
            );
        }};
    }

    #[test]
    fn test_invalid_and_operator() {
        let src = "all() && none()";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidAndOperator, 6, 2);

        let src = "all() AND none()";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidAndOperator, 6, 4);
    }

    #[test]
    fn test_invalid_or_operator() {
        let src = "all() || none()";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidOrOperator, 6, 2);

        let src = "all() OR none()";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidOrOperator, 6, 3);
    }

    #[test]
    fn test_missing_close_parentheses() {
        let src = "all(";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseParenthesis, 4, 0);
    }

    #[test]
    fn test_missing_open_parentheses() {
        let src = "all)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedOpenParenthesis, 3, 0);
    }

    #[test]
    fn test_missing_parentheses() {
        let src = "all";
        let mut errors = parse_err(src);
        assert_eq!(2, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedOpenParenthesis, 3, 0);
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseParenthesis, 3, 0);
    }

    #[test]
    fn test_invalid_escapes() {
        let src = r"package(foobar\$\#\@baz)";
        let mut errors = parse_err(src);
        assert_eq!(3, errors.len());

        // Ensure all three errors are reported.
        let error = errors.remove(0);
        assert_error!(error, InvalidEscapeCharacter, 14, 2);

        let error = errors.remove(0);
        assert_error!(error, InvalidEscapeCharacter, 16, 2);

        let error = errors.remove(0);
        assert_error!(error, InvalidEscapeCharacter, 18, 2);
    }

    #[test]
    fn test_invalid_regex() {
        let src = "package(/)/)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert!(
            matches!(error, ParseSingleError::InvalidRegex { span, .. } if span == (9, 1).into())
        );

        // Ensure more detailed error messages if possible.
        let src = "package(/foo(ab/)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        let (span, message) = match error {
            ParseSingleError::InvalidRegex { span, message } => (span, message),
            other => panic!("expected invalid regex with details, found {other}"),
        };
        assert_eq!(span, (12, 1).into(), "span matches");
        assert_eq!(message, "unclosed group");
    }

    #[test]
    fn test_invalid_glob() {
        let src = "package(#)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidString, 9, 0);

        let src = "package(#foo[)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        let (span, error) = match error {
            ParseSingleError::InvalidGlob { span, error } => (span, error),
            other => panic!("expected InvalidGlob with details, found {other}"),
        };
        assert_eq!(span, (9, 4).into(), "span matches");
        assert_eq!(error.to_string(), "unclosed character class; missing ']'");
    }

    #[test]
    fn test_invalid_platform() {
        let src = "platform(foo)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidPlatformArgument, 9, 3);

        let src = "platform(   bar\\t)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidPlatformArgument, 9, 8);
    }

    #[test]
    fn test_missing_close_regex() {
        let src = "package(/aaa)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseRegex, 12, 0);
    }

    #[test]
    fn test_unexpected_argument() {
        let src = "all(aaa)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, UnexpectedArgument, 4, 3);
    }

    #[test]
    fn test_expected_expr() {
        let src = "all() + ";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedExpr, 7, 1);
    }

    #[test]
    fn test_expected_eof() {
        let src = "all() blabla";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, ExpectedEndOfExpression, 5, 7);
    }

    #[test]
    fn test_missing_argument() {
        let src = "test()";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidString, 5, 0);
    }

    #[test]
    fn test_unexpected_comma() {
        let src = "test(aa, )";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, UnexpectedComma, 7, 0);
    }

    #[test]
    fn test_complex_error() {
        let src = "all) + package(/not) - deps(expr none)";
        let mut errors = parse_err(src);
        assert_eq!(2, errors.len(), "{errors:?}");
        let error = errors.remove(0);
        assert_error!(error, ExpectedOpenParenthesis, 3, 0);
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseRegex, 19, 0);
    }

    #[test_strategy::proptest]
    fn proptest_expr_roundtrip(#[strategy(ParsedExpr::strategy())] expr: ParsedExpr<()>) {
        let expr_string = expr.to_string();
        eprintln!("expr string: {expr_string}");
        let expr_2 = parse(&expr_string).drop_source_span();

        assert_eq!(expr, expr_2, "exprs must roundtrip");
    }

    #[track_caller]
    fn assert_eq_both_ways(expr: &ParsedExpr, string: &str) {
        assert_eq!(expr, &parse(string));
        assert_eq!(format!("{expr}"), string);
    }
}
