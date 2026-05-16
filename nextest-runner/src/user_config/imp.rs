// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! User config implementation.

use super::{
    discovery::user_config_paths,
    elements::{
        CompiledRecordOverride, CompiledUiOverride, DefaultRecordConfig, DefaultUiConfig,
        DeserializedRecordConfig, DeserializedRecordOverrideData, DeserializedUiConfig,
        DeserializedUiOverrideData, RecordConfig, UiConfig,
    },
    experimental::{ExperimentalConfig, UserConfigExperimental},
};
use crate::errors::UserConfigError;
use camino::Utf8Path;
use serde::Deserialize;
use std::{collections::BTreeSet, io};
use target_spec::{Platform, TargetSpec};
use tracing::{debug, warn};

/// Special value for `--user-config-file` and `NEXTEST_USER_CONFIG_FILE` that
/// skips user config loading entirely.
pub const USER_CONFIG_NONE: &str = "none";

/// Specifies where to load user configuration from.
#[derive(Clone, Copy, Debug)]
pub enum UserConfigLocation<'a> {
    /// Discover user config from default locations (e.g.,
    /// `~/.config/nextest/config.toml`).
    Default,

    /// Skip user config loading entirely, using only built-in defaults.
    ///
    /// This is useful for test isolation.
    Isolated,

    /// Load user config from an explicit path.
    ///
    /// Returns an error if the file does not exist.
    Explicit(&'a Utf8Path),
}

impl<'a> UserConfigLocation<'a> {
    /// Creates a user config location from a CLI or environment variable value.
    ///
    /// Returns `Default` if `None`, `Isolated` if `"none"`, otherwise
    /// `Explicit` with the path.
    pub fn from_cli_or_env(s: Option<&'a str>) -> Self {
        match s {
            None => Self::Default,
            Some(s) if s == USER_CONFIG_NONE => Self::Isolated,
            Some(s) => Self::Explicit(Utf8Path::new(s)),
        }
    }
}

/// User configuration after custom settings and overrides have been applied.
#[derive(Clone, Debug)]
pub struct UserConfig {
    /// Experimental features enabled (from config and environment variables).
    pub experimental: BTreeSet<UserConfigExperimental>,
    /// Resolved UI configuration.
    pub ui: UiConfig,
    /// Resolved record configuration.
    pub record: RecordConfig,
}

impl UserConfig {
    /// The pregenerated JSON Schema for `config.toml` in the user config
    /// directory.
    ///
    /// The schema is checked into the repository at
    /// `nextest-runner/jsonschemas/user-config.json`. (If you're working
    /// within the nextest repository, regenerate the schema with `just
    /// generate-schemas`.)
    pub const SCHEMA: &'static str = include_str!("../../jsonschemas/user-config.json");

    /// The embedded default user config TOML.
    ///
    /// User-specific configuration is layered on top of this default config.
    pub const DEFAULT_CONFIG: &'static str = include_str!("../../default-user-config.toml");

    /// Loads and resolves user configuration.
    ///
    /// Platform overrides in the user config are evaluated against the build
    /// target of the nextest binary (via [`Platform::build_target`]), not
    /// against the host platform reported by `rustc -vV`. User config expresses
    /// per-user preferences for the running nextest binary, so the binary's
    /// build target is the right thing to match against — and this keeps
    /// resolution consistent across normal runs, archive replay, and commands
    /// that don't otherwise need to detect a host platform.
    pub fn load(location: UserConfigLocation<'_>) -> Result<Self, UserConfigError> {
        let build_target =
            Platform::build_target().expect("nextest is built for a supported platform");

        let user_config = CompiledUserConfig::from_location(location)?;
        let default_user_config = DefaultUserConfig::from_embedded();

        // Combine experimental features from user config and environment variables.
        let mut experimental = UserConfigExperimental::from_env();
        if let Some(config) = &user_config {
            experimental.extend(config.experimental.iter().copied());
        }

        let resolved_ui = UiConfig::resolve(
            &default_user_config.ui,
            &default_user_config.ui_overrides,
            user_config.as_ref().map(|c| &c.ui),
            user_config
                .as_ref()
                .map(|c| &c.ui_overrides[..])
                .unwrap_or(&[]),
            &build_target,
        );

        let resolved_record = RecordConfig::resolve(
            &default_user_config.record,
            &default_user_config.record_overrides,
            user_config.as_ref().map(|c| &c.record),
            user_config
                .as_ref()
                .map(|c| &c.record_overrides[..])
                .unwrap_or(&[]),
            &build_target,
        );

        Ok(Self {
            experimental,
            ui: resolved_ui,
            record: resolved_record,
        })
    }

    /// Returns true if the specified experimental feature is enabled.
    pub fn is_experimental_enabled(&self, feature: UserConfigExperimental) -> bool {
        self.experimental.contains(&feature)
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

/// Per-user nextest configuration.
///
/// Stores personal preferences such as UI defaults and recording behavior.
/// This is distinct from the repository config (`.config/nextest.toml`),
/// which controls test execution.
///
/// See [_User configuration reference_](https://nexte.st/docs/user-config/reference)
/// for details on each setting.
#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "config-schema", schemars(deny_unknown_fields))]
#[serde(rename_all = "kebab-case")]
struct DeserializedUserConfig {
    /// Toggles for experimental, non-stable features.
    ///
    /// ```toml
    /// [experimental]
    /// record = true
    /// ```
    #[serde(default)]
    experimental: ExperimentalConfig,

    /// Display, progress, and pager settings.
    #[serde(default)]
    ui: DeserializedUiConfig,

    /// Retention settings for the record-replay-rerun feature.
    #[serde(default)]
    record: DeserializedRecordConfig,

    /// Platform-specific overrides applied on top of the base configuration.
    ///
    /// Each entry specifies a `platform` filter and any number of settings to
    /// substitute when that filter matches. For each setting, the first
    /// matching override wins; the base configuration is used if no override
    /// matches.
    #[serde(default)]
    overrides: Vec<DeserializedOverride>,
}

/// A single platform-specific override entry.
#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(feature = "config-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "config-schema", schemars(deny_unknown_fields))]
#[serde(rename_all = "kebab-case")]
struct DeserializedOverride {
    /// Target-spec expression selecting which platforms this override applies
    /// to.
    ///
    /// Accepts a target triple (e.g. `x86_64-unknown-linux-gnu`) or a `cfg()`
    /// expression (e.g. `cfg(windows)`, `cfg(target_os = "macos")`). Matched
    /// against the platform nextest was built for.
    platform: String,

    /// UI settings to substitute on matching platforms.
    #[serde(default)]
    ui: DeserializedUiOverrideData,

    /// Record retention settings to substitute on matching platforms.
    #[serde(default)]
    record: DeserializedRecordOverrideData,
}

/// Returns the JSON schema for `config.toml` in the user config directory.
///
/// As with [`nextest_config_schema`](crate::config::core::nextest_config_schema),
/// the schema is intentionally stricter than nextest's runtime parser: unknown
/// fields are errors so that editors flag likely typos, while at runtime they
/// are warnings so that older nextest binaries can load configs written for
/// newer versions.
#[cfg(feature = "config-schema")]
pub fn user_config_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(DeserializedUserConfig);
    // This indicates to Tombi that nextest supports TOML 1.1.0.
    schema.insert(
        "x-tombi-toml-version".to_owned(),
        serde_json::Value::String("v1.1.0".to_owned()),
    );
    schema
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
        let mut record_overrides = Vec::with_capacity(self.overrides.len());
        for (index, override_) in self.overrides.into_iter().enumerate() {
            let platform_spec = TargetSpec::new(override_.platform).map_err(|error| {
                UserConfigError::OverridePlatformSpec {
                    path: path.to_owned(),
                    index,
                    error: Box::new(error),
                }
            })?;
            // Each override entry uses the same platform spec for both UI and
            // record settings.
            ui_overrides.push(CompiledUiOverride::new(platform_spec.clone(), override_.ui));
            record_overrides.push(CompiledRecordOverride::new(platform_spec, override_.record));
        }

        // Convert the experimental config table to a set of enabled features.
        let experimental = self.experimental.to_set();

        Ok(CompiledUserConfig {
            experimental,
            ui: self.ui,
            record: self.record,
            ui_overrides,
            record_overrides,
        })
    }
}

/// Compiled user configuration with parsed platform specs.
///
/// This is created from [`DeserializedUserConfig`] after compiling platform
/// expressions in overrides.
#[derive(Clone, Debug)]
pub(super) struct CompiledUserConfig {
    /// Experimental features enabled in user config.
    pub(super) experimental: BTreeSet<UserConfigExperimental>,
    /// UI configuration.
    pub(super) ui: DeserializedUiConfig,
    /// Record configuration.
    pub(super) record: DeserializedRecordConfig,
    /// Compiled UI overrides with parsed platform specs.
    pub(super) ui_overrides: Vec<CompiledUiOverride>,
    /// Compiled record overrides with parsed platform specs.
    pub(super) record_overrides: Vec<CompiledRecordOverride>,
}

impl CompiledUserConfig {
    /// Loads and compiles user config from the specified location.
    pub(super) fn from_location(
        location: UserConfigLocation<'_>,
    ) -> Result<Option<Self>, UserConfigError> {
        Self::from_location_with_warnings(location, &mut DefaultUserConfigWarnings)
    }

    /// Loads and compiles user config from the specified location, with custom
    /// warning handling.
    fn from_location_with_warnings(
        location: UserConfigLocation<'_>,
        warnings: &mut impl UserConfigWarnings,
    ) -> Result<Option<Self>, UserConfigError> {
        match location {
            UserConfigLocation::Isolated => {
                debug!("user config: skipping (isolated)");
                Ok(None)
            }
            UserConfigLocation::Explicit(path) => {
                debug!("user config: loading from explicit path {path}");
                match Self::from_path_with_warnings(path, warnings)? {
                    Some(config) => Ok(Some(config)),
                    None => Err(UserConfigError::FileNotFound {
                        path: path.to_owned(),
                    }),
                }
            }
            UserConfigLocation::Default => Self::from_default_location_with_warnings(warnings),
        }
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

    /// Record configuration (base settings, all required).
    record: DefaultRecordConfig,

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

    /// Base record configuration.
    pub(super) record: DefaultRecordConfig,

    /// Compiled UI overrides with parsed platform specs.
    pub(super) ui_overrides: Vec<CompiledUiOverride>,

    /// Compiled record overrides with parsed platform specs.
    pub(super) record_overrides: Vec<CompiledRecordOverride>,
}

impl DefaultUserConfig {
    /// Parses and compiles the default config.
    ///
    /// Panics if the embedded TOML is invalid, contains unknown keys, or has
    /// invalid platform specs in overrides.
    pub(crate) fn from_embedded() -> Self {
        let deserializer = toml::Deserializer::parse(UserConfig::DEFAULT_CONFIG)
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
        let mut ui_overrides = Vec::with_capacity(config.overrides.len());
        let mut record_overrides = Vec::with_capacity(config.overrides.len());
        for (index, override_) in config.overrides.into_iter().enumerate() {
            let platform_spec = TargetSpec::new(override_.platform).unwrap_or_else(|error| {
                panic!(
                    "embedded default user config has invalid platform spec \
                     in [[overrides]] at index {index}: {error}"
                )
            });
            // Each override entry uses the same platform spec for both UI and
            // record settings.
            ui_overrides.push(CompiledUiOverride::new(platform_spec.clone(), override_.ui));
            record_overrides.push(CompiledRecordOverride::new(platform_spec, override_.record));
        }

        Self {
            ui: config.ui,
            record: config.record,
            ui_overrides,
            record_overrides,
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
        assert_eq!(config.ui_overrides.len(), 2, "should have 2 UI overrides");
        assert_eq!(
            config.record_overrides.len(),
            2,
            "should have 2 record overrides"
        );
    }

    #[test]
    fn overrides_record_parsing() {
        let config_contents = r#"
        [record]
        enabled = false

        [[overrides]]
        platform = "cfg(unix)"
        record.enabled = true
        record.max-output-size = "50MB"

        [[overrides]]
        platform = "cfg(windows)"
        record.enabled = true
        record.max-records = 200
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
        assert_eq!(
            config.record_overrides.len(),
            2,
            "should have 2 record overrides"
        );
    }

    #[test]
    fn overrides_record_unknown_key() {
        let config_contents = r#"
        [[overrides]]
        platform = "cfg(unix)"
        record.enabled = true
        record.unknown-key = "test"
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let _config = CompiledUserConfig::from_path_with_warnings(&config_path, &mut warnings)
            .expect("config valid")
            .expect("config should exist");

        let (path, unknown) = warnings.unknown_keys.expect("should have unknown keys");
        assert_eq!(path, config_path, "path should match");
        assert!(
            unknown.contains("overrides.0.record.unknown-key"),
            "unknown key should be detected: {unknown:?}"
        );
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

    #[test]
    fn experimental_features_parsing() {
        let config_contents = r#"
        [experimental]
        record = true

        [ui]
        show-progress = "bar"
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
        assert!(
            config
                .experimental
                .contains(&UserConfigExperimental::Record),
            "record feature should be enabled"
        );
    }

    #[test]
    fn experimental_features_disabled() {
        let config_contents = r#"
        [experimental]
        record = false

        [ui]
        show-progress = "bar"
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
        assert!(
            !config
                .experimental
                .contains(&UserConfigExperimental::Record),
            "record feature should not be enabled"
        );
    }

    #[test]
    fn experimental_features_unknown_warning() {
        let config_contents = r#"
        [experimental]
        record = true
        unknown-feature = true

        [ui]
        show-progress = "bar"
        "#;

        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, config_contents).unwrap();

        let mut warnings = TestUserConfigWarnings::default();
        let config = CompiledUserConfig::from_path_with_warnings(&config_path, &mut warnings)
            .expect("config valid")
            .expect("config should exist");

        // Unknown fields should be warnings, not errors.
        let (path, unknown) = warnings.unknown_keys.expect("should have unknown keys");
        assert_eq!(path, config_path, "path should match");
        assert!(
            unknown.contains("experimental.unknown-feature"),
            "unknown key should be detected: {unknown:?}"
        );

        // The known feature should still be enabled.
        assert!(
            config
                .experimental
                .contains(&UserConfigExperimental::Record),
            "record feature should be enabled"
        );
    }
}
