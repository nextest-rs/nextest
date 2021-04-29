// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Manages configuration for nextest.

use crate::reporter::FailureOutput;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use std::{
    collections::{hash_map::Entry, HashMap},
    error, fmt, io,
};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
pub struct ConfigOpts {
    /// Config file [default: <workspace-root>/nextest.toml]
    #[structopt(long)]
    config_file: Option<Utf8PathBuf>,
}

/// Configuration for nextest.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NextestConfig {
    /// The default profile: should be one of the profiles listed in the profiles section.
    #[serde(default = "default_string")]
    default_profile: String,

    /// Profiles for nextest, keyed by name.
    profiles: HashMap<String, NextestProfileImpl>,

    /// Metadata configuration.
    metadata: HashMap<String, MetadataConfig>,
}

fn default_string() -> String {
    "default".to_owned()
}

/// A hack that lets the contents of default-config.toml be included.
macro_rules! doc_comment {
    ($doc:expr, $($t:tt)*) => (
        #[doc = $doc]
        $($t)*
    );
}

impl NextestConfig {
    /// The string `nextest.toml`, used to read the config from the given directory.
    pub const NEXTEST_TOML: &'static str = "nextest.toml";

    doc_comment! {
        concat!("\
Contains the default config as a TOML file.

The default rules included with this copy of nextest-runner are:

```toml
", include_str!("../default-config.toml"), "\
```

Custom, repository-specific configuration is layered on top of the default config.
"),
        pub const DEFAULT_CONFIG: &'static str = include_str!("../default-config.toml");
    }

    /// Reads the nextest config from the given file, or if not present from `nextest.toml` in the
    /// given directory.
    ///
    /// If the file isn't specified and the directory doesn't have `nextest.toml`, uses the default
    /// config options.
    pub fn from_opts(
        config_opts: &ConfigOpts,
        workspace_root: &Utf8Path,
    ) -> Result<Self, ConfigReadError> {
        let config = Self::read_from_sources(config_opts.config_file.as_deref(), workspace_root)?;
        config.validate()?;
        Ok(config)
    }

    /// Returns the profile with the given name, the default profile if not specified, or an error
    /// if a profile was specified but not found.
    pub fn profile(&self, name: Option<&str>) -> Result<NextestProfile<'_>, ProfileNotFound> {
        match name {
            Some(name) => self
                .make_profile(name)
                .ok_or_else(|| ProfileNotFound::new(name, self.profiles.keys())),
            None => Ok(self
                .make_profile(&self.default_profile)
                .expect("validate() checks for default profile")),
        }
    }

    // ---
    // Helper methods
    // ---

    fn read_from_sources(
        file: Option<&Utf8Path>,
        workspace_root: &Utf8Path,
    ) -> Result<Self, ConfigReadError> {
        // First, get the default config. Other configs are layered on top of it.
        let mut config = Self::default();

        let repo_config = {
            // Read a file that's explicitly specified.
            if let Some(file) = file {
                let config = Self::read_file(file)?;
                Some((file.to_owned(), config))
            } else {
                // Attempt to read nextest.toml from the workspace root if it exists.
                let default_file = workspace_root.join(Self::NEXTEST_TOML);
                if default_file.is_file() {
                    let config = Self::read_file(&default_file)?;
                    Some((default_file, config))
                } else {
                    None
                }
            }
        };

        if let Some((file, repo_config)) = repo_config {
            config.merge(file, repo_config)?;
        }

        Ok(config)
    }

    fn read_file(file: &Utf8Path) -> Result<Self, ConfigReadError> {
        let data = std::fs::read_to_string(file).map_err(|err| ConfigReadError::read(file, err))?;
        toml::de::from_str(&data).map_err(|err| ConfigReadError::toml(file, err))
    }

    fn merge(&mut self, file: Utf8PathBuf, other: NextestConfig) -> Result<(), ConfigReadError> {
        self.default_profile = other.default_profile;

        let file = Self::merge_entries(file, "profile", &mut self.profiles, other.profiles)?;
        Self::merge_entries(file, "metadata", &mut self.metadata, other.metadata)?;

        Ok(())
    }

    // Returning the path passed in is a somewhat ugly way to avoid clones. Might be worth cleaning
    // this up in the future.
    fn merge_entries<V>(
        file: Utf8PathBuf,
        kind: &'static str,
        self_entries: &mut HashMap<String, V>,
        other_entries: HashMap<String, V>,
    ) -> Result<Utf8PathBuf, ConfigReadError> {
        for (key, value) in other_entries {
            match self_entries.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                Entry::Occupied(entry) => {
                    return Err(ConfigReadError::duplicate_key(file, entry.key(), kind));
                }
            }
        }

        Ok(file)
    }

    fn validate(&self) -> Result<(), ConfigReadError> {
        // Check that the profile listed in default_profile is valid.
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(ConfigReadError::default_profile_not_found(
                &self.default_profile,
                self.profiles.keys(),
            ));
        }

        // Check that metadata keys listed in profiles are valid.
        for (profile_name, profile) in &self.profiles {
            if !self.metadata.contains_key(&profile.metadata_key) {
                return Err(ConfigReadError::metadata_key_not_found(
                    profile_name,
                    &profile.metadata_key,
                    self.metadata.keys(),
                ));
            }
        }

        Ok(())
    }

    fn make_profile(&self, name: &str) -> Option<NextestProfile<'_>> {
        let inner = self.profiles.get(name)?;
        let metadata = self
            .metadata
            .get(&inner.metadata_key)
            .expect("validate() checks for metadata keys");
        Some(NextestProfile { inner, metadata })
    }
}

impl Default for NextestConfig {
    fn default() -> Self {
        toml::de::from_str(Self::DEFAULT_CONFIG).expect("default config should be valid")
    }
}

/// A representation of a nextest profile.
#[derive(Copy, Clone, Debug)]
pub struct NextestProfile<'cfg> {
    inner: &'cfg NextestProfileImpl,
    metadata: &'cfg MetadataConfig,
}

impl<'cfg> NextestProfile<'cfg> {
    /// Initialize the metadata directory if specified.
    pub fn init_metadata_dir(&self, workspace_root: &Utf8Path) -> anyhow::Result<()> {
        let metadata_dir = workspace_root.join(&self.metadata.dir);
        std::fs::create_dir_all(&metadata_dir)
            .with_context(|| format!("failed to create metadata dir '{}'", metadata_dir))
    }

    /// Returns the name of this test run.
    pub(crate) fn name(&self) -> &'cfg str {
        &self.inner.name
    }

    /// Returns the number of retries.
    pub(crate) fn retries(&self) -> usize {
        self.inner.retries
    }

    /// Returns the metadata configuration.
    pub(crate) fn metadata_config(&self) -> &'cfg MetadataConfig {
        self.metadata
    }

    /// Returns the configuration for the situations in which failure is output.
    pub(crate) fn failure_output(&self) -> FailureOutput {
        self.inner.failure_output
    }
}

/// A nextest profile, containing settings for how a test should be run.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestProfileImpl {
    /// A name given to this test run.
    name: String,

    /// The number of times a test is attempted to be re-run if it fails. Defaults to 0.
    #[serde(default)]
    retries: usize,

    /// Metadata configuration: one of the keys in the metadata section.
    #[serde(default = "default_string")]
    metadata_key: String,

    /// The situations in which a failure is output.
    #[serde(default)]
    failure_output: FailureOutput,
}

/// Metadata configuration for nextest.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct MetadataConfig {
    /// The directory where metadata files are stored. This is relative to the workspace root.
    pub(crate) dir: Utf8PathBuf,

    /// Output metadata in the JUnit format in addition to the canonical format.
    #[serde(default)]
    pub(crate) junit: Option<Utf8PathBuf>,
}

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
    fn read(file: impl Into<Utf8PathBuf>, err: io::Error) -> Self {
        ConfigReadError::Read {
            file: file.into(),
            err,
        }
    }

    fn toml(file: impl Into<Utf8PathBuf>, err: toml::de::Error) -> Self {
        ConfigReadError::Toml {
            file: file.into(),
            err,
        }
    }

    fn default_profile_not_found(
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

    fn metadata_key_not_found(
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

    fn duplicate_key(
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
    fn new(
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

#[cfg(test)]
mod tests {
    use crate::config::NextestConfig;

    #[test]
    fn default_config_is_valid() {
        let default_config = NextestConfig::default();
        default_config.validate().expect("default config is valid");
    }
}
