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
//!     - return an error variant of the expected result type
//!     - push an error in the parsing state (in span.extra)

use miette::SourceSpan;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_until, take_while1},
    character::complete::char,
    combinator::{eof, map, recognize},
    multi::{fold_many0, many0},
    sequence::{delimited, pair, preceded, terminated},
};
use nom_tracable::tracable_parser;
use unicode_xid::UnicodeXID;

use crate::error::*;

pub type Span<'a> = nom_locate::LocatedSpan<&'a str, State<'a>>;
type IResult<'a, T> = nom::IResult<Span<'a>, T>;

impl<'a> ToSourceSpane for Span<'a> {
    fn to_span(&self) -> SourceSpan {
        (self.location_offset(), self.fragment().len()).into()
    }
}

#[derive(Debug)]
pub(crate) enum RawNameMatcher {
    Equal(String),
    Contains(String),
    Regex(regex::Regex),

    Error,
}

impl PartialEq for RawNameMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Contains(s1), Self::Contains(s2)) => s1 == s2,
            (Self::Equal(s1), Self::Equal(s2)) => s1 == s2,
            (Self::Regex(r1), Self::Regex(r2)) => r1.as_str() == r2.as_str(),
            _ => false,
        }
    }
}

impl Eq for RawNameMatcher {}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SetDef {
    Package(RawNameMatcher),
    Deps(RawNameMatcher),
    Rdeps(RawNameMatcher),
    Test(RawNameMatcher),
    All,
    None,

    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Expr {
    Not(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
    Intersection(Box<Expr>, Box<Expr>),
    Set(SetDef),

    Error,
}

impl Expr {
    fn boxed(self) -> Box<Self> {
        Box::new(self)
    }

    fn not(self) -> Self {
        Expr::Not(self.boxed())
    }

    fn union(expr_1: Self, expr_2: Self) -> Self {
        Expr::Union(expr_1.boxed(), expr_2.boxed())
    }

    fn intersection(expr_1: Self, expr_2: Self) -> Self {
        Expr::Intersection(expr_1.boxed(), expr_2.boxed())
    }

    fn difference(expr_1: Self, expr_2: Self) -> Self {
        Expr::Intersection(expr_1.boxed(), expr_2.not().boxed())
    }

    #[cfg(test)]
    fn all() -> Expr {
        Expr::Set(SetDef::All)
    }

    #[cfg(test)]
    fn none() -> Expr {
        Expr::Set(SetDef::None)
    }
}

enum SpanLength {
    Unknown,
    Exact(usize),
}

fn expect_inner<'a, F, T>(
    mut parser: F,
    make_err: fn(SourceSpan) -> Error,
    limit: SpanLength,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    move |input| match parser(input) {
        Ok((remaining, out)) => Ok((remaining, Some(out))),
        Err(nom::Err::Error(err)) | Err(nom::Err::Failure(err)) => {
            let nom::error::Error { input, .. } = err;
            let start = input.location_offset();
            let len = input.fragment().len();
            let span = match limit {
                SpanLength::Unknown => (start, len).into(),
                SpanLength::Exact(x) => (start, x.min(len)).into(),
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
    make_err: fn(SourceSpan) -> Error,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    expect_inner(parser, make_err, SpanLength::Unknown)
}

fn expect_char<'a>(
    c: char,
    make_err: fn(SourceSpan) -> Error,
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
        let (i, _) = many0(char(' '))(input.clone())?;
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

fn is_identifier_char(c: char) -> bool {
    // This is use for NameMatcher::Contains(_) and NameMatcher::Equal(_)
    // The output should be valid part of a test-name or a package name.
    c == ':' || c.is_xid_continue()
}

#[tracable_parser]
fn parse_identifier_part(input: Span) -> IResult<Option<String>> {
    let start = input.location_offset();
    match map(
        recognize::<_, _, nom::error::Error<Span>, _>(take_while1(is_identifier_char)),
        |res: Span| res.fragment().to_string(),
    )(input.clone())
    {
        Ok((i1, res1)) => {
            match recognize::<_, _, nom::error::Error<Span>, _>(take_till(|c| c == ')'))(i1.clone())
            {
                Ok((i, res)) => {
                    if res.fragment().trim().is_empty() {
                        Ok((i1, Some(res1)))
                    } else {
                        let end = i.location_offset() - start;
                        let err = Error::InvalidIdentifier((start, end).into());
                        i.extra.report_error(err);
                        Ok((i, None))
                    }
                }
                Err(_) => unreachable!(),
            }
        }
        Err(_) => {
            match recognize::<_, _, nom::error::Error<Span>, _>(take_till(|c| c == ')'))(input) {
                Ok((i, res)) => {
                    let end = i.location_offset() - start;
                    let err = if res.fragment().trim().is_empty() {
                        Error::ExpectedIdentifier((start, end).into())
                    } else {
                        Error::InvalidIdentifier((start, end).into())
                    };
                    i.extra.report_error(err);
                    Ok((i, None))
                }
                Err(_) => unreachable!(),
            }
        }
    }
}

// This parse will never fail
#[tracable_parser]
fn parse_contains_matcher(input: Span) -> IResult<RawNameMatcher> {
    ws(map(parse_identifier_part, |res: Option<String>| {
        res.map(RawNameMatcher::Contains)
            .unwrap_or(RawNameMatcher::Error)
    }))(input)
}

#[tracable_parser]
fn parse_equal_matcher(input: Span) -> IResult<RawNameMatcher> {
    ws(map(
        preceded(char('='), ws(parse_identifier_part)),
        |res: Option<String>| {
            res.map(RawNameMatcher::Equal)
                .unwrap_or(RawNameMatcher::Error)
        },
    ))(input)
}

#[tracable_parser]
fn parse_regex_(input: Span) -> IResult<RawNameMatcher> {
    let (i, res) =
        match recognize::<_, _, nom::error::Error<Span>, _>(take_until("/"))(input.clone()) {
            Ok((i, res)) => (i, res),
            Err(_) => {
                match recognize::<_, _, nom::error::Error<Span>, _>(take_till(|c| c == ')'))(
                    input.clone(),
                ) {
                    Ok((i, res)) => {
                        let start = i.location_offset();
                        let err = Error::ExpectedCloseRegex((start, 0).into());
                        i.extra.report_error(err);
                        (i, res)
                    }
                    Err(_) => return Ok((input, RawNameMatcher::Error)),
                }
            }
        };
    let res = regex::Regex::new(res.fragment())
        .map(RawNameMatcher::Regex)
        .unwrap_or_else(|_| {
            let err = Error::InvalidRegex(res.to_span());
            i.extra.report_error(err);
            RawNameMatcher::Error
        });
    Ok((i, res))
}

#[tracable_parser]
fn parse_regex_matcher(input: Span) -> IResult<RawNameMatcher> {
    ws(delimited(
        char('/'),
        parse_regex_,
        silent_expect(ws(char('/'))),
    ))(input)
}

// This parse will never fail (because parse_contains_matcher won't)
#[tracable_parser]
fn parse_set_matcher(input: Span) -> IResult<RawNameMatcher> {
    ws(alt((
        parse_regex_matcher,
        parse_equal_matcher,
        parse_contains_matcher,
    )))(input)
}

fn nullary_set_def(
    name: &'static str,
    make_set: fn() -> SetDef,
) -> impl FnMut(Span) -> IResult<Option<SetDef>> {
    move |i| {
        let (i, _) = tag(name)(i)?;
        let (i, _) = expect_char('(', Error::ExpectedOpenParenthesis)(i)?;
        let i = match recognize::<_, _, nom::error::Error<Span>, _>(take_till(|c| c == ')'))(i) {
            Ok((i, res)) => {
                if !res.fragment().trim().is_empty() {
                    let err = Error::UnexpectedNameMatcher(res.to_span());
                    i.extra.report_error(err);
                }
                i
            }
            Err(_) => unreachable!(),
        };
        let (i, _) = expect_char(')', Error::ExpectedCloseParenthesis)(i)?;
        Ok((i, Some(make_set())))
    }
}

fn unary_set_def(
    name: &'static str,
    make_set: fn(RawNameMatcher) -> SetDef,
) -> impl FnMut(Span) -> IResult<Option<SetDef>> {
    move |i| {
        let (i, _) = tag(name)(i)?;
        let (i, _) = expect_char('(', Error::ExpectedOpenParenthesis)(i)?;
        let (i, res) = parse_set_matcher(i)?;
        let (i, _) = expect_char(')', Error::ExpectedCloseParenthesis)(i)?;
        Ok((i, Some(make_set(res))))
    }
}

#[tracable_parser]
fn parse_set_def(input: Span) -> IResult<SetDef> {
    map(
        ws(alt((
            unary_set_def("package", SetDef::Package),
            unary_set_def("deps", SetDef::Deps),
            unary_set_def("rdeps", SetDef::Rdeps),
            unary_set_def("test", SetDef::Test),
            nullary_set_def("all", || SetDef::All),
            nullary_set_def("none", || SetDef::None),
        ))),
        |res| res.unwrap_or(SetDef::Error),
    )(input)
}

fn expect_expr<'a, P: FnMut(Span<'a>) -> IResult<'a, Expr>>(
    inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, Expr> {
    map(expect(inner, Error::ExpectedExpr), |res| {
        res.unwrap_or(Expr::Error)
    })
}

#[tracable_parser]
fn parse_expr_not(input: Span) -> IResult<Expr> {
    map(
        preceded(
            alt((tag("not "), tag("!"))),
            expect_expr(ws(parse_basic_expr)),
        ),
        |e| Expr::Not(Box::new(e)),
    )(input)
}

#[tracable_parser]
fn parse_parentheses_expr(input: Span) -> IResult<Expr> {
    delimited(
        char('('),
        expect_expr(parse_expr),
        expect_char(')', Error::ExpectedCloseParenthesis),
    )(input)
}

#[tracable_parser]
fn parse_basic_expr(input: Span) -> IResult<Expr> {
    ws(alt((
        map(parse_set_def, Expr::Set),
        parse_expr_not,
        parse_parentheses_expr,
    )))(input)
}

enum Operator {
    Union,
    Intersection,
    Difference,
}

#[tracable_parser]
fn parse_operator(input: Span) -> IResult<Operator> {
    ws(alt((
        map(alt((tag("or "), tag("|"), tag("+"))), |_| Operator::Union),
        map(alt((tag("and "), tag("&"))), |_| Operator::Intersection),
        map(char('-'), |_| Operator::Difference),
    )))(input)
}

#[tracable_parser]
fn parse_expr(input: Span) -> IResult<Expr> {
    let (input, expr) = expect_expr(parse_basic_expr)(input)?;

    let (input, ops) = fold_many0(
        pair(parse_operator, expect_expr(parse_basic_expr)),
        Vec::new,
        |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        },
    )(input)?;

    let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| match op {
        Operator::Union => Expr::union(expr_1, expr_2),
        Operator::Intersection => Expr::intersection(expr_1, expr_2),
        Operator::Difference => Expr::difference(expr_1, expr_2),
    });

    Ok((input, expr))
}

pub(crate) fn parse(input: Span) -> Result<Expr, nom::Err<nom::error::Error<Span>>> {
    let (_, expr) = terminated(parse_expr, expect(ws(eof), Error::ExpectedEndOfExpression))(input)?;
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[track_caller]
    fn parse_set(input: &str) -> SetDef {
        let errors = RefCell::new(Vec::new());
        parse_set_def(Span::new_extra(input, State::new(&errors)))
            .unwrap()
            .1
    }

    #[test]
    fn test_parse_name_matcher() {
        assert_eq!(
            SetDef::Test(RawNameMatcher::Contains("something".to_string())),
            parse_set("test(something)")
        );
        assert_eq!(
            SetDef::Test(RawNameMatcher::Equal("something".to_string())),
            parse_set("test(=something)")
        );
        assert_eq!(
            SetDef::Test(RawNameMatcher::Regex(regex::Regex::new("some.*").unwrap())),
            parse_set("test(/some.*/)")
        );
    }

    #[test]
    fn test_parse_set_def() {
        assert_eq!(SetDef::All, parse_set("all()"));
        assert_eq!(SetDef::All, parse_set(" all ( ) "));

        assert_eq!(SetDef::None, parse_set("none()"));

        assert_eq!(
            SetDef::Package(RawNameMatcher::Contains("something".to_string())),
            parse_set("package(something)")
        );
        assert_eq!(
            SetDef::Deps(RawNameMatcher::Contains("something".to_string())),
            parse_set("deps(something)")
        );
        assert_eq!(
            SetDef::Rdeps(RawNameMatcher::Contains("something".to_string())),
            parse_set("rdeps(something)")
        );
        assert_eq!(
            SetDef::Test(RawNameMatcher::Contains("something".to_string())),
            parse_set("test(something)")
        );
    }

    #[track_caller]
    fn parse(input: &str) -> Expr {
        let errors = RefCell::new(Vec::new());
        super::parse(Span::new_extra(input, State::new(&errors))).unwrap()
    }

    #[test]
    fn test_parse_expr_set() {
        let expr = Expr::all();
        assert_eq!(expr, parse("all()"));
        assert_eq!(expr, parse("  all ( ) "));
    }

    #[test]
    fn test_parse_expr_not() {
        let expr = Expr::all().not();
        assert_eq!(expr, parse("not all()"));
        assert_eq!(expr, parse("not  all()"));
        assert_eq!(expr, parse("!all()"));
        assert_eq!(expr, parse("! all()"));

        let expr = Expr::all().not().not();
        assert_eq!(expr, parse("not not all()"));
    }

    #[test]
    fn test_parse_expr_intersection() {
        let expr = Expr::intersection(Expr::all(), Expr::none());
        assert_eq!(expr, parse("all() and none()"));
        assert_eq!(expr, parse("all()and none()"));
        assert_eq!(expr, parse("all() & none()"));
        assert_eq!(expr, parse("all()&none()"));
    }

    #[test]
    fn test_parse_expr_union() {
        let expr = Expr::union(Expr::all(), Expr::none());
        assert_eq!(expr, parse("all() or none()"));
        assert_eq!(expr, parse("all()or none()"));
        assert_eq!(expr, parse("all() | none()"));
        assert_eq!(expr, parse("all()|none()"));
        assert_eq!(expr, parse("all() + none()"));
        assert_eq!(expr, parse("all()+none()"));
    }

    #[test]
    fn test_parse_expr_difference() {
        let expr = Expr::difference(Expr::all(), Expr::none());
        assert_eq!(expr, parse("all()-none()"));
        assert_eq!(expr, parse("all() - none()"));
        assert_eq!(expr, parse("all() and not none()"));
    }

    #[test]
    fn test_parse_expr_precedence() {
        let expr = Expr::intersection(Expr::all().not(), Expr::none());
        assert_eq!(expr, parse("not all() and none()"));

        let expr = Expr::intersection(Expr::all(), Expr::none());
        let expr = Expr::union(expr, Expr::all());
        assert_eq!(expr, parse("all() & none() | all()"));

        let expr = Expr::union(Expr::all(), Expr::none());
        let expr = Expr::intersection(expr, Expr::all());
        assert_eq!(expr, parse("all() | none() & all()"));
        assert_eq!(expr, parse("(all() | none()) & all()"));

        let expr = Expr::intersection(Expr::none(), Expr::all());
        let expr = Expr::union(Expr::all(), expr);
        assert_eq!(expr, parse("all() | (none() & all())"));

        let expr = Expr::intersection(Expr::none(), Expr::all()).not();
        assert_eq!(expr, parse("not (none() & all())"));
    }

    #[track_caller]
    fn parse_err(input: &str) -> Vec<Error> {
        let errors = RefCell::new(Vec::new());
        super::parse(Span::new_extra(input, State::new(&errors))).unwrap();
        errors.into_inner()
    }

    macro_rules! assert_error {
        ($error:ident, $name:ident, $start:literal, $end:literal) => {
            assert_eq!(Error::$name(($start, $end).into()), $error);
        };
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
    fn test_invalid_regex() {
        let src = "package(/)/)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidRegex, 9, 1);
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
    fn test_invalid_identifier() {
        let src = "package(a aa)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, InvalidIdentifier, 8, 4);
    }

    #[test]
    fn test_unexpected_argument() {
        let src = "all(aaa)";
        let mut errors = parse_err(src);
        assert_eq!(1, errors.len());
        let error = errors.remove(0);
        assert_error!(error, UnexpectedNameMatcher, 4, 3);
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
    fn test_complex_error() {
        let src = "all) + package(/not) - deps(expr none)";
        let mut errors = parse_err(src);
        assert_eq!(3, errors.len(), "{:?}", errors);
        let error = errors.remove(0);
        assert_error!(error, ExpectedOpenParenthesis, 3, 0);
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseRegex, 19, 0);
        let error = errors.remove(0);
        assert_error!(error, InvalidIdentifier, 28, 9);
    }
}
