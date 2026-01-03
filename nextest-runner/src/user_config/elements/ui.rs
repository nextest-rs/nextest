// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! UI-related user configuration.

use crate::reporter::{MaxProgressRunning, ShowProgress};
use serde::{Deserialize, Deserializer, de};

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

fn deserialize_show_progress<'de, D>(deserializer: D) -> Result<Option<UiShowProgress>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s.as_deref() {
        None => Ok(None),
        Some("auto") => Ok(Some(UiShowProgress::Auto)),
        Some("none") => Ok(Some(UiShowProgress::None)),
        Some("bar") => Ok(Some(UiShowProgress::Bar)),
        Some("counter") => Ok(Some(UiShowProgress::Counter)),
        Some("only") => Ok(Some(UiShowProgress::Only)),
        Some(other) => Err(de::Error::custom(format!(
            "invalid show-progress value: {other:?}, expected one of: \
             auto, none, bar, counter, only"
        ))),
    }
}

fn deserialize_max_progress_running<'de, D>(
    deserializer: D,
) -> Result<Option<MaxProgressRunning>, D::Error>
where
    D: Deserializer<'de>,
{
    // This can be either a string ("infinite") or an integer.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrInt {
        String(String),
        Int(usize),
    }

    let value: Option<StringOrInt> = Option::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(StringOrInt::String(s)) => {
            if s.eq_ignore_ascii_case("infinite") {
                Ok(Some(MaxProgressRunning::Infinite))
            } else {
                // Try parsing as a number.
                match s.parse::<usize>() {
                    Ok(n) => Ok(Some(MaxProgressRunning::Count(n))),
                    Err(_) => Err(de::Error::custom(format!(
                        "invalid max-progress-running value: {s:?}, \
                         expected an integer or \"infinite\""
                    ))),
                }
            }
        }
        Some(StringOrInt::Int(n)) => Ok(Some(MaxProgressRunning::Count(n))),
    }
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
        let result: Result<UiConfig, _> = toml::from_str(r#"show-progress = "invalid""#);
        assert!(result.is_err());
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

        // Test case-insensitive.
        let config: UiConfig = toml::from_str(r#"max-progress-running = "INFINITE""#).unwrap();
        assert!(matches!(
            config.max_progress_running,
            Some(MaxProgressRunning::Infinite)
        ));

        // Test missing value.
        let config: UiConfig = toml::from_str("").unwrap();
        assert!(config.max_progress_running.is_none());

        // Test invalid value.
        let result: Result<UiConfig, _> = toml::from_str(r#"max-progress-running = "invalid""#);
        assert!(result.is_err());
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
