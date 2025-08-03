// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::{Deserialize, Deserializer};

/// Tracks if a particular default is implicit.
///
/// The `default` impl can be passed in to `serde` directly, if desired.
///
/// This could be its own crate maybe?
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackDefault<T> {
    /// The deserialized or default value.
    pub value: T,
    /// Whether the value was deserialized.
    pub is_deserialized: bool,
}

impl<T> TrackDefault<T> {
    pub fn with_default_value(value: T) -> Self {
        Self {
            value,
            is_deserialized: false,
        }
    }

    pub fn with_deserialized_value(value: T) -> Self {
        Self {
            value,
            is_deserialized: true,
        }
    }
}

impl<'de, T> Deserialize<'de> for TrackDefault<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Self::with_deserialized_value(T::deserialize(deserializer)?);
        Ok(value)
    }
}
