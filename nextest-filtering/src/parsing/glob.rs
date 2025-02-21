// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Glob matching.

use super::{Error, Span, parse_matcher_text};
use crate::{
    NameMatcher,
    errors::{GlobConstructError, ParseSingleError},
};
use winnow::{ModalParser, Parser, combinator::trace, stream::Location};

/// A glob pattern.
///
/// We do not use `globset::GlobMatcher` directly because it has path-like semantics, so we use
/// regexes directly.
#[derive(Clone, Debug)]
pub struct GenericGlob {
    /// The glob string.
    glob_str: String,

    /// The regex to match against.
    regex: regex::bytes::Regex,
}

impl GenericGlob {
    /// Creates a new generic glob.
    pub(crate) fn new(glob_str: String) -> Result<Self, GlobConstructError> {
        let glob = globset::GlobBuilder::new(&glob_str)
            // Only allow escapes via [].
            .backslash_escape(false)
            // Allow foo.{exe,} to match both foo.exe and foo.
            .empty_alternates(true)
            .build()
            .map_err(GlobConstructError::InvalidGlob)?;

        // Convert to a regex.
        let regex = regex::bytes::Regex::new(glob.regex())
            .map_err(|error| GlobConstructError::RegexError(error.to_string()))?;

        Ok(Self { glob_str, regex })
    }

    /// Returns the glob string.
    pub fn as_str(&self) -> &str {
        &self.glob_str
    }

    /// Returns the regex that will be used for matches.
    pub fn regex(&self) -> &regex::bytes::Regex {
        &self.regex
    }

    /// Returns true if this glob matches the given string.
    pub fn is_match(&self, s: &str) -> bool {
        self.regex.is_match(s.as_bytes())
    }
}

// This never returns Err(()) -- instead, it reports an error to the parsing state.
pub(super) fn parse_glob<'i>(
    implicit: bool,
) -> impl ModalParser<Span<'i>, Option<NameMatcher>, Error> {
    trace("parse_glob", move |input: &mut Span<'i>| {
        let start = input.current_token_start();
        let res = match parse_matcher_text.parse_next(input) {
            Ok(res) => res,
            Err(_) => {
                unreachable!("parse_matcher_text should never fail")
            }
        };

        let Some(parsed_value) = res else {
            return Ok(None);
        };

        match GenericGlob::new(parsed_value) {
            Ok(glob) => Ok(Some(NameMatcher::Glob { glob, implicit })),
            Err(error) => {
                let end = input.current_token_start();
                let err = ParseSingleError::InvalidGlob {
                    span: (start, end - start).into(),
                    error,
                };
                input.state.report_error(err);
                Ok(None)
            }
        }
    })
}
