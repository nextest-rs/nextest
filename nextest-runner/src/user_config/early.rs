// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Early user configuration loading for pager settings.
//!
//! This module provides minimal configuration loading for use before full CLI
//! parsing is complete.
//!
//! Following the pattern of [`crate::config::core::VersionOnlyConfig`], this
//! loads only the fields needed for early decisions, with graceful fallback
//! to defaults on any errors.

use super::{
    discovery::user_config_paths,
    elements::{
        CompiledUiOverride, DeserializedUiOverrideData, PagerSetting, PaginateSetting,
        StreampagerConfig, StreampagerInterface, StreampagerWrapping,
    },
    helpers::resolve_ui_setting,
    imp::{DefaultUserConfig, UserConfigLocation},
};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use std::{fmt, io};
use target_spec::{Platform, TargetSpec};
use tracing::{debug, warn};

/// Early user configuration for pager settings.
///
/// This is a minimal subset of user configuration loaded before full CLI
/// parsing completes. It contains only the settings needed to decide whether
/// and how to page help output.
///
/// Use [`Self::for_platform`] to load from the default location. If an error
/// occurs, defaults are used and a warning is logged.
#[derive(Clone, Debug)]
pub struct EarlyUserConfig {
    /// Which pager to use.
    pub pager: PagerSetting,
    /// When to paginate.
    pub paginate: PaginateSetting,
    /// Streampager configuration (for builtin pager).
    pub streampager: StreampagerConfig,
}

impl EarlyUserConfig {
    /// Loads early user configuration for the given host platform.
    ///
    /// This attempts to load user config from the specified location and resolve
    /// pager settings. On any error, returns defaults and logs a warning.
    ///
    /// This is intentionally fault-tolerant: help paging is a nice-to-have
    /// feature, so we prefer degraded behavior over failing to show help.
    pub fn for_platform(host_platform: &Platform, location: UserConfigLocation<'_>) -> Self {
        match Self::try_load(host_platform, location) {
            Ok(config) => config,
            Err(error) => {
                warn!(
                    "failed to load user config for pager settings, using defaults: {}",
                    error
                );
                Self::defaults(host_platform)
            }
        }
    }

    /// Returns the default pager configuration for the host platform.
    fn defaults(host_platform: &Platform) -> Self {
        let default_config = DefaultUserConfig::from_embedded();
        Self::resolve_from_defaults(&default_config, host_platform)
    }

    /// Attempts to load early user configuration from the specified location.
    fn try_load(
        host_platform: &Platform,
        location: UserConfigLocation<'_>,
    ) -> Result<Self, EarlyConfigError> {
        let default_config = DefaultUserConfig::from_embedded();

        match location {
            UserConfigLocation::Isolated => {
                debug!("early user config: skipping (isolated)");
                Ok(Self::resolve_from_defaults(&default_config, host_platform))
            }
            UserConfigLocation::Explicit(path) => {
                debug!("early user config: loading from explicit path {path}");
                match EarlyDeserializedConfig::from_path(path) {
                    Ok(Some(user_config)) => {
                        debug!("early user config: loaded from {path}");
                        Ok(Self::resolve(
                            &default_config,
                            Some(&user_config),
                            host_platform,
                        ))
                    }
                    Ok(None) => Err(EarlyConfigError::FileNotFound(path.to_owned())),
                    Err(error) => Err(error),
                }
            }
            UserConfigLocation::Default => {
                Self::try_load_from_default_locations(&default_config, host_platform)
            }
        }
    }

    /// Attempts to load early user configuration from default locations.
    fn try_load_from_default_locations(
        default_config: &DefaultUserConfig,
        host_platform: &Platform,
    ) -> Result<Self, EarlyConfigError> {
        let paths = user_config_paths().map_err(EarlyConfigError::Discovery)?;

        if paths.is_empty() {
            debug!("early user config: no config directory found, using defaults");
            return Ok(Self::resolve_from_defaults(default_config, host_platform));
        }

        // Try each candidate path.
        for path in &paths {
            match EarlyDeserializedConfig::from_path(path) {
                Ok(Some(user_config)) => {
                    debug!("early user config: loaded from {path}");
                    return Ok(Self::resolve(
                        default_config,
                        Some(&user_config),
                        host_platform,
                    ));
                }
                Ok(None) => {
                    debug!("early user config: file not found at {path}");
                    continue;
                }
                Err(error) => {
                    // Log a warning, but continue to try other paths or use defaults.
                    warn!("early user config: error loading {path}: {error}");
                    continue;
                }
            }
        }

        debug!("early user config: no config file found, using defaults");
        Ok(Self::resolve_from_defaults(default_config, host_platform))
    }

    /// Resolves configuration from defaults.
    fn resolve_from_defaults(default_config: &DefaultUserConfig, host_platform: &Platform) -> Self {
        Self::resolve(default_config, None, host_platform)
    }

    /// Resolves configuration from defaults and optional user config.
    fn resolve(
        default_config: &DefaultUserConfig,
        user_config: Option<&EarlyDeserializedConfig>,
        host_platform: &Platform,
    ) -> Self {
        // Compile user overrides.
        let user_overrides: Vec<CompiledUiOverride> = user_config
            .map(|c| {
                c.overrides
                    .iter()
                    .filter_map(|o| {
                        match TargetSpec::new(o.platform.clone()) {
                            Ok(spec) => Some(CompiledUiOverride::new(spec, o.ui.clone())),
                            Err(error) => {
                                // Log a warning, but otherwise skip invalid overrides.
                                warn!(
                                    "user config: invalid platform spec '{}': {error}",
                                    o.platform
                                );
                                None
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Resolve each setting using standard priority order.
        let pager = resolve_ui_setting(
            &default_config.ui.pager,
            &default_config.ui_overrides,
            user_config.and_then(|c| c.ui.pager.as_ref()),
            &user_overrides,
            host_platform,
            |data| data.pager(),
        );

        let paginate = resolve_ui_setting(
            &default_config.ui.paginate,
            &default_config.ui_overrides,
            user_config.and_then(|c| c.ui.paginate.as_ref()),
            &user_overrides,
            host_platform,
            |data| data.paginate(),
        );

        let streampager = StreampagerConfig {
            interface: resolve_ui_setting(
                &default_config.ui.streampager.interface,
                &default_config.ui_overrides,
                user_config.and_then(|c| c.ui.streampager_interface()),
                &user_overrides,
                host_platform,
                |data| data.streampager_interface(),
            ),
            wrapping: resolve_ui_setting(
                &default_config.ui.streampager.wrapping,
                &default_config.ui_overrides,
                user_config.and_then(|c| c.ui.streampager_wrapping()),
                &user_overrides,
                host_platform,
                |data| data.streampager_wrapping(),
            ),
            show_ruler: resolve_ui_setting(
                &default_config.ui.streampager.show_ruler,
                &default_config.ui_overrides,
                user_config.and_then(|c| c.ui.streampager_show_ruler()),
                &user_overrides,
                host_platform,
                |data| data.streampager_show_ruler(),
            ),
        };

        Self {
            pager,
            paginate,
            streampager,
        }
    }
}

/// Error type for early config loading.
///
/// This is internal and not exposed; errors are logged and defaults used.
#[derive(Debug)]
enum EarlyConfigError {
    Discovery(crate::errors::UserConfigError),
    /// The file specified via `NEXTEST_USER_CONFIG_FILE` does not exist.
    FileNotFound(Utf8PathBuf),
    Read(std::io::Error),
    Parse(toml::de::Error),
}

impl fmt::Display for EarlyConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery(e) => write!(f, "config discovery: {e}"),
            Self::FileNotFound(path) => write!(f, "config file not found at {path}"),
            Self::Read(e) => write!(f, "read: {e}"),
            Self::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

/// Deserialized early config - only pager-related fields.
///
/// Uses `#[serde(default)]` on all fields to ignore unknown keys and accept
/// partial configs.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct EarlyDeserializedConfig {
    #[serde(default)]
    ui: EarlyDeserializedUiConfig,
    #[serde(default)]
    overrides: Vec<EarlyDeserializedOverride>,
}

impl EarlyDeserializedConfig {
    /// Loads early config from a path.
    ///
    /// Returns `Ok(None)` if file doesn't exist, `Err` on read/parse errors.
    fn from_path(path: &Utf8Path) -> Result<Option<Self>, EarlyConfigError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(EarlyConfigError::Read(e)),
        };

        let config: Self = toml::from_str(&contents).map_err(EarlyConfigError::Parse)?;
        Ok(Some(config))
    }
}

/// Deserialized UI config - only pager-related fields.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct EarlyDeserializedUiConfig {
    #[serde(default)]
    pager: Option<PagerSetting>,
    #[serde(default)]
    paginate: Option<PaginateSetting>,
    // Streampager fields flattened for simpler access.
    #[serde(default, rename = "streampager")]
    streampager_section: EarlyDeserializedStreampagerConfig,
}

impl EarlyDeserializedUiConfig {
    fn streampager_interface(&self) -> Option<&StreampagerInterface> {
        self.streampager_section.interface.as_ref()
    }

    fn streampager_wrapping(&self) -> Option<&StreampagerWrapping> {
        self.streampager_section.wrapping.as_ref()
    }

    fn streampager_show_ruler(&self) -> Option<&bool> {
        self.streampager_section.show_ruler.as_ref()
    }
}

/// Deserialized streampager config.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct EarlyDeserializedStreampagerConfig {
    #[serde(default)]
    interface: Option<StreampagerInterface>,
    #[serde(default)]
    wrapping: Option<StreampagerWrapping>,
    #[serde(default)]
    show_ruler: Option<bool>,
}

/// Deserialized override entry.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct EarlyDeserializedOverride {
    platform: String,
    #[serde(default)]
    ui: DeserializedUiOverrideData,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::detect_host_platform_for_tests;

    #[test]
    fn test_early_user_config_defaults() {
        let host = detect_host_platform_for_tests();
        let config = EarlyUserConfig::defaults(&host);

        // This should have a configured pager.
        match &config.pager {
            PagerSetting::Builtin => {}
            PagerSetting::External(cmd) => {
                assert!(!cmd.command_name().is_empty());
            }
        }

        // Paginate should default to auto.
        assert_eq!(config.paginate, PaginateSetting::Auto);
    }

    #[test]
    fn test_early_user_config_from_host_platform() {
        let host = detect_host_platform_for_tests();

        // This should not panic, even if no config file exists.
        let config = EarlyUserConfig::for_platform(&host, UserConfigLocation::Default);

        // Should return a valid config.
        match &config.pager {
            PagerSetting::Builtin => {}
            PagerSetting::External(cmd) => {
                assert!(!cmd.command_name().is_empty());
            }
        }
    }
}
