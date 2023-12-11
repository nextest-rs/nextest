// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Glob matching.

use super::{parse_matcher_text, IResult, Span};
use crate::{
    errors::{GlobConstructError, ParseSingleError},
    NameMatcher,
};
use nom_tracable::tracable_parser;

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
#[tracable_parser]
pub(super) fn parse_glob(input: Span<'_>, implicit: bool) -> IResult<'_, Option<NameMatcher>> {
    let (i, res) = match parse_matcher_text(input.clone()) {
        Ok((i, res)) => (i, res),
        Err(_) => {
            unreachable!("parse_matcher_text should never fail")
        }
    };

    let Some(parsed_value) = res else {
        return Ok((i, None));
    };

    match GenericGlob::new(parsed_value) {
        Ok(glob) => Ok((i, Some(NameMatcher::Glob { glob, implicit }))),
        Err(error) => {
            let start = input.location_offset();
            let end = i.location_offset();
            let err = ParseSingleError::InvalidGlob {
                span: (start, end - start).into(),
                error,
            };
            i.extra.report_error(err);
            Ok((i, None))
        }
    }
}
