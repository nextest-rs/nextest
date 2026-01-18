// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User-level experimental features.
//!
//! These features are configured in the user config file (`~/.config/nextest/config.toml`)
//! or via environment variables. They are separate from the repository-level experimental
//! features in [`ConfigExperimental`](crate::config::core::ConfigExperimental).

use serde::Deserialize;
use std::{collections::BTreeSet, env, fmt, str::FromStr};

/// Deserialized experimental config from user config file.
///
/// This represents the `[experimental]` table in user config:
///
/// ```toml
/// [experimental]
/// record = true
/// ```
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ExperimentalConfig {
    /// Enable recording of test runs.
    #[serde(default)]
    pub record: bool,
}

impl ExperimentalConfig {
    /// Converts to a set of enabled experimental features.
    pub fn to_set(self) -> BTreeSet<UserConfigExperimental> {
        let Self { record } = self;
        let mut set = BTreeSet::new();
        if record {
            set.insert(UserConfigExperimental::Record);
        }
        set
    }
}

/// User-level experimental features.
///
/// These features can be enabled in the user config file or via environment variables.
/// Unlike repository-level experimental features, these are personal preferences that
/// aren't version-controlled with the project.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[non_exhaustive]
pub enum UserConfigExperimental {
    /// Enable recording of test runs.
    Record,
}

impl UserConfigExperimental {
    /// Returns the environment variable name that enables this feature.
    pub fn env_var(&self) -> &'static str {
        match self {
            Self::Record => "NEXTEST_EXPERIMENTAL_RECORD",
        }
    }

    /// Returns the feature name as used in configuration.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Record => "record",
        }
    }

    /// Returns all known experimental features.
    pub fn all() -> &'static [Self] {
        &[Self::Record]
    }

    /// Returns the set of experimental features enabled via environment variables.
    ///
    /// A feature is enabled if its corresponding environment variable is set to "1".
    pub fn from_env() -> BTreeSet<Self> {
        Self::all()
            .iter()
            .filter(|feature| {
                env::var(feature.env_var())
                    .map(|v| v == "1")
                    .unwrap_or(false)
            })
            .copied()
            .collect()
    }
}

impl fmt::Display for UserConfigExperimental {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for UserConfigExperimental {
    type Err = UnknownUserExperimentalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "record" => Ok(Self::Record),
            _ => Err(UnknownUserExperimentalError {
                feature: s.to_owned(),
            }),
        }
    }
}

/// Error returned when parsing an unknown experimental feature name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownUserExperimentalError {
    /// The unknown feature name.
    pub feature: String,
}

impl fmt::Display for UnknownUserExperimentalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown experimental feature `{}`; known features: {}",
            self.feature,
            UserConfigExperimental::all()
                .iter()
                .map(|f| f.name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownUserExperimentalError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str() {
        assert_eq!(
            "record".parse::<UserConfigExperimental>().unwrap(),
            UserConfigExperimental::Record
        );

        assert!("unknown".parse::<UserConfigExperimental>().is_err());
    }

    #[test]
    fn test_display() {
        assert_eq!(UserConfigExperimental::Record.to_string(), "record");
    }

    #[test]
    fn test_env_var() {
        assert_eq!(
            UserConfigExperimental::Record.env_var(),
            "NEXTEST_EXPERIMENTAL_RECORD"
        );
    }
}
