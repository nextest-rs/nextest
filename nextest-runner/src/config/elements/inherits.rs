// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Inherit settings for profiles
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Inherits(Option<String>);

impl Inherits {
    /// Creates a new `Inherits`.
    pub fn new(inherits: Option<String>) -> Self {
        Self(inherits)
    }

    /// Returns the profile that the custom profile inherits from
    pub fn inherits_from(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

// TODO: Need to write test cases  for this
