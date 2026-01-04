// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User config implementation.

use super::{
    discovery::user_config_paths,
    elements::{DefaultUiConfig, UiConfig},
};
use crate::errors::UserConfigError;
use camino::Utf8Path;
use serde::Deserialize;
use std::collections::BTreeSet;
use tracing::{debug, warn};

/// Trait for handling user configuration warnings.
///
/// This trait allows for different warning handling strategies, such as logging
/// warnings (the default behavior) or collecting them for testing purposes.
trait UserConfigWarnings {
    /// Handle unknown configuration keys found in a user config file.
    fn unknown_config_keys(&mut self, config_file: &Utf8Path, unknown: &BTreeSet<String>);
}

/// Default implementation of UserConfigWarnings that logs warnings using the
/// tracing crate.
struct DefaultUserConfigWarnings;

impl UserConfigWarnings for DefaultUserConfigWarnings {
    fn unknown_config_keys(&mut self, config_file: &Utf8Path, unknown: &BTreeSet<String>) {
        let mut unknown_str = String::new();
        if unknown.len() == 1 {
            // Print this on the same line.
            unknown_str.push_str("key: ");
            unknown_str.push_str(unknown.iter().next().unwrap());
        } else {
            unknown_str.push_str("keys:\n");
            for ignored_key in unknown {
                unknown_str.push('\n');
                unknown_str.push_str("  - ");
                unknown_str.push_str(ignored_key);
            }
        }

        warn!(
            "in user config file {}, ignoring unknown configuration {unknown_str}",
            config_file,
        );
    }
}

/// User-specific configuration.
///
/// This configuration is loaded from the user's config directory and contains
/// personal preferences that shouldn't be version-controlled.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UserConfig {
    /// UI configuration.
    #[serde(default)]
    pub ui: UiConfig,
}

impl UserConfig {
    /// Loads user config from the default location.
    ///
    /// Tries candidate paths in order and returns the first config file found:
    /// - Unix/macOS: `~/.config/nextest/config.toml`
    /// - Windows: `%APPDATA%\nextest\config.toml`, then `~/.config/nextest/config.toml`
    ///
    /// Returns `Ok(None)` if no config file exists at any candidate path.
    /// Returns `Err` if a config file exists but is invalid.
    pub fn from_default_location() -> Result<Option<Self>, UserConfigError> {
        Self::from_default_location_with_warnings(&mut DefaultUserConfigWarnings)
    }

    /// Loads user config from the default location, with custom warning
    /// handling.
    fn from_default_location_with_warnings(
        warnings: &mut impl UserConfigWarnings,
    ) -> Result<Option<Self>, UserConfigError> {
        let paths = user_config_paths()?;
        if paths.is_empty() {
            debug!("user config: could not determine config directory");
            return Ok(None);
        }

        for path in &paths {
            match Self::from_path_with_warnings(path, warnings)? {
                Some(config) => return Ok(Some(config)),
                None => continue,
            }
        }

        debug!(
            "user config: no config file found at any candidate path: {:?}",
            paths
        );
        Ok(None)
    }

    /// Loads user config from a specific path.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be read or parsed.
    pub fn from_path(path: &Utf8Path) -> Result<Option<Self>, UserConfigError> {
        Self::from_path_with_warnings(path, &mut DefaultUserConfigWarnings)
    }

    /// Loads user config from a specific path with custom warning handling.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be read or parsed.
    fn from_path_with_warnings(
        path: &Utf8Path,
        warnings: &mut impl UserConfigWarnings,
    ) -> Result<Option<Self>, UserConfigError> {
        debug!("user config: attempting to load from {path}");
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("user config: file does not exist at {path}");
                return Ok(None);
            }
            Err(error) => {
                return Err(UserConfigError::Read {
                    path: path.to_owned(),
                    error,
                });
            }
        };

        let (config, unknown) =
            Self::deserialize_toml(&contents).map_err(|error| UserConfigError::Parse {
                path: path.to_owned(),
                error,
            })?;

        if !unknown.is_empty() {
            warnings.unknown_config_keys(path, &unknown);
        }

        debug!("user config: loaded successfully from {path}");
        Ok(Some(config))
    }

    /// Deserializes TOML content and returns the config along with any unknown keys.
    fn deserialize_toml(contents: &str) -> Result<(Self, BTreeSet<String>), toml::de::Error> {
        let deserializer = toml::Deserializer::parse(contents)?;
        let mut unknown = BTreeSet::new();
        let config: UserConfig = serde_ignored::deserialize(deserializer, |path| {
            unknown.insert(path.to_string());
        })?;
        Ok((config, unknown))
    }
}

/// Default user configuration parsed from the embedded TOML.
///
/// All fields are required - this ensures the default config is complete.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DefaultUserConfig {
    /// UI configuration.
    pub ui: DefaultUiConfig,
}

impl DefaultUserConfig {
    /// The embedded default user config TOML.
    pub const DEFAULT_CONFIG: &'static str = include_str!("../../default-user-config.toml");

    /// Parses the default config.
    ///
    /// Panics if the embedded TOML is invalid or contains unknown keys.
    pub fn from_embedded() -> Self {
        let deserializer = toml::Deserializer::parse(Self::DEFAULT_CONFIG)
            .expect("embedded default user config should parse");
        let mut unknown = BTreeSet::new();
        let config: DefaultUserConfig =
            serde_ignored::deserialize(deserializer, |path: serde_ignored::Path| {
                unknown.insert(path.to_string());
            })
            .expect("embedded default user config should be valid");

        // Make sure there aren't any unknown keys in the default config, since it is
        // embedded/shipped with this binary.
        if !unknown.is_empty() {
            panic!(
                "found unknown keys in default user config: {}",
                unknown.into_iter().collect::<Vec<_>>().join(", ")
            );
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use camino_tempfile::tempdir;

    /// Test implementation of UserConfigWarnings that collects warnings for testing.
    #[derive(Default)]
    struct TestUserConfigWarnings {
        unknown_keys: Option<(Utf8PathBuf, BTreeSet<String>)>,
    }

    impl UserConfigWarnings for TestUserConfigWarnings {
        fn unknown_config_keys(&mut self, config_file: &Utf8Path, unknown: &BTreeSet<String>) {
            self.unknown_keys = Some((config_file.to_owned(), unknown.clone()));
        }
    }

    #[test]
    fn default_user_config_is_valid() {
        // This will panic if the TOML is missing any required fields, or has
        // unknown keys.
        let _ = DefaultUserConfig::from_embedded();
    }

    #[test]
    fn ignored_keys() {
        let config_contents = r#"
        ignored1 = "test"

        [ui]
        show-progress = "bar"
        ignored2 = "hi"
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let config =
            UserConfig::from_path_with_warnings(&config_path, &mut warnings).expect("config valid");

        assert!(config.is_some(), "config should be loaded");
        let config = config.unwrap();
        assert!(
            matches!(
                config.ui.show_progress,
                Some(crate::user_config::elements::UiShowProgress::Bar)
            ),
            "show-progress should be parsed correctly"
        );

        let (path, unknown) = warnings.unknown_keys.expect("should have unknown keys");
        assert_eq!(path, config_path, "path should match");
        assert_eq!(
            unknown,
            maplit::btreeset! {
                "ignored1".to_owned(),
                "ui.ignored2".to_owned(),
            },
            "unknown keys should be detected"
        );
    }

    #[test]
    fn no_ignored_keys() {
        let config_contents = r#"
        [ui]
        show-progress = "counter"
        max-progress-running = 10
        input-handler = false
        output-indent = true
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let config =
            UserConfig::from_path_with_warnings(&config_path, &mut warnings).expect("config valid");

        assert!(config.is_some(), "config should be loaded");
        assert!(
            warnings.unknown_keys.is_none(),
            "no unknown keys should be detected"
        );
    }
}
