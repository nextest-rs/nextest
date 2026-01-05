// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! UI-related user configuration.

use crate::{
    reporter::{MaxProgressRunning, ShowProgress},
    user_config::helpers::resolve_ui_setting,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, Unexpected},
};
use std::{collections::BTreeMap, fmt, process::Command};
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

    /// Pager command for output that benefits from scrolling.
    #[serde(default)]
    pager: Option<PagerSetting>,

    /// When to paginate output.
    #[serde(default)]
    paginate: Option<PaginateSetting>,

    /// Configuration for the builtin streampager.
    #[serde(default)]
    streampager: DeserializedStreampagerConfig,
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

    /// Pager command for output that benefits from scrolling.
    pub(in crate::user_config) pager: PagerSetting,

    /// When to paginate output.
    pub(in crate::user_config) paginate: PaginateSetting,

    /// Configuration for the builtin streampager.
    pub(in crate::user_config) streampager: DefaultStreampagerConfig,
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

    /// Pager command for output that benefits from scrolling.
    #[serde(default)]
    pub(in crate::user_config) pager: Option<PagerSetting>,

    /// When to paginate output.
    #[serde(default)]
    pub(in crate::user_config) paginate: Option<PaginateSetting>,

    /// Configuration for the builtin streampager.
    #[serde(default)]
    pub(in crate::user_config) streampager: DeserializedStreampagerConfig,
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
                pager: data.pager,
                paginate: data.paginate,
                streampager_interface: data.streampager.interface,
                streampager_wrapping: data.streampager.wrapping,
                streampager_show_ruler: data.streampager.show_ruler,
            },
        }
    }

    /// Checks if this override matches the host platform.
    ///
    /// Unknown results (e.g., unrecognized target features) are treated as
    /// non-matching to be conservative.
    pub(in crate::user_config) fn matches(&self, host_platform: &Platform) -> bool {
        self.platform_spec
            .eval(host_platform)
            .unwrap_or(/* unknown results are mapped to false */ false)
    }

    /// Returns a reference to the override data.
    pub(in crate::user_config) fn data(&self) -> &UiOverrideData {
        &self.data
    }
}

/// Override data for UI settings.
#[derive(Clone, Debug, Default)]
pub(in crate::user_config) struct UiOverrideData {
    show_progress: Option<UiShowProgress>,
    max_progress_running: Option<MaxProgressRunning>,
    input_handler: Option<bool>,
    output_indent: Option<bool>,
    pager: Option<PagerSetting>,
    paginate: Option<PaginateSetting>,
    streampager_interface: Option<StreampagerInterface>,
    streampager_wrapping: Option<StreampagerWrapping>,
    streampager_show_ruler: Option<bool>,
}

impl UiOverrideData {
    /// Returns the pager setting, if specified.
    pub(in crate::user_config) fn pager(&self) -> Option<&PagerSetting> {
        self.pager.as_ref()
    }

    /// Returns the paginate setting, if specified.
    pub(in crate::user_config) fn paginate(&self) -> Option<&PaginateSetting> {
        self.paginate.as_ref()
    }

    /// Returns the streampager interface, if specified.
    pub(in crate::user_config) fn streampager_interface(&self) -> Option<&StreampagerInterface> {
        self.streampager_interface.as_ref()
    }

    /// Returns the streampager wrapping, if specified.
    pub(in crate::user_config) fn streampager_wrapping(&self) -> Option<&StreampagerWrapping> {
        self.streampager_wrapping.as_ref()
    }

    /// Returns the streampager show-ruler setting, if specified.
    pub(in crate::user_config) fn streampager_show_ruler(&self) -> Option<&bool> {
        self.streampager_show_ruler.as_ref()
    }
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
    /// Pager command for output that benefits from scrolling.
    pub pager: PagerSetting,
    /// When to paginate output.
    pub paginate: PaginateSetting,
    /// Configuration for the builtin streampager.
    pub streampager: StreampagerConfig,
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
            show_progress: resolve_ui_setting(
                &default_config.show_progress,
                default_overrides,
                user_config.and_then(|c| c.show_progress.as_ref()),
                user_overrides,
                host_platform,
                |data| data.show_progress.as_ref(),
            ),
            max_progress_running: resolve_ui_setting(
                &default_config.max_progress_running,
                default_overrides,
                user_config.and_then(|c| c.max_progress_running.as_ref()),
                user_overrides,
                host_platform,
                |data| data.max_progress_running.as_ref(),
            ),
            input_handler: resolve_ui_setting(
                &default_config.input_handler,
                default_overrides,
                user_config.and_then(|c| c.input_handler.as_ref()),
                user_overrides,
                host_platform,
                |data| data.input_handler.as_ref(),
            ),
            output_indent: resolve_ui_setting(
                &default_config.output_indent,
                default_overrides,
                user_config.and_then(|c| c.output_indent.as_ref()),
                user_overrides,
                host_platform,
                |data| data.output_indent.as_ref(),
            ),
            pager: resolve_ui_setting(
                &default_config.pager,
                default_overrides,
                user_config.and_then(|c| c.pager.as_ref()),
                user_overrides,
                host_platform,
                |data| data.pager.as_ref(),
            ),
            paginate: resolve_ui_setting(
                &default_config.paginate,
                default_overrides,
                user_config.and_then(|c| c.paginate.as_ref()),
                user_overrides,
                host_platform,
                |data| data.paginate.as_ref(),
            ),
            streampager: StreampagerConfig {
                interface: resolve_ui_setting(
                    &default_config.streampager.interface,
                    default_overrides,
                    user_config.and_then(|c| c.streampager.interface.as_ref()),
                    user_overrides,
                    host_platform,
                    |data| data.streampager_interface.as_ref(),
                ),
                wrapping: resolve_ui_setting(
                    &default_config.streampager.wrapping,
                    default_overrides,
                    user_config.and_then(|c| c.streampager.wrapping.as_ref()),
                    user_overrides,
                    host_platform,
                    |data| data.streampager_wrapping.as_ref(),
                ),
                show_ruler: resolve_ui_setting(
                    &default_config.streampager.show_ruler,
                    default_overrides,
                    user_config.and_then(|c| c.streampager.show_ruler.as_ref()),
                    user_overrides,
                    host_platform,
                    |data| data.streampager_show_ruler.as_ref(),
                ),
            },
        }
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

/// Controls when to paginate output.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaginateSetting {
    /// Automatically page if stdout is a TTY and output would benefit from it.
    #[default]
    Auto,
    /// Never use a pager.
    Never,
}

/// The special string that indicates the builtin pager should be used.
pub const BUILTIN_PAGER_NAME: &str = ":builtin";

/// Deserialized streampager configuration (all fields optional).
///
/// Used in user config and overrides where any field may be unspecified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::user_config) struct DeserializedStreampagerConfig {
    /// Interface mode controlling alternate screen behavior.
    pub(in crate::user_config) interface: Option<StreampagerInterface>,
    /// Text wrapping mode.
    pub(in crate::user_config) wrapping: Option<StreampagerWrapping>,
    /// Whether to show a ruler at the bottom.
    pub(in crate::user_config) show_ruler: Option<bool>,
}

/// Default streampager configuration (all fields required).
///
/// Used in the embedded default config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::user_config) struct DefaultStreampagerConfig {
    /// Interface mode controlling alternate screen behavior.
    pub(in crate::user_config) interface: StreampagerInterface,
    /// Text wrapping mode.
    pub(in crate::user_config) wrapping: StreampagerWrapping,
    /// Whether to show a ruler at the bottom.
    pub(in crate::user_config) show_ruler: bool,
}

/// Resolved streampager configuration.
///
/// These settings control behavior when `pager = ":builtin"` is configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreampagerConfig {
    /// Interface mode controlling alternate screen behavior.
    pub interface: StreampagerInterface,
    /// Text wrapping mode.
    pub wrapping: StreampagerWrapping,
    /// Whether to show a ruler at the bottom.
    pub show_ruler: bool,
}

impl StreampagerConfig {
    /// Converts to the streampager library's interface mode.
    pub fn streampager_interface_mode(&self) -> streampager::config::InterfaceMode {
        use streampager::config::InterfaceMode;
        match self.interface {
            StreampagerInterface::FullScreenClearOutput => InterfaceMode::FullScreen,
            StreampagerInterface::QuitIfOnePage => InterfaceMode::Hybrid,
            StreampagerInterface::QuitQuicklyOrClearOutput => {
                InterfaceMode::Delayed(std::time::Duration::from_secs(2))
            }
        }
    }

    /// Converts to the streampager library's wrapping mode.
    pub fn streampager_wrapping_mode(&self) -> streampager::config::WrappingMode {
        use streampager::config::WrappingMode;
        match self.wrapping {
            StreampagerWrapping::None => WrappingMode::Unwrapped,
            StreampagerWrapping::Word => WrappingMode::WordBoundary,
            StreampagerWrapping::Anywhere => WrappingMode::GraphemeBoundary,
        }
    }
}

/// Interface mode for the builtin streampager.
///
/// Controls how the pager uses the alternate screen and when it exits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreampagerInterface {
    /// Exit immediately if content fits on one page; otherwise use full screen
    /// and clear on exit.
    #[default]
    QuitIfOnePage,
    /// Always use full screen mode and clear the screen on exit.
    FullScreenClearOutput,
    /// Wait briefly before entering full screen; clear on exit if entered.
    QuitQuicklyOrClearOutput,
}

/// Text wrapping mode for the builtin streampager.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreampagerWrapping {
    /// Do not wrap text; allow horizontal scrolling.
    None,
    /// Wrap at word boundaries.
    #[default]
    Word,
    /// Wrap at any character (grapheme) boundary.
    Anywhere,
}

/// A command with optional arguments and environment variables.
///
/// Supports three input formats, all normalized to the same representation:
///
/// - String: `"less -FRX"` (split on whitespace)
/// - Array: `["less", "-FRX"]`
/// - Structured: `{ command = ["less", "-FRX"], env = { LESSCHARSET = "utf-8" } }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandNameAndArgs {
    /// The command and its arguments (non-empty after deserialization).
    command: Vec<String>,
    /// Environment variables to set when running the command.
    env: BTreeMap<String, String>,
}

impl CommandNameAndArgs {
    /// Returns the command name.
    pub fn command_name(&self) -> &str {
        // The command is validated to be non-empty during deserialization.
        &self.command[0]
    }

    /// Returns the arguments.
    pub fn args(&self) -> &[String] {
        &self.command[1..]
    }

    /// Creates a [`std::process::Command`] from this configuration.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(self.command_name());
        cmd.args(self.args());
        cmd.envs(&self.env);
        cmd
    }
}

impl<'de> Deserialize<'de> for CommandNameAndArgs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CommandNameAndArgsVisitor)
    }
}

/// Visitor for deserializing CommandNameAndArgs.
struct CommandNameAndArgsVisitor;

impl<'de> de::Visitor<'de> for CommandNameAndArgsVisitor {
    type Value = CommandNameAndArgs;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(
            "a command string (\"less -FRX\"), \
             an array ([\"less\", \"-FRX\"]), \
             or a table ({ command = [\"less\", \"-FRX\"], env = { ... } })",
        )
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        let command: Vec<String> = shell_words::split(v).map_err(de::Error::custom)?;
        if command.is_empty() {
            return Err(de::Error::custom("command string must not be empty"));
        }
        Ok(CommandNameAndArgs {
            command,
            env: BTreeMap::new(),
        })
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut command = Vec::new();
        while let Some(arg) = seq.next_element::<String>()? {
            command.push(arg);
        }
        if command.is_empty() {
            return Err(de::Error::custom("command array must not be empty"));
        }
        Ok(CommandNameAndArgs {
            command,
            env: BTreeMap::new(),
        })
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        #[derive(Deserialize)]
        struct StructuredInner {
            command: Vec<String>,
            #[serde(default)]
            env: BTreeMap<String, String>,
        }

        let inner = StructuredInner::deserialize(de::value::MapAccessDeserializer::new(map))?;
        if inner.command.is_empty() {
            return Err(de::Error::custom("command array must not be empty"));
        }
        Ok(CommandNameAndArgs {
            command: inner.command,
            env: inner.env,
        })
    }
}

/// Controls which pager to use for output that benefits from scrolling.
///
/// This specifies *which* pager to use; whether to actually paginate is
/// controlled by [`PaginateSetting`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagerSetting {
    /// Use the builtin streampager.
    Builtin,
    /// Use an external command.
    External(CommandNameAndArgs),
}

// Only used in unit tests -- in regular code, the default is looked up via
// default-user-config.toml.
#[cfg(test)]
impl Default for PagerSetting {
    fn default() -> Self {
        Self::External(CommandNameAndArgs {
            command: vec!["less".to_owned(), "-FRX".to_owned()],
            env: [("LESSCHARSET".to_owned(), "utf-8".to_owned())]
                .into_iter()
                .collect(),
        })
    }
}

impl<'de> Deserialize<'de> for PagerSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PagerSettingVisitor)
    }
}

/// Visitor for deserializing PagerSetting.
struct PagerSettingVisitor;

impl<'de> de::Visitor<'de> for PagerSettingVisitor {
    type Value = PagerSetting;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .write_str("\":builtin\", a command string, an array, or a table with command and env")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        // Check for the special ":builtin" value.
        if v == BUILTIN_PAGER_NAME {
            return Ok(PagerSetting::Builtin);
        }
        let cmd = CommandNameAndArgsVisitor.visit_str(v)?;
        Ok(PagerSetting::External(cmd))
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        let args = CommandNameAndArgsVisitor.visit_seq(seq)?;
        Ok(PagerSetting::External(args))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let args = CommandNameAndArgsVisitor.visit_map(map)?;
        Ok(PagerSetting::External(args))
    }
}

/// Visitor for deserializing max-progress-running (string or integer).
struct MaxProgressRunningVisitor;

impl<'de> de::Visitor<'de> for MaxProgressRunningVisitor {
    type Value = MaxProgressRunning;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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
            output_indent: Some(false),
            ..Default::default()
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
                input_handler: Some(false),
                ..Default::default()
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
                input_handler: Some(false),
                ..Default::default()
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
                pager: Some(PagerSetting::default()),
                paginate: Some(PaginateSetting::Never),
                streampager: Default::default(),
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
            ..Default::default()
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

    #[test]
    fn test_paginate_setting_parsing() {
        // Test "auto".
        let config: DeserializedUiConfig = toml::from_str(r#"paginate = "auto""#).unwrap();
        assert_eq!(config.paginate, Some(PaginateSetting::Auto));

        // Test "never".
        let config: DeserializedUiConfig = toml::from_str(r#"paginate = "never""#).unwrap();
        assert_eq!(config.paginate, Some(PaginateSetting::Never));

        // Test missing value.
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.paginate.is_none());

        // Test invalid value.
        let err = toml::from_str::<DeserializedUiConfig>(r#"paginate = "invalid""#).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant"),
            "error should mention 'unknown variant': {err}"
        );
    }

    #[test]
    fn test_command_name_and_args_parsing() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            cmd: CommandNameAndArgs,
        }

        // String format: split using shell word parsing.
        let wrapper: Wrapper = toml::from_str(r#"cmd = "less -FRX""#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec!["less".to_owned(), "-FRX".to_owned()],
                env: BTreeMap::new(),
            }
        );
        assert_eq!(wrapper.cmd.command_name(), "less");
        assert_eq!(wrapper.cmd.args(), &["-FRX".to_owned()]);

        // Array format: each element is a separate argument.
        let wrapper: Wrapper = toml::from_str(r#"cmd = ["less", "-F", "-R", "-X"]"#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec![
                    "less".to_owned(),
                    "-F".to_owned(),
                    "-R".to_owned(),
                    "-X".to_owned()
                ],
                env: BTreeMap::new(),
            }
        );
        assert_eq!(wrapper.cmd.command_name(), "less");
        assert_eq!(
            wrapper.cmd.args(),
            &["-F".to_owned(), "-R".to_owned(), "-X".to_owned()]
        );

        // Structured format: command array with optional env.
        let cmd: CommandNameAndArgs = toml::from_str(
            r#"
            command = ["less", "-FRX"]
            env = { LESSCHARSET = "utf-8" }
            "#,
        )
        .unwrap();
        let expected_env: BTreeMap<String, String> =
            [("LESSCHARSET".to_owned(), "utf-8".to_owned())]
                .into_iter()
                .collect();
        assert_eq!(
            cmd,
            CommandNameAndArgs {
                command: vec!["less".to_owned(), "-FRX".to_owned()],
                env: expected_env,
            }
        );
        assert_eq!(cmd.command_name(), "less");
        assert_eq!(cmd.args(), &["-FRX".to_owned()]);

        // Shell quoting: double quotes preserve spaces.
        let wrapper: Wrapper = toml::from_str(r#"cmd = 'my-pager "arg with spaces"'"#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec!["my-pager".to_owned(), "arg with spaces".to_owned()],
                env: BTreeMap::new(),
            }
        );

        // Shell quoting: single quotes preserve spaces.
        let wrapper: Wrapper = toml::from_str(r#"cmd = "my-pager 'arg with spaces'""#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec!["my-pager".to_owned(), "arg with spaces".to_owned()],
                env: BTreeMap::new(),
            }
        );

        // Shell quoting: escaped quotes within double quotes.
        let wrapper: Wrapper =
            toml::from_str(r#"cmd = 'my-pager "quoted \"nested\" arg"'"#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec!["my-pager".to_owned(), "quoted \"nested\" arg".to_owned()],
                env: BTreeMap::new(),
            }
        );

        // Shell quoting: path with spaces.
        let wrapper: Wrapper = toml::from_str(r#"cmd = '"/path/to/my pager" --flag'"#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec!["/path/to/my pager".to_owned(), "--flag".to_owned()],
                env: BTreeMap::new(),
            }
        );

        // Shell quoting: multiple quoted arguments.
        let wrapper: Wrapper =
            toml::from_str(r#"cmd = 'cmd "first arg" "second arg" third'"#).unwrap();
        assert_eq!(
            wrapper.cmd,
            CommandNameAndArgs {
                command: vec![
                    "cmd".to_owned(),
                    "first arg".to_owned(),
                    "second arg".to_owned(),
                    "third".to_owned(),
                ],
                env: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn test_command_and_pager_empty_errors() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            #[expect(dead_code)]
            cmd: CommandNameAndArgs,
        }

        // Test CommandNameAndArgs empty cases.
        let cmd_cases = [
            ("empty array", "cmd = []"),
            ("empty string", r#"cmd = """#),
            ("whitespace-only string", r#"cmd = "   ""#),
            (
                "structured with empty command",
                r#"cmd = { command = [], env = { LESSCHARSET = "utf-8" } }"#,
            ),
        ];

        for (name, input) in cmd_cases {
            let err = toml::from_str::<Wrapper>(input).unwrap_err();
            assert!(
                err.to_string().contains("must not be empty"),
                "CommandNameAndArgs {name}: error should mention 'must not be empty': {err}"
            );
        }

        // Test PagerSetting empty cases (via DeserializedUiConfig).
        let pager_cases = [
            ("empty array", "pager = []"),
            ("empty string", r#"pager = """#),
        ];

        for (name, input) in pager_cases {
            let err = toml::from_str::<DeserializedUiConfig>(input).unwrap_err();
            assert!(
                err.to_string().contains("must not be empty"),
                "PagerSetting {name}: error should mention 'must not be empty': {err}"
            );
        }

        // Test invalid shell quoting (unclosed quotes).
        let unclosed_quote_cases = [
            ("unclosed double quote", r#"cmd = 'pager "unclosed'"#),
            ("unclosed single quote", r#"cmd = "pager 'unclosed""#),
        ];

        for (name, input) in unclosed_quote_cases {
            let err = toml::from_str::<Wrapper>(input).unwrap_err();
            assert!(
                err.to_string().contains("missing closing quote"),
                "CommandNameAndArgs {name}: error should mention 'missing closing quote': {err}"
            );
        }
    }

    #[test]
    fn test_command_name_and_args_to_command() {
        // Test that to_command produces a valid Command.
        let cmd = CommandNameAndArgs {
            command: vec!["echo".to_owned(), "hello".to_owned()],
            env: BTreeMap::new(),
        };
        let std_cmd = cmd.to_command();
        assert_eq!(cmd.command_name(), "echo");
        drop(std_cmd);
    }

    #[test]
    fn test_pager_setting_parsing() {
        // String format.
        let config: DeserializedUiConfig = toml::from_str(r#"pager = "less -FRX""#).unwrap();
        assert_eq!(
            config.pager,
            Some(PagerSetting::External(CommandNameAndArgs {
                command: vec!["less".to_owned(), "-FRX".to_owned()],
                env: BTreeMap::new(),
            }))
        );

        // Array format.
        let config: DeserializedUiConfig = toml::from_str(r#"pager = ["less", "-FRX"]"#).unwrap();
        assert_eq!(
            config.pager,
            Some(PagerSetting::External(CommandNameAndArgs {
                command: vec!["less".to_owned(), "-FRX".to_owned()],
                env: BTreeMap::new(),
            }))
        );

        // Structured format with env.
        let config: DeserializedUiConfig = toml::from_str(
            r#"
            [pager]
            command = ["less", "-FRX"]
            env = { LESSCHARSET = "utf-8" }
            "#,
        )
        .unwrap();
        let expected_env: BTreeMap<String, String> =
            [("LESSCHARSET".to_owned(), "utf-8".to_owned())]
                .into_iter()
                .collect();
        assert_eq!(
            config.pager,
            Some(PagerSetting::External(CommandNameAndArgs {
                command: vec!["less".to_owned(), "-FRX".to_owned()],
                env: expected_env,
            }))
        );

        // Missing pager (None).
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert!(config.pager.is_none());
    }

    #[test]
    fn test_resolved_ui_config_pager_defaults() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[], &host);

        // Resolved values should match the embedded defaults.
        assert_eq!(resolved.pager, defaults.pager);
        assert_eq!(resolved.paginate, defaults.paginate);
    }

    #[test]
    fn test_resolved_ui_config_pager_override() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create an override that sets a custom pager.
        let custom_pager = PagerSetting::External(CommandNameAndArgs {
            command: vec!["more".to_owned()],
            env: BTreeMap::new(),
        });
        let override_ = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                pager: Some(custom_pager.clone()),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override_], &host);

        assert_eq!(resolved.pager, custom_pager);
        // paginate should still be from defaults.
        assert_eq!(resolved.paginate, defaults.paginate);
    }

    #[test]
    fn test_resolved_ui_config_paginate_override() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Create an override that sets paginate to "never".
        let override_ = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                paginate: Some(PaginateSetting::Never),
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override_], &host);

        assert_eq!(resolved.paginate, PaginateSetting::Never);
        // pager should still be from defaults.
        assert_eq!(resolved.pager, defaults.pager);
    }

    #[test]
    fn test_pager_setting_builtin() {
        // `:builtin` special string.
        let config: DeserializedUiConfig = toml::from_str(r#"pager = ":builtin""#).unwrap();
        assert_eq!(config.pager, Some(PagerSetting::Builtin));
    }

    #[test]
    fn test_streampager_config_parsing() {
        // Full config.
        let config: DeserializedUiConfig = toml::from_str(
            r#"
            [streampager]
            interface = "full-screen-clear-output"
            wrapping = "anywhere"
            show-ruler = false
            "#,
        )
        .unwrap();
        assert_eq!(
            config.streampager.interface,
            Some(StreampagerInterface::FullScreenClearOutput)
        );
        assert_eq!(
            config.streampager.wrapping,
            Some(StreampagerWrapping::Anywhere)
        );
        assert_eq!(config.streampager.show_ruler, Some(false));

        // Partial config - unspecified fields are None.
        let config: DeserializedUiConfig = toml::from_str(
            r#"
            [streampager]
            interface = "quit-quickly-or-clear-output"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.streampager.interface,
            Some(StreampagerInterface::QuitQuicklyOrClearOutput)
        );
        assert_eq!(config.streampager.wrapping, None);
        assert_eq!(config.streampager.show_ruler, None);

        // Empty config - all fields are None.
        let config: DeserializedUiConfig = toml::from_str("").unwrap();
        assert_eq!(config.streampager.interface, None);
        assert_eq!(config.streampager.wrapping, None);
        assert_eq!(config.streampager.show_ruler, None);
    }

    #[test]
    fn test_streampager_config_resolution() {
        let defaults = DefaultUserConfig::from_embedded().ui;

        // Override just the interface.
        let override_ = make_override(
            "cfg(all())",
            DeserializedUiOverrideData {
                streampager: DeserializedStreampagerConfig {
                    interface: Some(StreampagerInterface::FullScreenClearOutput),
                    wrapping: None,
                    show_ruler: None,
                },
                ..Default::default()
            },
        );

        let host = detect_host_platform_for_tests();
        let resolved = UiConfig::resolve(&defaults, &[], None, &[override_], &host);

        // Interface should be overridden.
        assert_eq!(
            resolved.streampager.interface,
            StreampagerInterface::FullScreenClearOutput
        );
        // wrapping and show_ruler should be from defaults.
        assert_eq!(resolved.streampager.wrapping, defaults.streampager.wrapping);
        assert_eq!(
            resolved.streampager.show_ruler,
            defaults.streampager.show_ruler
        );
    }
}
