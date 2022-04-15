// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Adapted from https://github.com/Geal/nom/blob/294ffb3d9e0ade2c3b7ddfff52484b6d643dcce1/examples/string.rs

use nom::{
    branch::alt,
    bytes::streaming::{is_not, take_while_m_n},
    character::streaming::char,
    combinator::{map, map_opt, map_res, value, verify},
    multi::fold_many0,
    sequence::{delimited, preceded},
    Slice,
};
use nom_tracable::tracable_parser;

use super::{IResult, Span};

fn run_str_parser<'a, T, I>(mut inner: I) -> impl FnMut(Span<'a>) -> IResult<'a, T>
where
    I: FnMut(&'a str) -> nom::IResult<&'a str, T>,
{
    move |input| match inner(input.fragment()) {
        Ok((i, res)) => {
            let eaten = input.fragment().len() - i.len();
            Ok((input.slice(eaten..), res))
        }
        Err(nom::Err::Error(err)) => {
            let nom::error::Error { input: i, code } = err;
            let eaten = input.fragment().len() - i.len();
            let err = nom::error::Error {
                input: input.slice(eaten..),
                code,
            };
            Err(nom::Err::Error(err))
        }
        Err(nom::Err::Failure(err)) => {
            let nom::error::Error { input: i, code } = err;
            let eaten = input.fragment().len() - i.len();
            let err = nom::error::Error {
                input: input.slice(eaten..),
                code,
            };
            Err(nom::Err::Failure(err))
        }
        Err(nom::Err::Incomplete(err)) => Err(nom::Err::Incomplete(err)),
    }
}

#[tracable_parser]
fn parse_unicode(input: Span) -> IResult<char> {
    let parse_hex = take_while_m_n(1, 6, |c: char| c.is_ascii_hexdigit());
    let parse_delimited_hex = preceded(char('u'), delimited(char('{'), parse_hex, char('}')));
    let parse_u32 = map_res(parse_delimited_hex, |hex| u32::from_str_radix(hex, 16));
    run_str_parser(map_opt(parse_u32, std::char::from_u32))(input)
}

#[tracable_parser]
fn parse_escaped_char(input: Span) -> IResult<char> {
    preceded(
        char('\\'),
        alt((
            parse_unicode,
            value('\n', char('n')),
            value('\r', char('r')),
            value('\t', char('t')),
            value('\u{08}', char('b')),
            value('\u{0C}', char('f')),
            value('\\', char('\\')),
            value('/', char('/')),
            value(')', char(')')),
            value(',', char(',')),
        )),
    )(input)
}

#[tracable_parser]
fn parse_literal(input: Span) -> IResult<Span> {
    let not_quote_slash = is_not(",)\\");
    verify(not_quote_slash, |s: &Span| !s.fragment().is_empty())(input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFragment<'a> {
    Literal(&'a str),
    EscapedChar(char),
}

#[tracable_parser]
fn parse_fragment(input: Span) -> IResult<StringFragment<'_>> {
    alt((
        map(parse_literal, |span| {
            StringFragment::Literal(span.fragment())
        }),
        map(parse_escaped_char, StringFragment::EscapedChar),
    ))(input)
}

/// Construct a string by consuming the input until the next unescaped `'`
///
/// Return Err(Incomplete(1)) if not ending `'` is found
#[tracable_parser]
pub(super) fn parse_string(input: Span) -> IResult<String> {
    fold_many0(parse_fragment, String::new, |mut string, fragment| {
        match fragment {
            StringFragment::Literal(s) => string.push_str(s),
            StringFragment::EscapedChar(c) => string.push(c),
        }
        string
    })(input)
}
