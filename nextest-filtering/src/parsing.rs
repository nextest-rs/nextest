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
    character::complete::char,
    combinator::{eof, map, peek, recognize, verify},
    multi::{fold_many0, many0},
    sequence::{delimited, pair, preceded, terminated},
    Slice,
};
use nom_tracable::tracable_parser;

mod unicode_string;

use crate::{errors::*, NameMatcher};

pub(crate) type Span<'a> = nom_locate::LocatedSpan<&'a str, State<'a>>;
type IResult<'a, T> = nom::IResult<Span<'a>, T>;

impl<'a> ToSourceSpan for Span<'a> {
    fn to_span(&self) -> SourceSpan {
        (self.location_offset(), self.fragment().len()).into()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SetDef {
    Package(NameMatcher, SourceSpan),
    Deps(NameMatcher, SourceSpan),
    Rdeps(NameMatcher, SourceSpan),
    Kind(NameMatcher, SourceSpan),
    Platform(BuildPlatform, SourceSpan),
    Test(NameMatcher, SourceSpan),
    All,
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Expr {
    Not(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
    Intersection(Box<Expr>, Box<Expr>),
    Set(SetDef),
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

pub(crate) enum ParsedExpr {
    Valid(Expr),
    Error,
}

impl ParsedExpr {
    fn combine(self, op: fn(Expr, Expr) -> Expr, other: Self) -> Self {
        match (self, other) {
            (Self::Valid(expr_1), Self::Valid(expr_2)) => Self::Valid(op(expr_1, expr_2)),
            _ => Self::Error,
        }
    }

    fn negate(self) -> Self {
        match self {
            Self::Valid(expr) => Self::Valid(expr.not()),
            _ => Self::Error,
        }
    }
}

enum SpanLength {
    Unknown,
    Exact(usize),
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
    make_err: fn(SourceSpan) -> ParseSingleError,
) -> impl FnMut(Span<'a>) -> IResult<'a, Option<T>>
where
    F: FnMut(Span<'a>) -> IResult<T>,
{
    expect_inner(parser, make_err, SpanLength::Unknown)
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

// This parse will never fail
#[tracable_parser]
fn parse_matcher_text(input: Span) -> IResult<Option<String>> {
    let (i, res) = match expect(
        unicode_string::parse_string,
        ParseSingleError::InvalidString,
    )(input.clone())
    {
        Ok((i, res)) => (i, res),
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
        |res: Option<String>| res.map(NameMatcher::Contains),
    )(input)
}

#[tracable_parser]
fn parse_equal_matcher(input: Span) -> IResult<Option<NameMatcher>> {
    ws(map(
        preceded(char('='), parse_matcher_text),
        |res: Option<String>| res.map(NameMatcher::Equal),
    ))(input)
}

// This parse will never fail
fn default_matcher(
    make: fn(String) -> NameMatcher,
) -> impl FnMut(Span) -> IResult<Option<NameMatcher>> {
    move |input: Span| map(parse_matcher_text, |res: Option<String>| res.map(make))(input)
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

// This parse will never fail (because default_matcher won't)
fn set_matcher(
    make: fn(String) -> NameMatcher,
) -> impl FnMut(Span) -> IResult<Option<NameMatcher>> {
    move |input: Span| {
        ws(alt((
            parse_regex_matcher,
            parse_equal_matcher,
            parse_contains_matcher,
            default_matcher(make),
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

fn unary_set_def(
    name: &'static str,
    make_default_matcher: fn(String) -> NameMatcher,
    make_set: fn(NameMatcher, SourceSpan) -> SetDef,
) -> impl FnMut(Span) -> IResult<Option<SetDef>> {
    move |i| {
        let (i, _) = tag(name)(i)?;
        let (i, _) = expect_char('(', ParseSingleError::ExpectedOpenParenthesis)(i)?;
        let start = i.location_offset();
        let (i, res) = set_matcher(make_default_matcher)(i)?;
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
        unary_set_def("package", NameMatcher::Equal, SetDef::Package),
        unary_set_def("deps", NameMatcher::Equal, SetDef::Deps),
        unary_set_def("rdeps", NameMatcher::Equal, SetDef::Rdeps),
        unary_set_def("kind", NameMatcher::Equal, SetDef::Kind),
        unary_set_def("test", NameMatcher::Contains, SetDef::Test),
        platform_def,
        nullary_set_def("all", || SetDef::All),
        nullary_set_def("none", || SetDef::None),
    )))(input)
}

fn expect_expr<'a, P: FnMut(Span<'a>) -> IResult<'a, ParsedExpr>>(
    inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, ParsedExpr> {
    map(expect(inner, ParseSingleError::ExpectedExpr), |res| {
        res.unwrap_or(ParsedExpr::Error)
    })
}

#[tracable_parser]
fn parse_expr_not(input: Span) -> IResult<ParsedExpr> {
    map(
        preceded(
            alt((tag("not "), tag("!"))),
            expect_expr(ws(parse_basic_expr)),
        ),
        ParsedExpr::negate,
    )(input)
}

#[tracable_parser]
fn parse_parentheses_expr(input: Span) -> IResult<ParsedExpr> {
    delimited(
        char('('),
        expect_expr(parse_expr),
        expect_char(')', ParseSingleError::ExpectedCloseParenthesis),
    )(input)
}

#[tracable_parser]
fn parse_basic_expr(input: Span) -> IResult<ParsedExpr> {
    ws(alt((
        map(parse_set_def, |set| {
            set.map(|set| ParsedExpr::Valid(Expr::Set(set)))
                .unwrap_or(ParsedExpr::Error)
        }),
        parse_expr_not,
        parse_parentheses_expr,
    )))(input)
}

// ---

enum OrOperator {
    Union,
}

#[tracable_parser]
fn parse_expr(input: Span) -> IResult<ParsedExpr> {
    // "or" binds less tightly than "and", so parse and within or.
    let (input, expr) = expect_expr(parse_and_expr)(input)?;

    let (input, ops) = fold_many0(
        pair(parse_or_operator, expect_expr(parse_and_expr)),
        Vec::new,
        |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        },
    )(input)?;

    let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| match op {
        OrOperator::Union => expr_1.combine(Expr::union, expr_2),
    });

    Ok((input, expr))
}

#[tracable_parser]
fn parse_or_operator(input: Span) -> IResult<OrOperator> {
    ws(map(alt((tag("or "), tag("|"), tag("+"))), |_| {
        OrOperator::Union
    }))(input)
}

// ---

enum AndOperator {
    Intersection,
    Difference,
}

#[tracable_parser]
fn parse_and_expr(input: Span) -> IResult<ParsedExpr> {
    let (input, expr) = expect_expr(parse_basic_expr)(input)?;

    let (input, ops) = fold_many0(
        pair(parse_and_operator, expect_expr(parse_basic_expr)),
        Vec::new,
        |mut ops, (op, expr)| {
            ops.push((op, expr));
            ops
        },
    )(input)?;

    let expr = ops.into_iter().fold(expr, |expr_1, (op, expr_2)| match op {
        AndOperator::Intersection => expr_1.combine(Expr::intersection, expr_2),
        AndOperator::Difference => expr_1.combine(Expr::difference, expr_2),
    });

    Ok((input, expr))
}

#[tracable_parser]
fn parse_and_operator(input: Span) -> IResult<AndOperator> {
    ws(alt((
        map(alt((tag("and "), tag("&"))), |_| AndOperator::Intersection),
        map(char('-'), |_| AndOperator::Difference),
    )))(input)
}

// ---

pub(crate) fn parse(input: Span) -> Result<ParsedExpr, nom::Err<nom::error::Error<Span>>> {
    let (_, expr) = terminated(
        parse_expr,
        expect(ws(eof), ParseSingleError::ExpectedEndOfExpression),
    )(input)?;
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

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
            NameMatcher::Contains("something".to_string())
        );

        assert_set_def!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains("something".to_string())
        );
        assert_set_def!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal("something".to_string())
        );
        assert_set_def!(
            parse_set("test(/some.*/)"),
            Test,
            NameMatcher::Regex(regex::Regex::new("some.*").unwrap())
        );

        // Default matchers
        assert_set_def!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains("something".to_string())
        );
        assert_set_def!(
            parse_set("package(something)"),
            Package,
            NameMatcher::Equal("something".to_string())
        );

        // Explicit contains matching
        assert_set_def!(
            parse_set("test(~something)"),
            Test,
            NameMatcher::Contains("something".to_string())
        );
        assert_set_def!(
            parse_set("test(~~something)"),
            Test,
            NameMatcher::Contains("~something".to_string())
        );
        assert_set_def!(
            parse_set("test(~=something)"),
            Test,
            NameMatcher::Contains("=something".to_string())
        );
        assert_set_def!(
            parse_set("test(~/something/)"),
            Test,
            NameMatcher::Contains("/something/".to_string())
        );

        // Explicit equals matching.
        assert_set_def!(
            parse_set("test(=something)"),
            Test,
            NameMatcher::Equal("something".to_string())
        );
        assert_set_def!(
            parse_set("test(=~something)"),
            Test,
            NameMatcher::Equal("~something".to_string())
        );
        assert_set_def!(
            parse_set("test(==something)"),
            Test,
            NameMatcher::Equal("=something".to_string())
        );
        assert_set_def!(
            parse_set("test(=/something/)"),
            Test,
            NameMatcher::Equal("/something/".to_string())
        );
    }

    #[test]
    fn test_parse_name_matcher_quote() {
        assert_set_def!(
            parse_set(r"test(some'thing)"),
            Test,
            NameMatcher::Contains(r"some'thing".to_string())
        );
        assert_set_def!(
            parse_set(r"test(some(thing\))"),
            Test,
            NameMatcher::Contains(r"some(thing)".to_string())
        );
        assert_set_def!(
            parse_set(r"test(some \u{55})"),
            Test,
            NameMatcher::Contains(r"some U".to_string())
        );
    }

    #[test]
    fn test_parse_set_def() {
        assert_eq!(SetDef::All, parse_set("all()"));
        assert_eq!(SetDef::All, parse_set(" all ( ) "));

        assert_eq!(SetDef::None, parse_set("none()"));

        assert_set_def!(
            parse_set("package(something)"),
            Package,
            NameMatcher::Equal("something".to_string())
        );
        assert_set_def!(
            parse_set("deps(something)"),
            Deps,
            NameMatcher::Equal("something".to_string())
        );
        assert_set_def!(
            parse_set("rdeps(something)"),
            Rdeps,
            NameMatcher::Equal("something".to_string())
        );
        assert_set_def!(
            parse_set("test(something)"),
            Test,
            NameMatcher::Contains("something".to_string())
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
    fn parse(input: &str) -> Expr {
        let errors = RefCell::new(Vec::new());
        match super::parse(Span::new_extra(input, State::new(&errors))).unwrap() {
            ParsedExpr::Valid(expr) => expr,
            _ => panic!("Not a  valid expression"),
        }
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

        let expr = Expr::intersection(Expr::all(), Expr::none().not());
        assert_eq!(expr, parse("all() and not none()"));

        let expr = Expr::intersection(Expr::all(), Expr::none());
        let expr = Expr::union(expr, Expr::all());
        assert_eq!(expr, parse("all() & none() | all()"));

        let expr = Expr::intersection(Expr::none(), Expr::all());
        let expr = Expr::union(Expr::all(), expr);
        assert_eq!(expr, parse("all() | none() & all()"));

        let expr = Expr::union(Expr::all(), Expr::none());
        let expr = Expr::intersection(expr, Expr::all());
        assert_eq!(expr, parse("(all() | none()) & all()"));

        let expr = Expr::intersection(Expr::none(), Expr::all());
        let expr = Expr::union(Expr::all(), expr);
        assert_eq!(expr, parse("all() | (none() & all())"));

        let expr = Expr::difference(Expr::all(), Expr::none());
        let expr = Expr::intersection(expr, Expr::all());
        assert_eq!(expr, parse("all() - none() & all()"));

        let expr = Expr::intersection(Expr::all(), Expr::none());
        let expr = Expr::difference(expr, Expr::all());
        assert_eq!(expr, parse("all() & none() - all()"));

        let expr = Expr::intersection(Expr::none(), Expr::all()).not();
        assert_eq!(expr, parse("not (none() & all())"));
    }

    #[test]
    fn test_parse_comma() {
        // accept escaped comma
        let expr = Expr::Set(SetDef::Test(
            NameMatcher::Contains("a,".to_string()),
            (5, 3).into(),
        ));
        assert_eq!(expr, parse(r"test(a\,)"));

        // string parsing is compatible with possible future syntax
        fn parse_future_syntax(input: Span) -> IResult<(Option<NameMatcher>, Option<NameMatcher>)> {
            let (i, _) = tag("something")(input)?;
            let (i, _) = char('(')(i)?;
            let (i, n1) = set_matcher(NameMatcher::Contains)(i)?;
            let (i, _) = ws(char(','))(i)?;
            let (i, n2) = set_matcher(NameMatcher::Contains)(i)?;
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
        ($error:ident, $name:ident, $start:literal, $end:literal) => {
            assert!(matches!($error, ParseSingleError::$name(span) if span == ($start, $end).into()));
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
            other => panic!("expected invalid regex with details, found {}", other),
        };
        assert_eq!(span, (12, 1).into(), "span matches");
        assert_eq!(message, "unclosed group");
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
        assert_eq!(2, errors.len(), "{:?}", errors);
        let error = errors.remove(0);
        assert_error!(error, ExpectedOpenParenthesis, 3, 0);
        let error = errors.remove(0);
        assert_error!(error, ExpectedCloseRegex, 19, 0);
    }
}
