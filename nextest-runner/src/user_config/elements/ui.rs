// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! UI-related user configuration.

use crate::reporter::{MaxProgressRunning, ShowProgress};
use serde::{
    Deserialize, Deserializer,
    de::{self, Unexpected},
};
use target_spec::{Platform, TargetSpec};

/// UI-related configuration (deserialized form).
///
/// This section controls how nextest displays progress and output during test
/// runs. All fields are optional; unspecified fields will use defaults.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::user_config) struct DeserializedUiConfig {
    /// How to show progress during test runs.
    ///
    /// Accepts: `"auto"`, `"none"`, `"bar"`, `"counter"`, `"only"`.
    pub(in crate::user_config) show_progress: Option<UiShowProgress>,

    /// Maximum running tests to display in the progress bar.
    ///
    /// Accepts: an integer, or `"infinite"` for unlimited.
    #[serde(default, deserialize_with = "deserialize_max_progress_running")]
    max_progress_running: Option<MaxProgressRunning>,

    /// Whether to enable the input handler.
    input_handler: Option<bool>,

    /// Whether to indent captured test output.
    output_indent: Option<bool>,
}

/// Default UI configuration with all values required.
///
/// This is parsed from the embedded default user config TOML. All fields are
/// required - if the TOML is missing any field, parsing fails.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct DefaultUiConfig {
    /// How to show progress during test runs.
    show_progress: UiShowProgress,

    /// Maximum running tests to display in the progress bar.
    #[serde(deserialize_with = "deserialize_max_progress_running_required")]
    max_progress_running: MaxProgressRunning,

    /// Whether to enable the input handler.
    input_handler: bool,

    /// Whether to indent captured test output.
    output_indent: bool,
}

/// Deserialized form of UI override settings.
///
/// Each field is optional; only the fields that are specified will override the
/// base configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::user_config) struct DeserializedUiOverrideData {
    /// How to show progress during test runs.
    pub(in crate::user_config) show_progress: Option<UiShowProgress>,

    /// Maximum running tests to display in the progress bar.
    #[serde(default, deserialize_with = "deserialize_max_progress_running")]
    pub(in crate::user_config) max_progress_running: Option<MaxProgressRunning>,

    /// Whether to enable the input handler.
    pub(in crate::user_config) input_handler: Option<bool>,

    /// Whether to indent captured test output.
    pub(in crate::user_config) output_indent: Option<bool>,
}

/// A compiled UI override with parsed platform spec.
///
/// This is created after parsing the platform expression from a
/// `[[overrides]]` entry.
#[derive(Clone, Debug)]
pub(in crate::user_config) struct CompiledUiOverride {
    platform_spec: TargetSpec,
    data: UiOverrideData,
}

impl CompiledUiOverride {
    /// Creates a new compiled override from a platform spec and UI data.
    pub(in crate::user_config) fn new(
        platform_spec: TargetSpec,
        data: DeserializedUiOverrideData,
    ) -> Self {
        Self {
            platform_spec,
            data: UiOverrideData {
                show_progress: data.show_progress,
                max_progress_running: data.max_progress_running,
                input_handler: data.input_handler,
                output_indent: data.output_indent,
            },
        }
    }

    /// Checks if this override matches the host platform.
    ///
    /// Unknown results (e.g., unrecognized target features) are treated as
    /// non-matching to be conservative.
    fn matches(&self, host_platform: &Platform) -> bool {
        self.platform_spec
            .eval(host_platform)
            .unwrap_or(/* unknown results are mapped to false */ false)
    }
}

/// Override data for UI settings.
#[derive(Clone, Debug, Default)]
struct UiOverrideData {
    show_progress: Option<UiShowProgress>,
    max_progress_running: Option<MaxProgressRunning>,
    input_handler: Option<bool>,
    output_indent: Option<bool>,
}

/// Resolved UI configuration after applying overrides.
///
/// This represents the final resolved settings after evaluating the base
/// configuration and any matching platform-specific overrides.
#[derive(Clone, Debug)]
pub struct UiConfig {
    /// How to show progress during test runs.
    pub show_progress: UiShowProgress,
    /// Maximum running tests to display in the progress bar.
    pub max_progress_running: MaxProgressRunning,
    /// Whether to enable the input handler.
    pub input_handler: bool,
    /// Whether to indent captured test output.
    pub output_indent: bool,
}

impl UiConfig {
    /// Resolves UI configuration from user configs, defaults, and the host
    /// platform.
    ///
    /// Resolution order (highest to lowest priority):
    ///
    /// 1. User overrides (first matching override for each setting)
    /// 2. Default overrides (first matching override for each setting)
    /// 3. User base config
    /// 4. Default base config
    ///
    /// This matches the resolution order used by repo config.
    pub(in crate::user_config) fn resolve(
        default_config: &DefaultUiConfig,
        default_overrides: &[CompiledUiOverride],
        user_config: Option<&DeserializedUiConfig>,
        user_overrides: &[CompiledUiOverride],
        host_platform: &Platform,
    ) -> Self {
        Self {
            show_progress: Self::resolve_setting(
                default_config.show_progress,
                default_overrides,
                user_config.and_then(|c| c.show_progress),
                user_overrides,
                host_platform,
                |data| data.show_progress,
            ),
            max_progress_running: Self::resolve_setting(
                default_config.max_progress_running,
                default_overrides,
                user_config.and_then(|c| c.max_progress_running),
                user_overrides,
                host_platform,
                |data| data.max_progress_running,
            ),
            input_handler: Self::resolve_setting(
                default_config.input_handler,
                default_overrides,
                user_config.and_then(|c| c.input_handler),
                user_overrides,
                host_platform,
                |data| data.input_handler,
            ),
            output_indent: Self::resolve_setting(
                default_config.output_indent,
                default_overrides,
                user_config.and_then(|c| c.output_indent),
                user_overrides,
                host_platform,
                |data| data.output_indent,
            ),
        }
    }

    /// Resolves a single setting using the standard priority order.
    fn resolve_setting<T: Copy>(
        default_value: T,
        default_overrides: &[CompiledUiOverride],
        user_value: Option<T>,
        user_overrides: &[CompiledUiOverride],
        host_platform: &Platform,
        get_override: impl Fn(&UiOverrideData) -> Option<T>,
    ) -> T {
        // 1. User overrides (first match).
        for override_ in user_overrides {
            if override_.matches(host_platform)
                && let Some(v) = get_override(&override_.data)
            {
                return v;
            }
        }

        // 2. Default overrides (first match).
        for override_ in default_overrides {
            if override_.matches(host_platform)
                && let Some(v) = get_override(&override_.data)
            {
                return v;
            }
        }

        // 3. User base config.
        if let Some(v) = user_value {
            return v;
        }

        // 4. Default base config.
        default_value
    }
}

/// Show progress setting for UI configuration.
///
/// This is separate from [`ShowProgress`] because the `Only` variant has
/// special behavior: it implies `--status-level=slow` and
/// `--final-status-level=none`. This information would be lost if we converted
/// directly to `ShowProgress`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UiShowProgress {
    /// Automatically choose based on terminal capabilities.
    #[default]
    Auto,
    /// No progress display.
    None,
    /// Show a progress bar with running tests.
    Bar,
    /// Show a simple counter (e.g., "(1/10)").
    Counter,
    /// Like `Bar`, but also sets `status-level=slow` and
    /// `final-status-level=none`.
    Only,
}

impl From<UiShowProgress> for ShowProgress {
    fn from(ui: UiShowProgress) -> Self {
        match ui {
            UiShowProgress::Auto => ShowProgress::Auto,
            UiShowProgress::None => ShowProgress::None,
            UiShowProgress::Bar | UiShowProgress::Only => ShowProgress::Running,
            UiShowProgress::Counter => ShowProgress::Counter,
        }
    }
}

/// Visitor for deserializing max-progress-running (string or integer).
struct MaxProgressRunningVisitor;

impl<'de> de::Visitor<'de> for MaxProgressRunningVisitor {
    type Value = MaxProgressRunning;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a non-negative integer or \"infinite\"")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(MaxProgressRunning::Count(v as usize))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        if v < 0 {
            Err(E::invalid_value(Unexpected::Signed(v), &self))
        } else {
            Ok(MaxProgressRunning::Count(v as usize))
        }
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if v == "infinite" {
            Ok(MaxProgressRunning::Infinite)
        } else {
            // Try parsing as a number.
            v.parse::<usize>()
                .map(MaxProgressRunning::Count)
                .map_err(|_| E::invalid_value(Unexpected::Str(v), &self))
        }
    }
}

fn deserialize_max_progress_running<'de, D>(
    deserializer: D,
) -> Result<Option<MaxProgressRunning>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_option(OptionMaxProgressRunningVisitor)
}

/// Visitor for deserializing Option<MaxProgressRunning>.
struct OptionMaxProgressRunningVisitor;

impl<'de> de::Visitor<'de> for OptionMaxProgressRunningVisitor {
    type Value = Option<MaxProgressRunning>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a non-negative integer, \"infinite\", or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer
            .deserialize_any(MaxProgressRunningVisitor)
            .map(Some)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

fn deserialize_max_progress_running_required<'de, D>(
    deserializer: D,
) -> Result<MaxProgressRunning, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(MaxProgressRunningVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{platform::detect_host_platform_for_tests, user_config::DefaultUserConfig};

    /// Helper to create a CompiledUiOverride for tests.
    fn make_override(platform: &str, data: DeserializedUiOverrideData) -> CompiledUiOverride {
        let platform_spec =
            TargetSpec::new(platform.to_string()).expect("valid platform spec in test");
        CompiledUiOverride::new(platform_spec, data)
    }

    #[test]
    fn test_ui_config_show_progress() {
        // Test valid values.
        let config: DeserializedUiConfig = toml::from_str(r#"show-progress = "auto""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Auto)));

        let config: DeserializedUiConfig = toml::from_str(r#"show-progress = "none""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::None)));

        let config: DeserializedUiConfig = toml::from_str(r#"show-progress = "bar""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Bar)));

        let config: DeserializedUiConfig = toml::from_str(r#"show-progress = "counter""#).unwrap();
        assert!(matches!(
            config.show_progress,
            Some(UiShowProgress::Counter)
        ));

        let config: DeserializedUiConfig = toml::from_str(r#"show-progress = "only""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Only)));

        // Test missing value.
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.show_progress.is_none());

        // Test invalid value.
        toml::from_str::<DeserializedUiConfig>(r#"show-progress = "invalid""#).unwrap_err();
    }

    #[test]
    fn test_ui_show_progress_to_show_progress() {
        // Test conversion to ShowProgress.
        assert_eq!(ShowProgress::from(UiShowProgress::Auto), ShowProgress::Auto);
        assert_eq!(ShowProgress::from(UiShowProgress::None), ShowProgress::None);
        assert_eq!(
            ShowProgress::from(UiShowProgress::Bar),
            ShowProgress::Running
        );
        assert_eq!(
            ShowProgress::from(UiShowProgress::Counter),
            ShowProgress::Counter
        );
        // Only maps to Running (special behavior handled separately).
        assert_eq!(
            ShowProgress::from(UiShowProgress::Only),
            ShowProgress::Running
        );
    }

    #[test]
    fn test_ui_config_max_progress_running() {
        // Test integer values.
        let config: DeserializedUiConfig = toml::from_str("max-progress-running = 10").unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Count(10))
        ));

        let config: DeserializedUiConfig = toml::from_str("max-progress-running = 0").unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Count(0))
        ));

        // Test string "infinite".
        let config: DeserializedUiConfig =
            toml::from_str(r#"max-progress-running = "infinite""#).unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Infinite)
        ));

        // Test that matching is case-sensitive.
        toml::from_str::<DeserializedUiConfig>(r#"max-progress-running = "INFINITE""#).unwrap_err();

        // Test missing value.
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.max_progress_running.is_none());

        // Test invalid value.
        toml::from_str::<DeserializedUiConfig>(r#"max-progress-running = "invalid""#).unwrap_err();
    }

    #[test]
    fn test_ui_config_input_handler() {
        let config: DeserializedUiConfig = toml::from_str("input-handler = true").unwrap();
        assert_eq!(config.input_handler, Some(true));
        let config: DeserializedUiConfig = toml::from_str("input-handler = false").unwrap();
        assert_eq!(config.input_handler, Some(false));
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.input_handler.is_none());
    }

    #[test]
    fn test_ui_config_output_indent() {
        let config: DeserializedUiConfig = toml::from_str("output-indent = true").unwrap();
        assert_eq!(config.output_indent, Some(true));
        let config: DeserializedUiConfig = toml::from_str("output-indent = false").unwrap();
        assert_eq!(config.output_indent, Some(false));
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.output_indent.is_none());
    }

    #[test]
    fn test_resolved_ui_config_defaults_only() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[], &host);

        // Resolved values should match the embedded defaults.
        assert_eq!(resolved.show_progress, defaults.show_progress);
        assert_eq!(resolved.max_progress_running, defaults.max_progress_running);
        assert_eq!(resolved.input_handler, defaults.input_handler);
        assert_eq!(resolved.output_indent, defaults.output_indent);
    }

    #[test]
    fn test_resolved_ui_config_user_config_overrides_defaults() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        let user_config = DeserializedUiConfig {
            show_progress: Some(UiShowProgress::Bar),
            max_progress_running: Some(MaxProgressRunning::Count(4)),
            input_handler: None, // Use default.
            output_indent: Some(false),
        };

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], Some(&user_config), &[], &host);

        assert_eq!(resolved.show_progress, UiShowProgress::Bar);
        assert_eq!(resolved.max_progress_running, MaxProgressRunning::Count(4));
        assert_eq!(resolved.input_handler, defaults.input_handler); // From defaults.
        assert!(!resolved.output_indent);
    }

    #[test]
    fn test_resolved_ui_config_user_override_applies() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create a user override that matches any platform.
        let override_ = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter),
                max_progress_running: None,
                input_handler: Some(false),
                output_indent: None,
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override_], &host);

        assert_eq!(resolved.show_progress, UiShowProgress::Counter);
        assert_eq!(resolved.max_progress_running, defaults.max_progress_running); // From defaults.
        assert!(!resolved.input_handler);
        assert_eq!(resolved.output_indent, defaults.output_indent); // From defaults.
    }

    #[test]
    fn test_resolved_ui_config_default_override_applies() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create a default override that matches any platform.
        let override_ = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter),
                max_progress_running: None,
                input_handler: Some(false),
                output_indent: None,
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[override_], None, &[], &host);

        assert_eq!(resolved.show_progress, UiShowProgress::Counter);
        assert_eq!(resolved.max_progress_running, defaults.max_progress_running); // From defaults.
        assert!(!resolved.input_handler);
        assert_eq!(resolved.output_indent, defaults.output_indent); // From defaults.
    }

    #[test]
    fn test_resolved_ui_config_platform_override_no_match() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create an override that never matches (cfg(any()) with no arguments
        // is false).
        let override_ = make_override(
            "cfg(any())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter),
                max_progress_running: Some(MaxProgressRunning::Count(2)),
                input_handler: Some(false),
                output_indent: Some(false),
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override_], &host);

        // Nothing should be overridden - all values should match defaults.
        assert_eq!(resolved.show_progress, defaults.show_progress);
        assert_eq!(resolved.max_progress_running, defaults.max_progress_running);
        assert_eq!(resolved.input_handler, defaults.input_handler);
        assert_eq!(resolved.output_indent, defaults.output_indent);
    }

    #[test]
    fn test_resolved_ui_config_first_matching_user_override_wins() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create two user overrides that both match (cfg(all()) is always true).
        let override1 = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Bar),
                ..Default::default()
            },
        );

        let override2 = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter), // Should be ignored.
                max_progress_running: Some(MaxProgressRunning::Count(4)),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override1, override2], &host);

        // First override wins for show_progress.
        assert_eq!(resolved.show_progress, UiShowProgress::Bar);
        // Second override's max_progress_running applies (first didn't set it).
        assert_eq!(resolved.max_progress_running, MaxProgressRunning::Count(4));
    }

    #[test]
    fn test_resolved_ui_config_user_override_beats_default_override() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // User override sets show_progress.
        let user_override = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Bar),
                ..Default::default()
            },
        );

        // Default override sets show_progress and max_progress_running.
        let default_override = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter), // Should be ignored.
                max_progress_running: Some(MaxProgressRunning::Count(4)),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(
            &defaults,
            &[default_override],
            None,
            &[user_override],
            &host,
        );

        // User override wins for show_progress.
        assert_eq!(resolved.show_progress, UiShowProgress::Bar);
        // Default override applies for max_progress_running (user didn't set it).
        assert_eq!(resolved.max_progress_running, MaxProgressRunning::Count(4));
    }

    #[test]
    fn test_resolved_ui_config_override_beats_user_base() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // User base config sets show_progress.
        let user_config = DeserializedUiConfig {
            show_progress: Some(UiShowProgress::None),
            max_progress_running: Some(MaxProgressRunning::Count(2)),
            input_handler: None,
            output_indent: None,
        };

        // Default override sets show_progress (should beat user base).
        let default_override = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                show_progress: Some(UiShowProgress::Counter),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(
            &defaults,
            &[default_override],
            Some(&user_config),
            &[],
            &host,
        );

        // Default override is chosen over user base for show_progress.
        assert_eq!(resolved.show_progress, UiShowProgress::Counter);
        // User base applies for max_progress_running (override didn't set it).
        assert_eq!(resolved.max_progress_running, MaxProgressRunning::Count(2));
    }
}
