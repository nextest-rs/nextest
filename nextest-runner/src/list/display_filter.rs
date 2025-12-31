// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use nextest_metadata::{RustBinaryId, TestCaseName};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default)]
pub(crate) struct TestListDisplayFilter<'list> {
    // This is a map of paths to the matching tests in those paths. The matching tests are stored as
    // a set of test names.
    // Invariant: hashset values are never empty.
    map: HashMap<&'list RustBinaryId, HashSet<&'list TestCaseName>>,
}

impl<'list> TestListDisplayFilter<'list> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(
        &mut self,
        binary_id: &'list RustBinaryId,
        test_name: &'list TestCaseName,
    ) {
        self.map.entry(binary_id).or_default().insert(test_name);
    }

    pub(crate) fn test_count(&self) -> usize {
        self.map.values().map(|set| set.len()).sum()
    }

    pub(crate) fn matcher_for(
        &self,
        binary_id: &'list RustBinaryId,
    ) -> Option<DisplayFilterMatcher<'list, '_>> {
        self.map.get(binary_id).map(DisplayFilterMatcher::Some)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DisplayFilterMatcher<'list, 'filter> {
    All,
    Some(&'filter HashSet<&'list TestCaseName>),
}

impl DisplayFilterMatcher<'_, '_> {
    pub(crate) fn is_match(&self, test_name: &TestCaseName) -> bool {
        match self {
            Self::All => true,
            Self::Some(set) => set.contains(test_name),
        }
    }
}
