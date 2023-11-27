// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parsing for filtering expressions
//!
//! The parsing strategy is based on the following blog post:
//! `<https://eyalkalderon.com/blog/nom-error-recovery/>`
//!
//! All high level parsing functions should:
//! - always return Ok(_)
//! - on error:
//!     - consume as much input as it makes sense so that we can try to resume parsing
//!     - return an error/none variant of the expected result type
//!     - push an error in the parsing state (in span.extra)

use guppy::graph::cargo::BuildPlatform;
use miette::SourceSpan;
use nom::{
    branch::alt,
    bytes::complete::{is_not, tag, take_till},
    character::complete::{char, line_ending},
    combinator::{eof, map, peek, recognize, value, verify},
    multi::{fold_many0, many0},
    sequence::{delimited, pair, preceded, terminated},
    Slice,
};
use nom_tracable::tracable_parser;
use std::{cell::RefCell, fmt};

mod glob;
mod unicode_string;
use crate::{errors::*, NameMatcher};
pub(crate) use glob::GenericGlob;
pub(crate) use unicode_string::DisplayParsedString;

pub(crate) type Span<'a> = nom_locate::LocatedSpan<&'a str, State<'a>>;
type IResult<'a, T> = nom::IResult<Span<'a>, T>;

impl<'a> ToSourceSpan for Span<'a> {
    fn to_span(&self) -> SourceSpan {
        (self.location_offset(), self.fragment().len()).into()
    }
}

/// A filter expression that hasn't been compiled against a package graph.
///
/// Not part of the public API. Exposed for testing only.
#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum SetDef<S = SourceSpan> {
    Package(NameMatcher, S),
    Deps(NameMatcher, S),
    Rdeps(NameMatcher, S),
    Kind(NameMatcher, S),
    Binary(NameMatcher, S),
    BinaryId(NameMatcher, S),
    Platform(BuildPlatform, S),
    Test(NameMatcher, S),
    All,
    None,
}

impl SetDef {
    fn drop_source_span(self) -> SetDef<()> {
        match self {
            Self::Package(matcher, _) => SetDef::Package(matcher, ()),
            Self::Deps(matcher, _) => SetDef::Deps(matcher, ()),
            Self::Rdeps(matcher, _) => SetDef::Rdeps(matcher, ()),
            Self::Kind(matcher, _) => SetDef::Kind(matcher, ()),
            Self::Binary(matcher, _) => SetDef::Binary(matcher, ()),
            Self::BinaryId(matcher, _) => SetDef::BinaryId(matcher, ()),
            Self::Platform(platform, _) => SetDef::Platform(platform, ()),
            Self::Test(matcher, _) => SetDef::Test(matcher, ()),
            Self::All => SetDef::All,
            Self::None => SetDef::None,
        }
    }
}

impl<S> fmt::Display for SetDef<S> {
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
            Self::All => write!(f, "all()"),
            Self::None => write!(f, "none()"),
        }
    }
}

/// A filter expression that hasn't been compiled against a package graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedExpr<S = SourceSpan> {
    Not(NotOperator, Box<ParsedExpr<S>>),
    Union(OrOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Intersection(AndOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Difference(DifferenceOperator, Box<ParsedExpr<S>>, Box<ParsedExpr<S>>),
    Parens(Box<ParsedExpr<S>>),
    Set(SetDef<S>),
}

impl ParsedExpr {
    pub fn parse(input: &str) -> Result<Self, Vec<ParseSingleError>> {
        let errors = RefCell::new(Vec::new());
        match parse(Span::new_extra(input, State::new(&errors))).unwrap() {
            ExprResult::Valid(expr) => Ok(expr),
            ExprResult::Error => Err(errors.into_inner()),
        }
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
        ParsedExpr::Set(SetDef::All)
    }

    #[cfg(test)]
    fn none() -> ParsedExpr {
        ParsedExpr::Set(SetDef::None)
    }

    #[allow(unused)]
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
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    move |input| match parser(input) {
        Ok((remaining, out)) => Ok((remaining, Some(out))),
        Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
            let nom::error::Error { input, .. } = err;
            let fragment_start = input.location_offset();
            let fragment_length = input.fragment().len();
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
            input.extra.report_error(err);
            Ok((input, None))
        }
        Err(err) => Err(err),
    }
}

fn expect<'a, F, T>(
    parser: F,
    make_err: fn(SourceSpan) -> ParseSingleError,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    expect_inner(parser, make_err, SpanLength::Unknown)
}

fn expect_n<'a, F, T>(
    parser: F,
    make_err: fn(SourceSpan) -> ParseSingleError,
    limit: SpanLength,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    expect_inner(parser, make_err, limit)
}

fn expect_char<'a>(
    c: char,
    make_err: fn(SourceSpan) -> ParseSingleError,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<char>> {
    expect_inner(ws(char(c)), make_err, SpanLength::Exact(0))
}

fn silent_expect<'a, F, T>(mut parser: F) -> impl FnMut(Span<'a>) -> IResult<Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    move |input| match parser(input) {
        Ok((remaining, out)) => Ok((remaining, Some(out))),
        Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
            let nom::error::Error { input, .. } = err;
            Ok((input, None))
        }
        Err(err) => Err(err),
    }
}

fn ws<'a, T, P: FnMut(Span<'a>) -> IResult<'a, T>>(
    mut inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, T> {
    move |input| {
        let (i, _) = many0(alt((
            // Match individual space characters.
            value((), char(' ')),
            // Match CRLF and LF line endings. This allows filters to be specified as multiline TOML
            // strings.
            value((), line_ending),
        )))(input.clone())?;
        match inner(i) {
            Ok(res) => Ok(res),
            Err(nom::Err::Error(err)) => {
                let nom::error::Error { code, .. } = err;
                Err(nom::Err::Error(nom::error::Error { input, code }))
            }
            Err(nom::Err::Failure(err)) => {
                let nom::error::Error { code, .. } = err;
                Err(nom::Err::Failure(nom::error::Error { input, code }))
            }
            Err(err) => Err(err),
        }
    }
}

// This parse will never fail
#[tracable_parser]
fn parse_matcher_text(input: Span) -> IResult<Option<String>> {
    let (i, res) = match expect(
        unicode_string::parse_string,
        ParseSingleError::InvalidString,
    )(input.clone())
    {
        Ok((i, res)) => (i, res.flatten()),
        Err(nom::Err::Incomplete(_)) => {
            let i = input.slice(input.fragment().len()..);
            // No need for error reporting, missing closing ')' will be detected after
            (i, None)
        }
        Err(_) => unreachable!(),
    };

    if res.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
        let start = i.location_offset();
        i.extra
            .report_error(ParseSingleError::InvalidString((start..0).into()));
    }

    Ok((i, res))
}

#[tracable_parser]
fn parse_contains_matcher(input: Span) -> IResult<Option<NameMatcher>> {
    map(
        preceded(char('~'), parse_matcher_text),
        |res: Option<String>| {
            res.map(|value| NameMatcher::Contains {
                value,
                implicit: false,
            })
        },
    )(input)
}

#[tracable_parser]
fn parse_equal_matcher(input: Span) -> IResult<Option<NameMatcher>> {
    ws(map(
        preceded(char('='), parse_matcher_text),
        |res: Option<String>| {
            res.map(|value| NameMatcher::Equal {
                value,
                implicit: false,
            })
        },
    ))(input)
}

#[tracable_parser]
fn parse_regex_inner(input: Span) -> IResult<String> {
    enum Frag<'a> {
        Literal(&'a str),
        Escape(char),
    }

    let parse_escape = map(alt((map(tag(r"\/"), |_| '/'), char('\\'))), Frag::Escape);
    let parse_literal = map(
        verify(is_not("\\/"), |s: &Span| !s.fragment().is_empty()),
        |s: Span| Frag::Literal(s.fragment()),
    );
    let parse_frag = alt((parse_escape, parse_literal));

    let (i, res) = fold_many0(parse_frag, String::new, |mut string, frag| {
        match frag {
            Frag::Escape(c) => string.push(c),
            Frag::Literal(s) => string.push_str(s),
        }
        string
    })(input)?;

    let (i, _) = peek(char('/'))(i)?;

    Ok((i, res))
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

#[tracable_parser]
fn parse_regex(input: Span) -> IResult<Option<NameMatcher>> {
    let (i, res) = match parse_regex_inner(input.clone()) {
        Ok((i, res)) => (i, res),
        Err(_) => match take_till::<_, _, nom::error::Error<Span>>(|c| c == ')')(input.clone()) {
            Ok((i, _)) => {
                let start = i.location_offset();
                let err = ParseSingleError::ExpectedCloseRegex((start, 0).into());
                i.extra.report_error(err);
                return Ok((i, None));
            }
            Err(_) => unreachable!(),
        },
    };
    match regex::Regex::new(&res).map(NameMatcher::Regex) {
        Ok(res) => Ok((i, Some(res))),
        Err(_) => {
            let start = input.location_offset();
            let end = i.location_offset();
            let err = ParseSingleError::invalid_regex(&res, start, end);
            i.extra.report_error(err);
            Ok((i, None))
        }
    }
}

#[tracable_parser]
fn parse_regex_matcher(input: Span) -> IResult<Option<NameMatcher>> {
    ws(delimited(
        char('/'),
        parse_regex,
        silent_expect(ws(char('/'))),
    ))(input)
}

#[tracable_parser]
fn parse_glob_matcher(input: Span) -> IResult<Option<NameMatcher>> {
    ws(preceded(char('#'), |input| glob::parse_glob(input, false)))(input)
}

// This parse will never fail (because default_matcher won't)
fn set_matcher(
    default_matcher: DefaultMatcher,
) -> impl FnMut(Span) -> IResult<Option<NameMatcher>> {
    move |input: Span| {
        ws(alt((
            parse_regex_matcher,
            parse_glob_matcher,
            parse_equal_matcher,
            parse_contains_matcher,
            default_matcher.into_parser(),
        )))(input)
    }
}

#[tracable_parser]
fn recover_unexpected_comma(input: Span) -> IResult<()> {
    match peek(ws(char(',')))(input.clone()) {
        Ok((i, _)) => {
            let pos = i.location_offset();
            i.extra
                .report_error(ParseSingleError::UnexpectedComma((pos..0).into()));
            match take_till::<_, _, nom::error::Error<Span>>(|c| c == ')')(i) {
                Ok((i, _)) => Ok((i, ())),
                Err(_) => unreachable!(),
            }
        }
        Err(_) => Ok((input, ())),
    }
}

fn nullary_set_def(
    name: &'static str,
    make_set: fn() -> SetDef,
) -> impl FnMut(Span) -> IResult<Option<SetDef>> {
    move |i| {
        let (i, _) = tag(name)(i)?;
        let (i, _) = expect_char('(', ParseSingleError::ExpectedOpenParenthesis)(i)?;
        let i = match recognize::<_, _, nom::error::Error<Span>, _>(take_till(|c| c == ')'))(i) {
            Ok((i, res)) => {
                if !res.fragment().trim().is_empty() {
                    let err = ParseSingleError::UnexpectedArgument(res.to_span());
                    i.extra.report_error(err);
                }
                i
            }
            Err(_) => unreachable!(),
        };
        let (i, _) = expect_char(')', ParseSingleError::ExpectedCloseParenthesis)(i)?;
        Ok((i, Some(make_set())))
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
    fn into_parser(self) -> impl FnMut(Span) -> IResult<Option<NameMatcher>> {
        move |input| match self {
            Self::Equal => map(parse_matcher_text, |res: Option<String>| {
                res.map(NameMatcher::implicit_equal)
            })(input),
            Self::Contains => map(parse_matcher_text, |res: Option<String>| {
                res.map(NameMatcher::implicit_contains)
            })(input),
            Self::Glob => glob::parse_glob(input, true),
        }
    }
}

fn unary_set_def(
    name: &'static str,
    default_matcher: DefaultMatcher,
    make_set: fn(NameMatcher, SourceSpan) -> SetDef,
) -> impl FnMut(Span) -> IResult<Option<SetDef>> {
    move |i| {
        let (i, _) = tag(name)(i)?;
        let (i, _) = expect_char('(', ParseSingleError::ExpectedOpenParenthesis)(i)?;
        let start = i.location_offset();
        let (i, res) = set_matcher(default_matcher)(i)?;
        let end = i.location_offset();
        let (i, _) = recover_unexpected_comma(i)?;
        let (i, _) = expect_char(')', ParseSingleError::ExpectedCloseParenthesis)(i)?;
        Ok((
            i,
            res.map(|matcher| make_set(matcher, (start, end - start).into())),
        ))
    }
}

fn platform_def(i: Span) -> IResult<Option<SetDef>> {
    let (i, _) = tag("platform")(i)?;
    let (i, _) = expect_char('(', ParseSingleError::ExpectedOpenParenthesis)(i)?;
    let start = i.location_offset();
    // Try parsing the argument as a string for better error messages.
    let (i, res) = ws(parse_matcher_text)(i)?;
    let end = i.location_offset();
    let (i, _) = recover_unexpected_comma(i)?;
    let (i, _) = expect_char(')', ParseSingleError::ExpectedCloseParenthesis)(i)?;

    // The returned string will include leading and trailing whitespace.
    let platform = match res.as_deref().map(|res| res.trim()) {
        Some("host") => Some(BuildPlatform::Host),
        Some("target") => Some(BuildPlatform::Target),
        Some(_) => {
            i.extra
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
    Ok((
        i,
        platform.map(|platform| SetDef::Platform(platform, (start, end - start).into())),
    ))
}

#[tracable_parser]
fn parse_set_def(input: Span) -> IResult<Option<SetDef>> {
    ws(alt((
        unary_set_def("package", DefaultMatcher::Glob, SetDef::Package),
        unary_set_def("deps", DefaultMatcher::Glob, SetDef::Deps),
        unary_set_def("rdeps", DefaultMatcher::Glob, SetDef::Rdeps),
        unary_set_def("kind", DefaultMatcher::Equal, SetDef::Kind),
        // binary_id must go above binary, otherwise we'll parse the opening predicate wrong.
        unary_set_def("binary_id", DefaultMatcher::Glob, SetDef::BinaryId),
        unary_set_def("binary", DefaultMatcher::Glob, SetDef::Binary),
        unary_set_def("test", DefaultMatcher::Contains, SetDef::Test),
        platform_def,
        nullary_set_def("all", || SetDef::All),
        nullary_set_def("none", || SetDef::None),
    )))(input)
}

fn expect_expr<'a, P: FnMut(Span<'a>) -> IResult<'a, ExprResult>>(
    inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, ExprResult> {
    map(expect(inner, ParseSingleError::ExpectedExpr), |res| {
        res.unwrap_or(ExprResult::Error)
    })
}

#[tracable_parser]
fn parse_parentheses_expr(input: Span) -> IResult<ExprResult> {
    map(
        delimited(
            char('('),
            expect_expr(parse_expr),
            expect_char(')', ParseSingleError::ExpectedCloseParenthesis),
        ),
        |expr| expr.parens(),
    )(input)
}

#[tracable_parser]
fn parse_basic_expr(input: Span) -> IResult<ExprResult> {
    ws(alt((
        map(parse_set_def, |set| {
            set.map(|set| ExprResult::Valid(ParsedExpr::Set(set)))
                .unwrap_or(ExprResult::Error)
        }),
        parse_expr_not,
        parse_parentheses_expr,
    )))(input)
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

#[tracable_parser]
fn parse_expr_not(input: Span) -> IResult<ExprResult> {
    map(
        pair(
            alt((
                value(NotOperator::LiteralNot, tag("not ")),
                value(NotOperator::Exclamation, tag("!")),
            )),
            expect_expr(ws(parse_basic_expr)),
        ),
        |(op, expr)| expr.negate(op),
    )(input)
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

#[tracable_parser]
fn parse_expr(input: Span) -> IResult<ExprResult> {
    // "or" binds less tightly than "and", so parse and within or.
    let (input, expr) = expect_expr(parse_and_or_difference_expr)(input)?;

    let (input, ops) = fold_many0(
        pair(parse_or_operator, expect_expr(parse_and_or_difference_expr)),
        Vec::new,
        |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        },
    )(input)?;

    let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| {
        expr_1.combine(
            |expr_1, expr_2| ParsedExpr::union(op, expr_1, expr_2),
            expr_2,
        )
    });

    Ok((input, expr))
}

#[tracable_parser]
fn parse_or_operator(input: Span) -> IResult<OrOperator> {
    ws(alt((
        value(OrOperator::LiteralOr, tag("or ")),
        value(OrOperator::Pipe, tag("|")),
        value(OrOperator::Plus, tag("+")),
    )))(input)
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

#[tracable_parser]
fn parse_and_or_difference_expr(input: Span) -> IResult<ExprResult> {
    let (input, expr) = expect_expr(parse_basic_expr)(input)?;

    let (input, ops) = fold_many0(
        pair(
            parse_and_or_difference_operator,
            expect_expr(parse_basic_expr),
        ),
        Vec::new,
        |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        },
    )(input)?;

    let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| match op {
        AndOrDifferenceOperator::And(op) => expr_1.combine(
            |expr_1, expr_2| ParsedExpr::intersection(op, expr_1, expr_2),
            expr_2,
        ),
        AndOrDifferenceOperator::Difference(op) => expr_1.combine(
            |expr_1, expr_2| ParsedExpr::difference(op, expr_1, expr_2),
            expr_2,
        ),
    });

    Ok((input, expr))
}

#[tracable_parser]
fn parse_and_or_difference_operator(input: Span) -> IResult<AndOrDifferenceOperator> {
    ws(alt((
        value(
            AndOrDifferenceOperator::And(AndOperator::LiteralAnd),
            tag("and "),
        ),
        value(
            AndOrDifferenceOperator::And(AndOperator::Ampersand),
            char('&'),
        ),
        value(
            AndOrDifferenceOperator::Difference(DifferenceOperator::Minus),
            char('-'),
        ),
    )))(input)
}

// ---

pub(crate) fn parse(input: Span) -> Result<ExprResult, nom::Err<nom::error::Error<Span>>> {
    let (_, expr) = terminated(
        parse_expr,
        expect(ws(eof), ParseSingleError::ExpectedEndOfExpression),
    )(input)?;
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[track_caller]
    fn parse_regex(input: &str) -> NameMatcher {
        let errors = RefCell::new(Vec::new());
        parse_regex_matcher(Span::new_extra(input, State::new(&errors)))
            .unwrap()
            .1
            .unwrap()
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
        let errors = RefCell::new(Vec::new());
        let matcher = parse_glob_matcher(Span::new_extra(input, State::new(&errors)))
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
        if errors.borrow().len() > 0 {
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
    fn parse_set(input: &str) -> SetDef {
        let errors = RefCell::new(Vec::new());
        parse_set_def(Span::new_extra(input, State::new(&errors)))
            .unwrap()
            .1
            .unwrap()
    }

    macro_rules! assert_set_def {
        ($input: expr, $name:ident, $matches:expr) => {
            assert!(matches!($input, SetDef::$name(x, _) if x == $matches));
        };
    }

    #[test]
    fn test_parse_name_matcher() {
        // Basic matchers
        assert_set_def!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: false,
            }
        );

        assert_set_def!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(/some.*/)"),
            Test,
            NameMatcher::Regex(regex::Regex::new("some.*").unwrap())
        );
        assert_set_def!(
            parse_set("test(#something)"),
            Test,
            make_glob_matcher("something", false)
        );
        assert_set_def!(
            parse_set("test(#something*)"),
            Test,
            make_glob_matcher("something*", false)
        );
        assert_set_def!(
            parse_set(r"test(#something/[?])"),
            Test,
            make_glob_matcher("something/[?]", false)
        );

        // Default matchers
        assert_set_def!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: true,
            }
        );
        assert_set_def!(
            parse_set("package(something)"),
            Package,
            make_glob_matcher("something", true)
        );

        // Explicit contains matching
        assert_set_def!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(~~something)"),
            Test,
            NameMatcher::Contains {
                value: "~something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(~=something)"),
            Test,
            NameMatcher::Contains {
                value: "=something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(~/something/)"),
            Test,
            NameMatcher::Contains {
                value: "/something/".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(~#something)"),
            Test,
            NameMatcher::Contains {
                value: "#something".to_string(),
                implicit: false,
            }
        );

        // Explicit equals matching.
        assert_set_def!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(=~something)"),
            Test,
            NameMatcher::Equal {
                value: "~something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(==something)"),
            Test,
            NameMatcher::Equal {
                value: "=something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(=/something/)"),
            Test,
            NameMatcher::Equal {
                value: "/something/".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("test(=#something)"),
            Test,
            NameMatcher::Equal {
                value: "#something".to_string(),
                implicit: false,
            }
        );

        // Explicit glob matching.
        assert_set_def!(
            parse_set("test(#~something)"),
            Test,
            make_glob_matcher("~something", false)
        );
        assert_set_def!(
            parse_set("test(#=something)"),
            Test,
            make_glob_matcher("=something", false)
        );
        assert_set_def!(
            parse_set("test(#/something/)"),
            Test,
            make_glob_matcher("/something/", false)
        );
        assert_set_def!(
            parse_set("test(##something)"),
            Test,
            make_glob_matcher("#something", false)
        );
    }

    #[test]
    fn test_parse_name_matcher_quote() {
        assert_set_def!(
            parse_set(r"test(some'thing)"),
            Test,
            NameMatcher::Contains {
                value: r"some'thing".to_string(),
                implicit: true,
            }
        );
        assert_set_def!(
            parse_set(r"test(some(thing\))"),
            Test,
            NameMatcher::Contains {
                value: r"some(thing)".to_string(),
                implicit: true,
            }
        );
        assert_set_def!(
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
        assert_eq!(SetDef::All, parse_set("all()"));
        assert_eq!(SetDef::All, parse_set(" all ( ) "));

        assert_eq!(SetDef::None, parse_set("none()"));

        assert_set_def!(
            parse_set("package(=something)"),
            Package,
            NameMatcher::Equal {
                value: "something".to_string(),
                implicit: false,
            }
        );
        assert_set_def!(
            parse_set("deps(something)"),
            Deps,
            make_glob_matcher("something", true)
        );
        assert_set_def!(
            parse_set("rdeps(something)"),
            Rdeps,
            make_glob_matcher("something", true)
        );
        assert_set_def!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains {
                value: "something".to_string(),
                implicit: true,
            }
        );
        assert_set_def!(parse_set("platform(host)"), Platform, BuildPlatform::Host);
        assert_set_def!(
            parse_set("platform(target)"),
            Platform,
            BuildPlatform::Target
        );
        assert_set_def!(
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
        let expr = ParsedExpr::Set(SetDef::Test(
            NameMatcher::Contains {
                value: "a,".to_string(),
                implicit: false,
            },
            (5, 4).into(),
        ));
        assert_eq_both_ways(&expr, r"test(~a\,)");

        // string parsing is compatible with possible future syntax
        fn parse_future_syntax(input: Span) -> IResult<(Option<NameMatcher>, Option<NameMatcher>)> {
            let (i, _) = tag("something")(input)?;
            let (i, _) = char('(')(i)?;
            let (i, n1) = set_matcher(DefaultMatcher::Contains)(i)?;
            let (i, _) = ws(char(','))(i)?;
            let (i, n2) = set_matcher(DefaultMatcher::Contains)(i)?;
            let (i, _) = char(')')(i)?;
            Ok((i, (n1, n2)))
        }

        let errors = RefCell::new(Vec::new());
        if parse_future_syntax(Span::new_extra("something(aa, bb)", State::new(&errors))).is_err() {
            panic!("Failed to parse comma separated matchers");
        }
    }

    #[track_caller]
    fn parse_err(input: &str) -> Vec<ParseSingleError> {
        let errors = RefCell::new(Vec::new());
        super::parse(Span::new_extra(input, State::new(&errors))).unwrap();
        errors.into_inner()
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
