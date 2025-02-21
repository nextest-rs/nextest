// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Adapted from https://github.com/Geal/nom/blob/294ffb3d9e0ade2c3b7ddfff52484b6d643dcce1/examples/string.rs

use super::{PResult, Span, SpanLength, expect_n};
use crate::errors::ParseSingleError;
use std::fmt;
use winnow::{
    Parser,
    combinator::{alt, delimited, preceded, repeat, trace},
    token::{take_till, take_while},
};

fn parse_unicode(input: &mut Span<'_>) -> PResult<char> {
    trace("parse_unicode", |input: &mut _| {
        let parse_hex = take_while(1..=6, |c: char| c.is_ascii_hexdigit());
        let parse_delimited_hex = preceded('u', delimited('{', parse_hex, '}'));
        let parse_u32 = parse_delimited_hex.try_map(|hex| u32::from_str_radix(hex, 16));
        parse_u32.verify_map(std::char::from_u32).parse_next(input)
    })
    .parse_next(input)
}

fn parse_escaped_char(input: &mut Span<'_>) -> PResult<Option<char>> {
    trace("parse_escaped_char", |input: &mut _| {
        let valid = alt((
            parse_unicode,
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
        .parse_next(input)
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
fn parse_literal<'i>(input: &mut Span<'i>) -> PResult<&'i str> {
    trace("parse_literal", |input: &mut _| {
        let not_quote_slash = take_till(1.., (',', ')', '\\'));

        not_quote_slash
            .verify(|s: &str| !s.is_empty())
            .parse_next(input)
    })
    .parse_next(input)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFragment<'a> {
    Literal(&'a str),
    EscapedChar(char),
}

fn parse_fragment<'i>(input: &mut Span<'i>) -> PResult<Option<StringFragment<'i>>> {
    trace(
        "parse_fragment",
        alt((
            parse_literal.map(|span| Some(StringFragment::Literal(span))),
            parse_escaped_char.map(|res| res.map(StringFragment::EscapedChar)),
        )),
    )
    .parse_next(input)
}

/// Construct a string by consuming the input until the next unescaped ) or ,.
///
/// Returns None if the string isn't valid.
pub(super) fn parse_string(input: &mut Span<'_>) -> PResult<Option<String>> {
    trace(
        "parse_string",
        repeat(0.., parse_fragment).fold(
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
