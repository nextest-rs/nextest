// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User config implementation.

use super::{
    discovery::user_config_paths,
    elements::{
        CompiledUiOverride, DefaultUiConfig, DeserializedUiConfig, DeserializedUiOverrideData,
        UiConfig,
    },
};
use crate::errors::UserConfigError;
use camino::Utf8Path;
use serde::Deserialize;
use std::{collections::BTreeSet, io};
use target_spec::{Platform, TargetSpec};
use tracing::{debug, warn};

/// User configuration after custom settings and overrides have been applied.
#[derive(Clone, Debug)]
pub struct UserConfig {
    /// Resolved UI configuration.
    pub ui: UiConfig,
}

impl UserConfig {
    /// Loads and resolves user configuration for the given host platform.
    pub fn for_host_platform(host_platform: &Platform) -> Result<Self, UserConfigError> {
        let user_config = CompiledUserConfig::from_default_location()?;
        let default_user_config = DefaultUserConfig::from_embedded();

        let resolved_ui = UiConfig::resolve(
            &default_user_config.ui,
            &default_user_config.ui_overrides,
            user_config.as_ref().map(|c| &c.ui),
            user_config
                .as_ref()
                .map(|c| &c.ui_overrides[..])
                .unwrap_or(&[]),
            host_platform,
        );

        Ok(Self { ui: resolved_ui })
    }
}

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

/// User-specific configuration (deserialized form).
///
/// This configuration is loaded from the user's config directory and contains
/// personal preferences that shouldn't be version-controlled.
///
/// Use [`DeserializedUserConfig::compile`] to compile platform specs and get a
/// [`CompiledUserConfig`].
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DeserializedUserConfig {
    /// UI configuration.
    #[serde(default)]
    ui: DeserializedUiConfig,

    /// Configuration overrides.
    #[serde(default)]
    overrides: Vec<DeserializedOverride>,
}

/// Deserialized form of a single override entry.
///
/// Each override has a platform filter and optional settings for different
/// configuration sections.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DeserializedOverride {
    /// Platform to match (required).
    ///
    /// This is a target-spec expression like `cfg(windows)` or
    /// `x86_64-unknown-linux-gnu`.
    platform: String,

    /// UI settings to override.
    #[serde(default)]
    ui: DeserializedUiOverrideData,
}

impl DeserializedUserConfig {
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
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
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
        let config: DeserializedUserConfig = serde_ignored::deserialize(deserializer, |path| {
            unknown.insert(path.to_string());
        })?;
        Ok((config, unknown))
    }

    /// Compiles the user config by parsing platform specs in overrides.
    ///
    /// The `path` is used for error reporting.
    fn compile(self, path: &Utf8Path) -> Result<CompiledUserConfig, UserConfigError> {
        let mut ui_overrides = Vec::with_capacity(self.overrides.len());
        for (index, override_) in self.overrides.into_iter().enumerate() {
            let platform_spec = TargetSpec::new(override_.platform).map_err(|error| {
                UserConfigError::OverridePlatformSpec {
                    path: path.to_owned(),
                    index,
                    error,
                }
            })?;
            ui_overrides.push(CompiledUiOverride::new(platform_spec, override_.ui));
        }

        Ok(CompiledUserConfig {
            ui: self.ui,
            ui_overrides,
        })
    }
}

/// Compiled user configuration with parsed platform specs.
///
/// This is created from [`DeserializedUserConfig`] after compiling platform
/// expressions in overrides.
#[derive(Clone, Debug)]
pub(super) struct CompiledUserConfig {
    /// UI configuration.
    pub(super) ui: DeserializedUiConfig,
    /// Compiled UI overrides with parsed platform specs.
    pub(super) ui_overrides: Vec<CompiledUiOverride>,
}

impl CompiledUserConfig {
    /// Loads and compiles user config from the default location.
    ///
    /// This is a convenience method that combines loading and compilation.
    /// Platform specs in overrides are compiled and validated.
    ///
    /// Returns `Ok(None)` if no config file exists at any candidate path.
    /// Returns `Err` if:
    /// - A config file exists but cannot be read or parsed.
    /// - A platform spec in an override is invalid.
    pub(super) fn from_default_location() -> Result<Option<Self>, UserConfigError> {
        Self::from_default_location_with_warnings(&mut DefaultUserConfigWarnings)
    }

    /// Loads and compiles user config from the default location, with custom
    /// warning handling.
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

    /// Loads and compiles user config from a specific path with custom warning
    /// handling.
    fn from_path_with_warnings(
        path: &Utf8Path,
        warnings: &mut impl UserConfigWarnings,
    ) -> Result<Option<Self>, UserConfigError> {
        match DeserializedUserConfig::from_path_with_warnings(path, warnings)? {
            Some(config) => Ok(Some(config.compile(path)?)),
            None => Ok(None),
        }
    }
}

/// Deserialized form of the default user config before compilation.
///
/// This includes both base settings (all required) and platform-specific
/// overrides.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DeserializedDefaultUserConfig {
    /// UI configuration (base settings, all required).
    ui: DefaultUiConfig,

    /// Configuration overrides.
    #[serde(default)]
    overrides: Vec<DeserializedOverride>,
}

/// Default user configuration parsed from the embedded TOML.
///
/// This contains both the base settings (all required) and compiled
/// platform-specific overrides.
#[derive(Clone, Debug)]
pub(super) struct DefaultUserConfig {
    /// Base UI configuration.
    pub(super) ui: DefaultUiConfig,

    /// Compiled UI overrides with parsed platform specs.
    pub(super) ui_overrides: Vec<CompiledUiOverride>,
}

impl DefaultUserConfig {
    /// The embedded default user config TOML.
    const DEFAULT_CONFIG: &'static str = include_str!("../../default-user-config.toml");

    /// Parses and compiles the default config.
    ///
    /// Panics if the embedded TOML is invalid, contains unknown keys, or has
    /// invalid platform specs in overrides.
    pub(crate) fn from_embedded() -> Self {
        let deserializer = toml::Deserializer::parse(Self::DEFAULT_CONFIG)
            .expect("embedded default user config should parse");
        let mut unknown = BTreeSet::new();
        let config: DeserializedDefaultUserConfig =
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

        // Compile platform specs in overrides.
        let ui_overrides: Vec<CompiledUiOverride> = config
            .overrides
            .into_iter()
            .enumerate()
            .map(|(index, override_)| {
                let platform_spec = TargetSpec::new(override_.platform).unwrap_or_else(|error| {
                    panic!(
                        "embedded default user config has invalid platform spec \
                         in [[overrides]] at index {index}: {error}"
                    )
                });
                CompiledUiOverride::new(platform_spec, override_.ui)
            })
            .collect();

        Self {
            ui: config.ui,
            ui_overrides,
        }
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
        let config = DeserializedUserConfig::from_path_with_warnings(&config_path, &mut warnings)
            .expect("config valid");

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
        let config = DeserializedUserConfig::from_path_with_warnings(&config_path, &mut warnings)
            .expect("config valid");

        assert!(config.is_some(), "config should be loaded");
        assert!(
            warnings.unknown_keys.is_none(),
            "no unknown keys should be detected"
        );
    }

    #[test]
    fn overrides_parsing() {
        let config_contents = r#"
        [ui]
        show-progress = "bar"

        [[overrides]]
        platform = "cfg(windows)"
        ui.show-progress = "counter"
        ui.max-progress-running = 4

        [[overrides]]
        platform = "cfg(unix)"
        ui.input-handler = false
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let config = CompiledUserConfig::from_path_with_warnings(&config_path, &mut warnings)
            .expect("config valid")
            .expect("config should exist");

        assert!(
            warnings.unknown_keys.is_none(),
            "no unknown keys should be detected"
        );
        assert_eq!(config.ui_overrides.len(), 2, "should have 2 overrides");
    }

    #[test]
    fn overrides_invalid_platform() {
        let config_contents = r#"
        [ui]
        show-progress = "bar"

        [[overrides]]
        platform = "invalid platform spec!!!"
        ui.show-progress = "counter"
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let result = CompiledUserConfig::from_path_with_warnings(&config_path, &mut warnings);

        assert!(
            matches!(
                result,
                Err(UserConfigError::OverridePlatformSpec { index: 0, .. })
            ),
            "should fail with platform spec error at index 0"
        );
    }

    #[test]
    fn overrides_missing_platform() {
        let config_contents = r#"
        [ui]
        show-progress = "bar"

        [[overrides]]
        # platform field is missing - should fail to parse
        ui.show-progress = "counter"
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let result = DeserializedUserConfig::from_path_with_warnings(&config_path, &mut warnings);

        assert!(
            matches!(result, Err(UserConfigError::Parse { .. })),
            "should fail with parse error due to missing required platform field: {result:?}"
        );
    }
}
