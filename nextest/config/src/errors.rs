// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use std::{error, fmt, io};

/// An error that occurred while reading the config.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigReadError {
    /// An error occurred while reading the file.
    Read { file: Utf8PathBuf, err: io::Error },

    /// An error occurred while deserializing the file into TOML.
    Toml {
        file: Utf8PathBuf,
        err: toml::de::Error,
    },

    /// The default profile wasn't found.
    DefaultProfileNotFound {
        default_profile: String,
        all_profiles: Vec<String>,
    },

    /// The metadata key wasn't found for a profile.
    MetadataKeyNotFound {
        profile: String,
        metadata_key: String,
        all_keys: Vec<String>,
    },

    /// The repository config defines a profile or metadata key that's already part of the default
    /// config.
    DuplicateKey {
        /// The file which contains the key that's duplicated.
        file: Utf8PathBuf,

        /// The key that's duplicated.
        key: String,

        /// The kind of key that's duplicate: either "profile" or "metadata" currently.
        kind: &'static str,
    },
}

impl ConfigReadError {
    pub(crate) fn read(file: impl Into<Utf8PathBuf>, err: io::Error) -> Self {
        ConfigReadError::Read {
            file: file.into(),
            err,
        }
    }

    pub(crate) fn toml(file: impl Into<Utf8PathBuf>, err: toml::de::Error) -> Self {
        ConfigReadError::Toml {
            file: file.into(),
            err,
        }
    }

    pub(crate) fn default_profile_not_found(
        default_profile: impl Into<String>,
        all_profiles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut all_profiles: Vec<_> = all_profiles.into_iter().map(|s| s.into()).collect();
        all_profiles.sort_unstable();
        ConfigReadError::DefaultProfileNotFound {
            default_profile: default_profile.into(),
            all_profiles,
        }
    }

    pub(crate) fn metadata_key_not_found(
        profile: impl Into<String>,
        metadata_key: impl Into<String>,
        all_keys: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut all_keys: Vec<_> = all_keys.into_iter().map(|s| s.into()).collect();
        all_keys.sort_unstable();
        ConfigReadError::MetadataKeyNotFound {
            profile: profile.into(),
            metadata_key: metadata_key.into(),
            all_keys,
        }
    }

    pub(crate) fn duplicate_key(
        file: impl Into<Utf8PathBuf>,
        key: impl Into<String>,
        kind: &'static str,
    ) -> Self {
        ConfigReadError::DuplicateKey {
            file: file.into(),
            key: key.into(),
            kind,
        }
    }
}

impl fmt::Display for ConfigReadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigReadError::Read { file, .. } => write!(f, "failed to read file {}", file),
            ConfigReadError::Toml { file, .. } => {
                write!(f, "failed to deserialize TOML from file {}", file)
            }
            ConfigReadError::DefaultProfileNotFound {
                default_profile,
                all_profiles,
            } => {
                write!(
                    f,
                    "default profile '{}' not found (known profiles: {})",
                    default_profile,
                    all_profiles.join(", ")
                )
            }
            ConfigReadError::MetadataKeyNotFound {
                profile,
                metadata_key,
                all_keys,
            } => write!(
                f,
                "for profile '{}', metadata key '{}' not found (known keys: {})",
                profile,
                metadata_key,
                all_keys.join(", ")
            ),
            ConfigReadError::DuplicateKey { file, key, kind } => write!(
                f,
                "in {}, {} key '{}' duplicated from default config",
                file, kind, key
            ),
        }
    }
}

impl error::Error for ConfigReadError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ConfigReadError::Read { err, .. } => Some(err),
            ConfigReadError::Toml { err, .. } => Some(err),
            ConfigReadError::DefaultProfileNotFound { .. }
            | ConfigReadError::MetadataKeyNotFound { .. }
            | ConfigReadError::DuplicateKey { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProfileNotFound {
    profile: String,
    all_profiles: Vec<String>,
}

impl ProfileNotFound {
    pub(crate) fn new(
        profile: impl Into<String>,
        all_profiles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut all_profiles: Vec<_> = all_profiles.into_iter().map(|s| s.into()).collect();
        all_profiles.sort_unstable();
        Self {
            profile: profile.into(),
            all_profiles,
        }
    }
}

impl fmt::Display for ProfileNotFound {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "profile '{}' not found (known profiles: {})",
            self.profile,
            self.all_profiles.join(", ")
        )
    }
}

impl error::Error for ProfileNotFound {}
