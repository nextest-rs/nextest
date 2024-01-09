// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Adapted from https://github.com/Geal/nom/blob/294ffb3d9e0ade2c3b7ddfff52484b6d643dcce1/examples/string.rs

use super::{expect_n, IResult, Span, SpanLength};
use crate::errors::ParseSingleError;
use std::fmt;
use winnow::{
    combinator::{alt, delimited, fold_repeat, preceded},
    stream::SliceLen,
    stream::Stream,
    token::{take_till, take_while},
    trace::trace,
    unpeek, Parser,
};

fn run_str_parser<'a, T, I>(mut inner: I) -> impl Parser<Span<'a>, T, super::Error<'a>>
where
    I: Parser<&'a str, T, winnow::error::InputError<&'a str>>,
{
    unpeek(
        move |input: Span<'a>| match inner.parse_peek(input.peek_slice(input.slice_len()).1) {
            Ok((i, res)) => {
                let eaten = input.slice_len() - i.len();
                Ok((input.peek_slice(eaten).0, res))
            }
            Err(winnow::error::ErrMode::Backtrack(err)) => {
                let winnow::error::InputError { input: i, kind } = err;
                let eaten = input.slice_len() - i.len();
                let err = winnow::error::InputError {
                    input: input.peek_slice(eaten).0,
                    kind,
                };
                Err(winnow::error::ErrMode::Backtrack(err))
            }
            Err(winnow::error::ErrMode::Cut(err)) => {
                let winnow::error::InputError { input: i, kind } = err;
                let eaten = input.slice_len() - i.len();
                let err = winnow::error::InputError {
                    input: input.peek_slice(eaten).0,
                    kind,
                };
                Err(winnow::error::ErrMode::Cut(err))
            }
            Err(winnow::error::ErrMode::Incomplete(err)) => {
                Err(winnow::error::ErrMode::Incomplete(err))
            }
        },
    )
}

fn parse_unicode(input: Span<'_>) -> IResult<'_, char> {
    trace(
        "parse_unicode",
        unpeek(|input| {
            let parse_hex = take_while(1..=6, |c: char| c.is_ascii_hexdigit());
            let parse_delimited_hex = preceded('u', delimited('{', parse_hex, '}'));
            let parse_u32 = parse_delimited_hex.try_map(|hex| u32::from_str_radix(hex, 16));
            run_str_parser(parse_u32.verify_map(std::char::from_u32)).parse_peek(input)
        }),
    )
    .parse_peek(input)
}

fn parse_escaped_char(input: Span<'_>) -> IResult<'_, Option<char>> {
    trace(
        "parse_escaped_char",
        unpeek(|input| {
            let valid = alt((
                unpeek(parse_unicode),
                'n'.value('\n'),
                'r'.value('\r'),
                't'.value('\t'),
                'b'.value('\u{08}'),
                'f'.value('\u{0C}'),
                '\\'.value('\\'),
                '/'.value('/'),
                ')'.value(')'),
                ','.value(','),
            ));
            preceded(
                '\\',
                // If none of the valid characters are found, this will report an error.
                expect_n(
                    valid,
                    ParseSingleError::InvalidEscapeCharacter,
                    // -1 to account for the preceding backslash.
                    SpanLength::Offset(-1, 2),
                ),
            )
            .parse_peek(input)
        }),
    )
    .parse_peek(input)
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
    trace(
        "parse_literal",
        unpeek(|input: Span<'i>| {
            let not_quote_slash = take_till(1.., (',', ')', '\\'));
            let res = not_quote_slash
                .verify(|s: &str| !s.is_empty())
                .parse_peek(input.clone());
            res
        }),
    )
    .parse_peek(input)
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
            unpeek(parse_literal).map(|span| Some(StringFragment::Literal(span))),
            unpeek(parse_escaped_char).map(|res| res.map(StringFragment::EscapedChar)),
        )),
    )
    .parse_peek(input)
}

/// Construct a string by consuming the input until the next unescaped ) or ,.
///
/// Returns None if the string isn't valid.
pub(super) fn parse_string(input: Span<'_>) -> IResult<'_, Option<String>> {
    trace(
        "parse_string",
        fold_repeat(
            0..,
            unpeek(parse_fragment),
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
    .parse_peek(input)
}
