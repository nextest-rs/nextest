// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::NameMatcher;

use nom::{
    branch::alt,
    bytes::complete::{tag, take_until1, take_while1},
    character::complete::char,
    combinator::{eof, map, map_res, recognize, success},
    multi::{fold_many0, many0},
    sequence::{delimited, pair, preceded, terminated},
};
use nom_tracable::tracable_parser;
use unicode_xid::UnicodeXID;

pub type Span<'a> = nom_locate::LocatedSpan<&'a str, nom_tracable::TracableInfo>;
type IResult<'a, T> = nom::IResult<Span<'a>, T>;

/// Define a set of tests
#[cfg_attr(test, derive(PartialEq, Eq))]
#[derive(Debug)]
pub enum SetDef {
    /// All tests in a package
    Package(NameMatcher),
    /// All tests in a package dependencies
    Deps(NameMatcher),
    /// All tests in a package reverse dependencies
    Rdeps(NameMatcher),
    /// All tests matching a name
    Test(NameMatcher),
    /// All tests
    All,
    /// No tests
    None,
    // Possible addition: Binary(NameMatcher)
}

/// Filtering expression
///
/// Used to filter tests to run.
#[cfg_attr(test, derive(PartialEq, Eq))]
#[derive(Debug)]
pub enum Expr {
    /// Accepts every tests not in the given expression
    Not(Box<Expr>),
    /// Accepts every tests in either given expression
    Union(Box<Expr>, Box<Expr>),
    /// Accepts every tests in both given expression
    Intersection(Box<Expr>, Box<Expr>),
    /// Accepts every tests in a set
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

fn ws<'a, T, P: FnMut(Span<'a>) -> IResult<'a, T>>(
    inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, T> {
    preceded(many0(char(' ')), inner)
}

fn parentheses<'a, T, P: FnMut(Span<'a>) -> IResult<'a, T>>(
    inner: P,
) -> impl FnMut(Span<'a>) -> IResult<'a, T> {
    delimited(ws(char('(')), inner, ws(char(')')))
}

#[tracable_parser]
fn parse_identifier_part(input: Span) -> IResult<String> {
    // This is use for NameMatcher::Contains(_) and NameMatcher::Equal(_)
    // The output should be valid part of a test-name or a package name.
    map(
        recognize(take_while1(|c: char| c == ':' || c.is_xid_continue())),
        |res: Span| res.fragment().to_string(),
    )(input)
}

#[tracable_parser]
fn parse_contains_matcher(input: Span) -> IResult<NameMatcher> {
    ws(map(parse_identifier_part, |res: String| {
        NameMatcher::Contains(res)
    }))(input)
}

#[tracable_parser]
fn parse_equal_matcher(input: Span) -> IResult<NameMatcher> {
    ws(map(
        preceded(char('='), parse_identifier_part),
        |res: String| NameMatcher::Equal(res),
    ))(input)
}

#[tracable_parser]
fn parse_regex_matcher(input: Span) -> IResult<NameMatcher> {
    ws(map_res(
        delimited(char('/'), recognize(take_until1("/")), char('/')),
        |res: Span| regex::Regex::new(res.fragment()).map(NameMatcher::Regex),
    ))(input)
}

#[tracable_parser]
fn parse_set_matcher(input: Span) -> IResult<NameMatcher> {
    ws(alt((
        parse_regex_matcher,
        parse_equal_matcher,
        parse_contains_matcher,
    )))(input)
}

fn nullary_set_def(
    name: &'static str,
    make_set: fn() -> SetDef,
) -> impl FnMut(Span) -> IResult<SetDef> {
    move |input| {
        map(preceded(tag(name), parentheses(success(()))), |_| {
            make_set()
        })(input)
    }
}

fn unary_set_def(
    name: &'static str,
    make_set: fn(NameMatcher) -> SetDef,
) -> impl FnMut(Span) -> IResult<SetDef> {
    move |input| {
        map(preceded(tag(name), parentheses(parse_set_matcher)), |m| {
            make_set(m)
        })(input)
    }
}

#[tracable_parser]
#[allow(unused)]
fn parse_set_def(input: Span) -> IResult<SetDef> {
    ws(alt((
        unary_set_def("package", SetDef::Package),
        unary_set_def("deps", SetDef::Deps),
        unary_set_def("rdeps", SetDef::Rdeps),
        unary_set_def("test", SetDef::Test),
        nullary_set_def("all", || SetDef::All),
        nullary_set_def("none", || SetDef::None),
    )))(input)
}

#[tracable_parser]
fn parse_expr_not(input: Span) -> IResult<Expr> {
    map(
        preceded(alt((tag("not "), tag("!"))), ws(parse_basic_expr)),
        |e| Expr::Not(Box::new(e)),
    )(input)
}

#[tracable_parser]
fn parse_basic_expr(input: Span) -> IResult<Expr> {
    ws(alt((
        map(parse_set_def, Expr::Set),
        parse_expr_not,
        parentheses(parse_expr),
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
    let (input, expr) = parse_basic_expr(input)?;

    let (input, ops) = fold_many0(
        pair(parse_operator, parse_basic_expr),
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

pub fn parse_expression(input: Span) -> Result<Expr, nom::Err<nom::error::Error<Span>>> {
    let (_, expr) = terminated(parse_expr, ws(eof))(input)?;
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn parse_set(input: &str) -> SetDef {
        let info = nom_tracable::TracableInfo::new()
            .forward(true)
            .backward(true);
        parse_set_def(Span::new_extra(input, info)).unwrap().1
    }

    #[test]
    fn test_parse_name_matcher() {
        assert_eq!(
            SetDef::Test(NameMatcher::Contains("something".to_string())),
            parse_set("test(something)")
        );
        assert_eq!(
            SetDef::Test(NameMatcher::Equal("something".to_string())),
            parse_set("test(=something)")
        );
        assert_eq!(
            SetDef::Test(NameMatcher::Regex(regex::Regex::new("some.*").unwrap())),
            parse_set("test(/some.*/)")
        );
    }

    #[test]
    fn test_parse_set_def() {
        assert_eq!(SetDef::All, parse_set("all()"));
        assert_eq!(SetDef::All, parse_set(" all ( ) "));

        assert_eq!(SetDef::None, parse_set("none()"));

        assert_eq!(
            SetDef::Package(NameMatcher::Contains("something".to_string())),
            parse_set("package(something)")
        );
        assert_eq!(
            SetDef::Deps(NameMatcher::Contains("something".to_string())),
            parse_set("deps(something)")
        );
        assert_eq!(
            SetDef::Rdeps(NameMatcher::Contains("something".to_string())),
            parse_set("rdeps(something)")
        );
        assert_eq!(
            SetDef::Test(NameMatcher::Contains("something".to_string())),
            parse_set("test(something)")
        );
    }

    #[track_caller]
    fn parse(input: &str) -> Expr {
        let info = nom_tracable::TracableInfo::new()
            .forward(true)
            .backward(true);
        parse_expression(Span::new_extra(input, info)).unwrap()
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
}
