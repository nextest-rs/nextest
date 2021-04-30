// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use aho_corasick::AhoCorasick;
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Whether to run ignored tests.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RunIgnored {
    /// Only run tests that aren't ignored.
    ///
    /// This is the default.
    Default,

    /// Only run tests that are ignored.
    IgnoredOnly,

    /// Run both ignored and non-ignored tests.
    All,
}

impl RunIgnored {
    pub fn variants() -> [&'static str; 3] {
        ["default", "ignored-only", "all"]
    }
}

impl Default for RunIgnored {
    fn default() -> Self {
        RunIgnored::Default
    }
}

impl fmt::Display for RunIgnored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunIgnored::Default => write!(f, "default"),
            RunIgnored::IgnoredOnly => write!(f, "ignored-only"),
            RunIgnored::All => write!(f, "all"),
        }
    }
}

impl FromStr for RunIgnored {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "default" => RunIgnored::Default,
            "ignored-only" => RunIgnored::IgnoredOnly,
            "all" => RunIgnored::All,
            other => bail!("unrecognized value for run-ignored: {}", other),
        };
        Ok(val)
    }
}

/// A filter for tests.
#[derive(Clone, Debug)]
pub struct TestFilter {
    run_ignored: RunIgnored,
    name_match: NameMatch,
}

#[derive(Clone, Debug)]
enum NameMatch {
    MatchAll,
    MatchSet(Box<AhoCorasick>),
}

impl TestFilter {
    /// Creates a new `TestFilter` from the given patterns.
    ///
    /// If an empty slice is passed, the test filter matches all possible test names.
    pub fn new(run_ignored: RunIgnored, patterns: &[impl AsRef<[u8]>]) -> Self {
        let name_match = if patterns.is_empty() {
            NameMatch::MatchAll
        } else {
            NameMatch::MatchSet(Box::new(AhoCorasick::new_auto_configured(patterns)))
        };
        Self {
            run_ignored,
            name_match,
        }
    }

    /// Creates a new `TestFilter` that matches any pattern by name.
    pub fn any(run_ignored: RunIgnored) -> Self {
        Self {
            run_ignored,
            name_match: NameMatch::MatchAll,
        }
    }

    /// Returns an enum describing the match status of this filter.
    pub fn filter_match(&self, test_name: &str, ignored: bool) -> FilterMatch {
        match self.run_ignored {
            RunIgnored::IgnoredOnly => {
                if !ignored {
                    return FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    };
                }
            }
            RunIgnored::Default => {
                if ignored {
                    return FilterMatch::Mismatch {
                        reason: MismatchReason::Ignored,
                    };
                }
            }
            _ => {}
        };

        let string_match = match &self.name_match {
            NameMatch::MatchAll => true,
            NameMatch::MatchSet(set) => set.is_match(test_name),
        };
        if string_match {
            FilterMatch::Matches
        } else {
            FilterMatch::Mismatch {
                reason: MismatchReason::String,
            }
        }
    }
}

/// An enum describing whether a test matches a filter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum FilterMatch {
    /// This test matches this filter.
    Matches,

    /// This test does not match this filter.
    ///
    /// The `MismatchReason` inside describes the reason this filter isn't matched.
    Mismatch { reason: MismatchReason },
}

impl FilterMatch {
    /// Returns true if the filter doesn't match.
    pub fn is_match(&self) -> bool {
        matches!(self, FilterMatch::Matches)
    }
}

/// The reason for why a test doesn't match a filter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MismatchReason {
    /// This test does not match the run-ignored option in the filter.
    Ignored,

    /// This test does not match the provided string filters.
    String,
}

impl fmt::Display for MismatchReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MismatchReason::Ignored => write!(f, "does not match the run-ignored option"),
            MismatchReason::String => write!(f, "does not match the provided string filters"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::vec, prelude::*};

    proptest! {
        #[test]
        fn proptest_empty(test_names in vec(any::<String>(), 0..16)) {
            let patterns: &[String] = &[];
            let test_filter = TestFilter::new(RunIgnored::Default, patterns);
            for test_name in test_names {
                prop_assert!(test_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that exact names match.
        #[test]
        fn proptest_exact(test_names in vec(any::<String>(), 0..16)) {
            let test_filter = TestFilter::new(RunIgnored::Default, &test_names);
            for test_name in test_names {
                prop_assert!(test_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that substrings match.
        #[test]
        fn proptest_substring(
            substring_prefix_suffixes in vec([any::<String>(); 3], 0..16),
        ) {
            let mut patterns = Vec::with_capacity(substring_prefix_suffixes.len());
            let mut test_names = Vec::with_capacity(substring_prefix_suffixes.len());
            for [substring, prefix, suffix] in substring_prefix_suffixes {
                test_names.push(prefix + &substring + &suffix);
                patterns.push(substring);
            }

            let test_filter = TestFilter::new(RunIgnored::Default, &patterns);
            for test_name in test_names {
                prop_assert!(test_filter.filter_match(&test_name, false).is_match());
            }
        }

        // Test that dropping a character from a string doesn't match.
        #[test]
        fn proptest_no_match(
            substring in any::<String>(),
            prefix in any::<String>(),
            suffix in any::<String>(),
        ) {
            prop_assume!(!substring.is_empty() && !(prefix.is_empty() && suffix.is_empty()));
            let pattern = prefix + &substring + &suffix;
            let test_filter = TestFilter::new(RunIgnored::Default, &[&pattern]);
            prop_assert!(!test_filter.filter_match(&substring, false).is_match());
        }
    }
}
