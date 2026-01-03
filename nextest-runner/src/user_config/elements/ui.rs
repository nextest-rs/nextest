// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! UI-related user configuration.

use crate::reporter::{MaxProgressRunning, ShowProgress};
use serde::{
    Deserialize, Deserializer,
    de::{self, Unexpected},
};

/// UI-related configuration.
///
/// This section controls how nextest displays progress and output during test
/// runs.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UiConfig {
    /// How to show progress during test runs.
    ///
    /// Accepts: `"auto"`, `"none"`, `"bar"`, `"counter"`, `"only"`.
    #[serde(default, deserialize_with = "deserialize_show_progress")]
    pub show_progress: Option<UiShowProgress>,

    /// Maximum running tests to display in the progress bar.
    ///
    /// Accepts: an integer, or `"infinite"` for unlimited.
    #[serde(default, deserialize_with = "deserialize_max_progress_running")]
    pub max_progress_running: Option<MaxProgressRunning>,

    /// Whether to enable the input handler.
    pub input_handler: Option<bool>,

    /// Whether to indent captured test output.
    pub output_indent: Option<bool>,
}

/// Default UI configuration with all values required.
///
/// This is parsed from the embedded default user config TOML. All fields are
/// required - if the TOML is missing any field, parsing fails.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DefaultUiConfig {
    /// How to show progress during test runs.
    #[serde(deserialize_with = "deserialize_show_progress_required")]
    pub show_progress: UiShowProgress,

    /// Maximum running tests to display in the progress bar.
    #[serde(deserialize_with = "deserialize_max_progress_running_required")]
    pub max_progress_running: MaxProgressRunning,

    /// Whether to enable the input handler.
    pub input_handler: bool,

    /// Whether to indent captured test output.
    pub output_indent: bool,
}

/// Show progress setting for UI configuration.
///
/// This is separate from [`ShowProgress`] because the `Only` variant has
/// special behavior: it implies `--status-level=slow` and
/// `--final-status-level=none`. This information would be lost if we converted
/// directly to `ShowProgress`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// Parses a string into a UiShowProgress value.
fn parse_show_progress<E: de::Error>(s: &str) -> Result<UiShowProgress, E> {
    match s {
        "auto" => Ok(UiShowProgress::Auto),
        "none" => Ok(UiShowProgress::None),
        "bar" => Ok(UiShowProgress::Bar),
        "counter" => Ok(UiShowProgress::Counter),
        "only" => Ok(UiShowProgress::Only),
        other => Err(E::custom(format!(
            "invalid show-progress value: {other:?}, expected one of: \
             auto, none, bar, counter, only"
        ))),
    }
}

fn deserialize_show_progress<'de, D>(deserializer: D) -> Result<Option<UiShowProgress>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    s.map(|s| parse_show_progress(&s)).transpose()
}

fn deserialize_show_progress_required<'de, D>(deserializer: D) -> Result<UiShowProgress, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    parse_show_progress(&s)
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

    #[test]
    fn test_ui_config_show_progress() {
        // Test valid values.
        let config: UiConfig = toml::from_str(r#"show-progress = "auto""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Auto)));

        let config: UiConfig = toml::from_str(r#"show-progress = "none""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::None)));

        let config: UiConfig = toml::from_str(r#"show-progress = "bar""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Bar)));

        let config: UiConfig = toml::from_str(r#"show-progress = "counter""#).unwrap();
        assert!(matches!(
            config.show_progress,
            Some(UiShowProgress::Counter)
        ));

        let config: UiConfig = toml::from_str(r#"show-progress = "only""#).unwrap();
        assert!(matches!(config.show_progress, Some(UiShowProgress::Only)));

        // Test missing value.
        let config: UiConfig = toml::from_str("").unwrap();
        assert!(config.show_progress.is_none());

        // Test invalid value.
        toml::from_str::<UiConfig>(r#"show-progress = "invalid""#).unwrap_err();
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
        let config: UiConfig = toml::from_str("max-progress-running = 10").unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Count(10))
        ));

        let config: UiConfig = toml::from_str("max-progress-running = 0").unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Count(0))
        ));

        // Test string "infinite".
        let config: UiConfig = toml::from_str(r#"max-progress-running = "infinite""#).unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Infinite)
        ));

        // Test that matching is case-sensitive.
        toml::from_str::<UiConfig>(r#"max-progress-running = "INFINITE""#).unwrap_err();

        // Test missing value.
        let config: UiConfig = toml::from_str("").unwrap();
        assert!(config.max_progress_running.is_none());

        // Test invalid value.
        toml::from_str::<UiConfig>(r#"max-progress-running = "invalid""#).unwrap_err();
    }

    #[test]
    fn test_ui_config_input_handler() {
        let config: UiConfig = toml::from_str("input-handler = true").unwrap();
        assert_eq!(config.input_handler, Some(true));
        let config: UiConfig = toml::from_str("input-handler = false").unwrap();
        assert_eq!(config.input_handler, Some(false));
        let config: UiConfig = toml::from_str("").unwrap();
        assert!(config.input_handler.is_none());
    }

    #[test]
    fn test_ui_config_output_indent() {
        let config: UiConfig = toml::from_str("output-indent = true").unwrap();
        assert_eq!(config.output_indent, Some(true));
        let config: UiConfig = toml::from_str("output-indent = false").unwrap();
        assert_eq!(config.output_indent, Some(false));
        let config: UiConfig = toml::from_str("").unwrap();
        assert!(config.output_indent.is_none());
    }
}
