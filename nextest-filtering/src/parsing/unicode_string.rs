// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Adapted from https://github.com/Geal/nom/blob/294ffb3d9e0ade2c3b7ddfff52484b6d643dcce1/examples/string.rs

use super::{expect_n, IResult, Span, SpanLength};
use crate::errors::ParseSingleError;
use nom::{
    branch::alt,
    bytes::complete::{is_not, take_while_m_n},
    character::complete::char,
    combinator::{map, map_opt, map_res, value, verify},
    multi::fold_many0,
    sequence::{delimited, preceded},
    stream::SliceLen,
    stream::Stream,
    trace::trace,
    Parser,
};
use std::fmt;
use winnow as nom;

fn run_str_parser<'a, T, I>(mut inner: I) -> impl FnMut(Span<'a>) -> IResult<'a, T>
where
    I: FnMut(&'a str) -> nom::IResult<&'a str, T>,
{
    move |input| match inner(input.next_slice(input.slice_len()).1) {
        Ok((i, res)) => {
            let eaten = input.slice_len() - i.len();
            Ok((input.next_slice(eaten).0, res))
        }
        Err(nom::Err::Backtrack(err)) => {
            let nom::error::Error { input: i, kind } = err;
            let eaten = input.slice_len() - i.len();
            let err = nom::error::Error {
                input: input.next_slice(eaten).0,
                kind,
            };
            Err(nom::Err::Backtrack(err))
        }
        Err(nom::Err::Cut(err)) => {
            let nom::error::Error { input: i, kind } = err;
            let eaten = input.slice_len() - i.len();
            let err = nom::error::Error {
                input: input.next_slice(eaten).0,
                kind,
            };
            Err(nom::Err::Cut(err))
        }
        Err(nom::Err::Incomplete(err)) => Err(nom::Err::Incomplete(err)),
    }
}

fn parse_unicode(input: Span<'_>) -> IResult<'_, char> {
    trace("parse_unicode", |input| {
        let parse_hex = take_while_m_n(1, 6, |c: char| c.is_ascii_hexdigit());
        let parse_delimited_hex = preceded(char('u'), delimited(char('{'), parse_hex, char('}')));
        let parse_u32 = map_res(parse_delimited_hex, |hex| u32::from_str_radix(hex, 16));
        run_str_parser(map_opt(parse_u32, std::char::from_u32))(input)
    })
    .parse_next(input)
}

fn parse_escaped_char(input: Span<'_>) -> IResult<'_, Option<char>> {
    trace("parse_escaped_char", |input| {
        let valid = alt((
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
        ));
        preceded(
            char('\\'),
            // If none of the valid characters are found, this will report an error.
            expect_n(
                valid,
                ParseSingleError::InvalidEscapeCharacter,
                // -1 to account for the preceding backslash.
                SpanLength::Offset(-1, 2),
            ),
        )(input)
    })
    .parse_next(input)
}

// This should match parse_escaped_char above.
pub(crate) struct DisplayParsedString<'a>(pub(crate) &'a str);

impl fmt::Display for DisplayParsedString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            match c {
                // These escapes are custom to nextest.
                '/' => f.write_str("\\/")?,
                ')' => f.write_str("\\)")?,
                ',' => f.write_str("\\,")?,
                // All the other escapes should be covered by this.
                c => write!(f, "{}", c.escape_default())?,
            }
        }
        Ok(())
    }
}
fn parse_literal<'i>(input: Span<'i>) -> IResult<'i, &str> {
    trace("parse_literal", |input: Span<'i>| {
        let not_quote_slash = is_not(",)\\");
        let res = verify(not_quote_slash, |s: &str| !s.is_empty())(input.clone());
        res
    })
    .parse_next(input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFragment<'a> {
    Literal(&'a str),
    EscapedChar(char),
}

fn parse_fragment(input: Span<'_>) -> IResult<'_, Option<StringFragment<'_>>> {
    trace(
        "parse_fragment",
        alt((
            map(parse_literal, |span| Some(StringFragment::Literal(span))),
            map(parse_escaped_char, |res| {
                res.map(StringFragment::EscapedChar)
            }),
        )),
    )
    .parse_next(input)
}

/// Construct a string by consuming the input until the next unescaped ) or ,.
///
/// Returns None if the string isn't valid.
pub(super) fn parse_string(input: Span<'_>) -> IResult<'_, Option<String>> {
    trace(
        "parse_string",
        fold_many0(
            parse_fragment,
            || Some(String::new()),
            |string, fragment| {
                match (string, fragment) {
                    (Some(mut string), Some(StringFragment::Literal(s))) => {
                        string.push_str(s);
                        Some(string)
                    }
                    (Some(mut string), Some(StringFragment::EscapedChar(c))) => {
                        string.push(c);
                        Some(string)
                    }
                    (Some(_), None) => {
                        // We encountered a parsing error, and at this point we'll stop returning
                        // values.
                        None
                    }
                    (None, _) => None,
                }
            },
        ),
    )
    .parse_next(input)
}
