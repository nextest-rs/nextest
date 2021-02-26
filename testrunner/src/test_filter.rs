// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use aho_corasick::AhoCorasick;

/// A filter for tests.
#[derive(Clone, Debug)]
pub struct TestFilter {
    inner: TestFilterInner,
}

#[derive(Clone, Debug)]
enum TestFilterInner {
    MatchAll,
    MatchSet(Box<AhoCorasick>),
}

impl TestFilter {
    /// Creates a new `TestFilter` from the given patterns.
    ///
    /// If an empty slice is passed, the test filter matches all possible test names.
    pub fn new(patterns: &[impl AsRef<[u8]>]) -> Self {
        let inner = if patterns.is_empty() {
            TestFilterInner::MatchAll
        } else {
            TestFilterInner::MatchSet(Box::new(AhoCorasick::new_auto_configured(patterns)))
        };
        Self { inner }
    }

    /// Creates a new `TestFilter` that matches any pattern.
    pub fn any() -> Self {
        Self {
            inner: TestFilterInner::MatchAll,
        }
    }

    /// Matches the given string in this set.
    pub fn is_match(&self, test_name: &str) -> bool {
        match &self.inner {
            TestFilterInner::MatchAll => true,
            TestFilterInner::MatchSet(set) => set.is_match(test_name),
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
            let test_filter = TestFilter::new(patterns);
            for test_name in test_names {
                prop_assert!(test_filter.is_match(&test_name));
            }
        }

        // Test that exact names match.
        #[test]
        fn proptest_exact(test_names in vec(any::<String>(), 0..16)) {
            let test_filter = TestFilter::new(&test_names);
            for test_name in test_names {
                prop_assert!(test_filter.is_match(&test_name));
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

            let test_filter = TestFilter::new(&patterns);
            for test_name in test_names {
                prop_assert!(test_filter.is_match(&test_name));
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
            let test_filter = TestFilter::new(&[&pattern]);
            prop_assert!(!test_filter.is_match(&substring));
        }
    }
}
