// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Nextest version configuration.

use super::{NextestConfig, ToolConfigFile, ToolName};
use crate::errors::{ConfigParseError, ConfigParseErrorKind};
use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use std::{borrow::Cow, collections::BTreeSet, fmt, str::FromStr};

/// A "version-only" form of the nextest configuration.
///
/// This is used as a first pass to determine the required nextest version before parsing the rest
/// of the configuration. That avoids issues parsing incompatible configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VersionOnlyConfig {
    /// The nextest version configuration.
    nextest_version: NextestVersionConfig,

    /// Experimental features configuration.
    experimental: ExperimentalConfig,
}

impl VersionOnlyConfig {
    /// Reads the nextest version configuration from the given sources.
    ///
    /// See [`NextestConfig::from_sources`] for more details.
    pub fn from_sources<'a, I>(
        workspace_root: &Utf8Path,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        let tool_config_files_rev = tool_config_files.into_iter().rev();

        Self::read_from_sources(workspace_root, config_file, tool_config_files_rev)
    }

    /// Returns the nextest version requirement.
    pub fn nextest_version(&self) -> &NextestVersionConfig {
        &self.nextest_version
    }

    /// Returns the experimental features configuration.
    pub fn experimental(&self) -> &ExperimentalConfig {
        &self.experimental
    }

    fn read_from_sources<'a>(
        workspace_root: &Utf8Path,
        config_file: Option<&Utf8Path>,
        tool_config_files_rev: impl Iterator<Item = &'a ToolConfigFile>,
    ) -> Result<Self, ConfigParseError> {
        let mut nextest_version = NextestVersionConfig::default();
        let mut known = BTreeSet::new();
        let mut unknown = BTreeSet::new();

        // Merge in tool configs.
        for ToolConfigFile { config_file, tool } in tool_config_files_rev {
            if let Some(v) = Self::read_and_deserialize(config_file, Some(tool))?.nextest_version {
                nextest_version.accumulate(v, Some(tool.clone()));
            }
        }

        // Finally, merge in the repo config.
        let config_file = match config_file {
            Some(file) => Some(Cow::Borrowed(file)),
            None => {
                let config_file = workspace_root.join(NextestConfig::CONFIG_PATH);
                config_file.exists().then_some(Cow::Owned(config_file))
            }
        };
        if let Some(config_file) = config_file {
            let d = Self::read_and_deserialize(&config_file, None)?;
            if let Some(v) = d.nextest_version {
                nextest_version.accumulate(v, None);
            }

            // Process experimental features. Unknown features are stored rather
            // than immediately causing an error, so that the nextest version
            // check can run first.
            known.extend(d.experimental.known);
            unknown.extend(d.experimental.unknown);
        }

        Ok(Self {
            nextest_version,
            experimental: ExperimentalConfig { known, unknown },
        })
    }

    fn read_and_deserialize(
        config_file: &Utf8Path,
        tool: Option<&ToolName>,
    ) -> Result<VersionOnlyDeserialize, ConfigParseError> {
        let toml_str = std::fs::read_to_string(config_file.as_str()).map_err(|error| {
            ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::VersionOnlyReadError(error),
            )
        })?;
        let toml_de = toml::de::Deserializer::parse(&toml_str).map_err(|error| {
            ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::TomlParseError(Box::new(error)),
            )
        })?;
        let v: VersionOnlyDeserialize =
            serde_path_to_error::deserialize(toml_de).map_err(|error| {
                ConfigParseError::new(
                    config_file,
                    tool,
                    ConfigParseErrorKind::VersionOnlyDeserializeError(Box::new(error)),
                )
            })?;
        if tool.is_some() && !v.experimental.is_empty() {
            return Err(ConfigParseError::new(
                config_file,
                tool,
                ConfigParseErrorKind::ExperimentalFeaturesInToolConfig {
                    features: v.experimental.feature_names(),
                },
            ));
        }

        Ok(v)
    }
}

/// A version of configuration that only deserializes the nextest version.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct VersionOnlyDeserialize {
    #[serde(default)]
    nextest_version: Option<NextestVersionDeserialize>,
    #[serde(default)]
    experimental: ExperimentalDeserialize,
}

/// Intermediate representation for experimental config deserialization.
///
/// This supports both the table format (`[experimental] setup-scripts = true`)
/// and the array format (`experimental = ["setup-scripts"]`). The array format
/// will be deprecated in the future.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ExperimentalDeserialize {
    /// Known experimental features that are enabled.
    known: BTreeSet<ConfigExperimental>,
    /// Unknown feature names (for error reporting).
    unknown: BTreeSet<String>,
}

impl ExperimentalDeserialize {
    /// Returns true if no experimental features are specified.
    fn is_empty(&self) -> bool {
        self.known.is_empty() && self.unknown.is_empty()
    }

    /// Returns the feature names for error messages (used by tool config
    /// validation).
    fn feature_names(&self) -> BTreeSet<String> {
        let mut names = self.unknown.clone();
        for feature in &self.known {
            names.insert(feature.to_string());
        }
        names
    }
}

impl<'de> Deserialize<'de> for ExperimentalDeserialize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ExperimentalVisitor;

        impl<'de> Visitor<'de> for ExperimentalVisitor {
            type Value = ExperimentalDeserialize;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "a table ({ setup-scripts = true, benchmarks = true }) \
                     or an array ([\"setup-scripts\", \"benchmarks\"])",
                )
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // Array format: parse each string to ConfigExperimental.
                let mut known = BTreeSet::new();
                let mut unknown = BTreeSet::new();
                while let Some(feature_str) = seq.next_element::<String>()? {
                    if let Ok(feature) = feature_str.parse::<ConfigExperimental>() {
                        known.insert(feature);
                    } else {
                        unknown.insert(feature_str);
                    }
                }
                Ok(ExperimentalDeserialize { known, unknown })
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                // Table format: use typed struct with serde_ignored for unknown
                // fields.
                #[derive(Deserialize)]
                #[serde(rename_all = "kebab-case")]
                struct TableConfig {
                    #[serde(default)]
                    setup_scripts: bool,
                    #[serde(default)]
                    wrapper_scripts: bool,
                    #[serde(default)]
                    benchmarks: bool,
                }

                let mut unknown = BTreeSet::new();
                let de = serde::de::value::MapAccessDeserializer::new(map);
                let mut cb = |path: serde_ignored::Path| {
                    unknown.insert(path.to_string());
                };
                let ignored_de = serde_ignored::Deserializer::new(de, &mut cb);
                let TableConfig {
                    setup_scripts,
                    wrapper_scripts,
                    benchmarks,
                } = Deserialize::deserialize(ignored_de).map_err(serde::de::Error::custom)?;

                let mut known = BTreeSet::new();
                if setup_scripts {
                    known.insert(ConfigExperimental::SetupScripts);
                }
                if wrapper_scripts {
                    known.insert(ConfigExperimental::WrapperScripts);
                }
                if benchmarks {
                    known.insert(ConfigExperimental::Benchmarks);
                }

                Ok(ExperimentalDeserialize { known, unknown })
            }
        }

        deserializer.deserialize_any(ExperimentalVisitor)
    }
}

/// Nextest version configuration.
///
/// Similar to the [`rust-version`
/// field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field),
/// `nextest-version` lets you specify the minimum required version of nextest for a repository.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NextestVersionConfig {
    /// The minimum version of nextest to produce an error before.
    pub required: NextestVersionReq,

    /// The minimum version of nextest to produce a warning before.
    ///
    /// This might be lower than [`Self::required`], in which case it is ignored. [`Self::eval`]
    /// checks for required versions before it checks for recommended versions.
    pub recommended: NextestVersionReq,
}

impl NextestVersionConfig {
    /// Accumulates a deserialized version requirement into this configuration.
    pub(crate) fn accumulate(&mut self, v: NextestVersionDeserialize, v_tool: Option<ToolName>) {
        if let Some(version) = v.required {
            self.required.accumulate(version, v_tool.clone());
        }
        if let Some(version) = v.recommended {
            self.recommended.accumulate(version, v_tool);
        }
    }

    /// Returns whether the given version satisfies the nextest version requirement.
    pub fn eval(
        &self,
        current_version: &Version,
        override_version_check: bool,
    ) -> NextestVersionEval {
        match self.required.satisfies(current_version) {
            Ok(()) => {}
            Err((required, tool)) => {
                if override_version_check {
                    return NextestVersionEval::ErrorOverride {
                        required: required.clone(),
                        current: current_version.clone(),
                        tool: tool.cloned(),
                    };
                } else {
                    return NextestVersionEval::Error {
                        required: required.clone(),
                        current: current_version.clone(),
                        tool: tool.cloned(),
                    };
                }
            }
        }

        match self.recommended.satisfies(current_version) {
            Ok(()) => NextestVersionEval::Satisfied,
            Err((recommended, tool)) => {
                if override_version_check {
                    NextestVersionEval::WarnOverride {
                        recommended: recommended.clone(),
                        current: current_version.clone(),
                        tool: tool.cloned(),
                    }
                } else {
                    NextestVersionEval::Warn {
                        recommended: recommended.clone(),
                        current: current_version.clone(),
                        tool: tool.cloned(),
                    }
                }
            }
        }
    }
}

/// Experimental features configuration.
///
/// This stores both known and unknown experimental features. Unknown features are stored rather
/// than immediately causing an error, so that the nextest version check can run first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExperimentalConfig {
    /// Known experimental features that are enabled.
    known: BTreeSet<ConfigExperimental>,

    /// Unknown experimental feature names.
    unknown: BTreeSet<String>,
}

impl ExperimentalConfig {
    /// Returns the known experimental features that are enabled.
    pub fn known(&self) -> &BTreeSet<ConfigExperimental> {
        &self.known
    }

    /// Evaluates the experimental configuration.
    ///
    /// This should be called after the nextest version check, so that the version error takes
    /// precedence over unknown experimental features (a future version may have new features).
    pub fn eval(&self) -> ExperimentalConfigEval {
        if self.unknown.is_empty() {
            ExperimentalConfigEval::Satisfied
        } else {
            ExperimentalConfigEval::UnknownFeatures {
                unknown: self.unknown.clone(),
                known: ConfigExperimental::known_features().collect(),
            }
        }
    }
}

/// The result of evaluating an [`ExperimentalConfig`].
///
/// Returned by [`ExperimentalConfig::eval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentalConfigEval {
    /// All experimental features are known.
    Satisfied,

    /// Unknown experimental features were found.
    UnknownFeatures {
        /// The set of unknown feature names.
        unknown: BTreeSet<String>,

        /// The set of known features.
        known: BTreeSet<ConfigExperimental>,
    },
}

impl ExperimentalConfigEval {
    /// Converts this eval result into an error, if it represents an error condition.
    ///
    /// Returns `Some(ConfigParseError)` if this is `UnknownFeatures`, and `None` if `Satisfied`.
    pub fn into_error(self, config_file: impl Into<Utf8PathBuf>) -> Option<ConfigParseError> {
        match self {
            ExperimentalConfigEval::Satisfied => None,
            ExperimentalConfigEval::UnknownFeatures { unknown, known } => {
                Some(ConfigParseError::new(
                    config_file,
                    None,
                    ConfigParseErrorKind::UnknownExperimentalFeatures { unknown, known },
                ))
            }
        }
    }
}

/// Experimental configuration features.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[non_exhaustive]
pub enum ConfigExperimental {
    /// Enable support for setup scripts.
    SetupScripts,
    /// Enable support for wrapper scripts.
    WrapperScripts,
    /// Enable support for benchmarks.
    Benchmarks,
}

impl ConfigExperimental {
    /// Returns an iterator over all known experimental features.
    pub fn known_features() -> impl Iterator<Item = Self> {
        vec![Self::SetupScripts, Self::WrapperScripts, Self::Benchmarks].into_iter()
    }

    /// Returns the environment variable name for this feature, if any.
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::SetupScripts => None,
            Self::WrapperScripts => None,
            Self::Benchmarks => Some("NEXTEST_EXPERIMENTAL_BENCHMARKS"),
        }
    }

    /// Returns the set of experimental features enabled via environment variables.
    pub fn from_env() -> std::collections::BTreeSet<Self> {
        let mut set = std::collections::BTreeSet::new();
        for feature in Self::known_features() {
            if let Some(env_var) = feature.env_var()
                && std::env::var(env_var).as_deref() == Ok("1")
            {
                set.insert(feature);
            }
        }
        set
    }
}

impl FromStr for ConfigExperimental {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "setup-scripts" => Ok(Self::SetupScripts),
            "wrapper-scripts" => Ok(Self::WrapperScripts),
            "benchmarks" => Ok(Self::Benchmarks),
            _ => Err(()),
        }
    }
}

impl fmt::Display for ConfigExperimental {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetupScripts => write!(f, "setup-scripts"),
            Self::WrapperScripts => write!(f, "wrapper-scripts"),
            Self::Benchmarks => write!(f, "benchmarks"),
        }
    }
}

/// Specification for a nextest version. Part of [`NextestVersionConfig`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum NextestVersionReq {
    /// A version was specified.
    Version {
        /// The version to warn before.
        version: Version,

        /// The tool which produced this version specification.
        tool: Option<ToolName>,
    },

    /// No version was specified.
    #[default]
    None,
}

impl NextestVersionReq {
    fn accumulate(&mut self, v: Version, v_tool: Option<ToolName>) {
        match self {
            NextestVersionReq::Version { version, tool } => {
                // This is v >= version rather than v > version, so that if multiple tools specify
                // the same version, the last tool wins.
                if &v >= version {
                    *version = v;
                    *tool = v_tool;
                }
            }
            NextestVersionReq::None => {
                *self = NextestVersionReq::Version {
                    version: v,
                    tool: v_tool,
                };
            }
        }
    }

    fn satisfies(&self, version: &Version) -> Result<(), (&Version, Option<&ToolName>)> {
        match self {
            NextestVersionReq::Version {
                version: required,
                tool,
            } => {
                if version >= required {
                    Ok(())
                } else {
                    Err((required, tool.as_ref()))
                }
            }
            NextestVersionReq::None => Ok(()),
        }
    }
}

/// The result of checking whether a [`NextestVersionConfig`] satisfies a requirement.
///
/// Returned by [`NextestVersionConfig::eval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextestVersionEval {
    /// The version satisfies the requirement.
    Satisfied,

    /// An error should be produced.
    Error {
        /// The minimum version required.
        required: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<ToolName>,
    },

    /// A warning should be produced.
    Warn {
        /// The minimum version recommended.
        recommended: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<ToolName>,
    },

    /// An error should be produced but the version is overridden.
    ErrorOverride {
        /// The minimum version recommended.
        required: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<ToolName>,
    },

    /// A warning should be produced but the version is overridden.
    WarnOverride {
        /// The minimum version recommended.
        recommended: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<ToolName>,
    },
}

/// Nextest version configuration.
///
/// Similar to the [`rust-version`
/// field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field),
/// `nextest-version` lets you specify the minimum required version of nextest for a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NextestVersionDeserialize {
    /// The minimum version of nextest that this repository requires.
    required: Option<Version>,

    /// The minimum version of nextest that this repository produces a warning against.
    recommended: Option<Version>,
}

impl<'de> Deserialize<'de> for NextestVersionDeserialize {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;

        impl<'de2> serde::de::Visitor<'de2> for V {
            type Value = NextestVersionDeserialize;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a table ({{ required = \"0.9.20\", recommended = \"0.9.30\" }}) or a string (\"0.9.50\")",
                )
            }

            fn visit_str<E>(self, s: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let required = parse_version::<E>(s.to_owned())?;
                Ok(NextestVersionDeserialize {
                    required: Some(required),
                    recommended: None,
                })
            }

            fn visit_map<A>(self, map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de2>,
            {
                #[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
                struct NextestVersionMap {
                    #[serde(default, deserialize_with = "deserialize_version_opt")]
                    required: Option<Version>,
                    #[serde(default, deserialize_with = "deserialize_version_opt")]
                    recommended: Option<Version>,
                }

                let NextestVersionMap {
                    required,
                    recommended,
                } = NextestVersionMap::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))?;

                if let (Some(required), Some(recommended)) = (&required, &recommended)
                    && required > recommended
                {
                    return Err(serde::de::Error::custom(format!(
                        "required version ({required}) must not be greater than recommended version ({recommended})"
                    )));
                }

                Ok(NextestVersionDeserialize {
                    required,
                    recommended,
                })
            }
        }

        deserializer.deserialize_any(V)
    }
}

/// This has similar logic to the [`rust-version`
/// field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field).
///
/// Adapted from cargo_metadata
fn deserialize_version_opt<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Version>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = Option::<String>::deserialize(deserializer)?;
    s.map(parse_version::<D::Error>).transpose()
}

fn parse_version<E>(mut s: String) -> std::result::Result<Version, E>
where
    E: serde::de::Error,
{
    for ch in s.chars() {
        if ch == '-' {
            return Err(E::custom(
                "pre-release identifiers are not supported in nextest-version",
            ));
        } else if ch == '+' {
            return Err(E::custom(
                "build metadata is not supported in nextest-version",
            ));
        }
    }

    // The major.minor format is not used with nextest 0.9, but support it anyway to match
    // rust-version.
    if s.matches('.').count() == 1 {
        // e.g. 1.0 -> 1.0.0
        s.push_str(".0");
    }

    Version::parse(&s).map_err(E::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(
        r#"
            nextest-version = "0.9"
        "#,
        NextestVersionDeserialize { required: Some("0.9.0".parse().unwrap()), recommended: None } ; "basic"
    )]
    #[test_case(
        r#"
            nextest-version = "0.9.30"
        "#,
        NextestVersionDeserialize { required: Some("0.9.30".parse().unwrap()), recommended: None } ; "basic with patch"
    )]
    #[test_case(
        r#"
            nextest-version = { recommended = "0.9.20" }
        "#,
        NextestVersionDeserialize { required: None, recommended: Some("0.9.20".parse().unwrap()) } ; "with warning"
    )]
    #[test_case(
        r#"
            nextest-version = { required = "0.9.20", recommended = "0.9.25" }
        "#,
        NextestVersionDeserialize {
            required: Some("0.9.20".parse().unwrap()),
            recommended: Some("0.9.25".parse().unwrap()),
        } ; "with error and warning"
    )]
    fn test_valid_nextest_version(input: &str, expected: NextestVersionDeserialize) {
        let actual: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert_eq!(actual.nextest_version.unwrap(), expected);
    }

    #[test_case(
        r#"
            nextest-version = 42
        "#,
        "a table ({{ required = \"0.9.20\", recommended = \"0.9.30\" }}) or a string (\"0.9.50\")" ; "empty"
    )]
    #[test_case(
        r#"
            nextest-version = "0.9.30-rc.1"
        "#,
        "pre-release identifiers are not supported in nextest-version" ; "pre-release"
    )]
    #[test_case(
        r#"
            nextest-version = "0.9.40+mybuild"
        "#,
        "build metadata is not supported in nextest-version" ; "build metadata"
    )]
    #[test_case(
        r#"
            nextest-version = { required = "0.9.20", recommended = "0.9.10" }
        "#,
        "required version (0.9.20) must not be greater than recommended version (0.9.10)" ; "error greater than warning"
    )]
    fn test_invalid_nextest_version(input: &str, error_message: &str) {
        let err = toml::from_str::<VersionOnlyDeserialize>(input).unwrap_err();
        assert!(
            err.to_string().contains(error_message),
            "error `{err}` contains `{error_message}`"
        );
    }

    fn tool_name(s: &str) -> ToolName {
        ToolName::new(s.into()).unwrap()
    }

    #[test]
    fn test_accumulate() {
        let mut nextest_version = NextestVersionConfig::default();
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: Some("0.9.20".parse().unwrap()),
                recommended: None,
            },
            Some(tool_name("tool1")),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: Some("0.9.30".parse().unwrap()),
                recommended: Some("0.9.35".parse().unwrap()),
            },
            Some(tool_name("tool2")),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: None,
                // This recommended version is ignored since it is less than the last recommended
                // version.
                recommended: Some("0.9.25".parse().unwrap()),
            },
            Some(tool_name("tool3")),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                // This is accepted because it is the same as the last required version, and the
                // last tool wins.
                required: Some("0.9.30".parse().unwrap()),
                recommended: None,
            },
            Some(tool_name("tool4")),
        );

        assert_eq!(
            nextest_version,
            NextestVersionConfig {
                required: NextestVersionReq::Version {
                    version: "0.9.30".parse().unwrap(),
                    tool: Some(tool_name("tool4")),
                },
                recommended: NextestVersionReq::Version {
                    version: "0.9.35".parse().unwrap(),
                    tool: Some(tool_name("tool2")),
                },
            }
        );
    }

    #[test]
    fn test_from_env_benchmarks() {
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("NEXTEST_EXPERIMENTAL_BENCHMARKS", "1") };
        assert!(ConfigExperimental::from_env().contains(&ConfigExperimental::Benchmarks));

        // Other values do not enable the feature.
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("NEXTEST_EXPERIMENTAL_BENCHMARKS", "0") };
        assert!(!ConfigExperimental::from_env().contains(&ConfigExperimental::Benchmarks));

        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("NEXTEST_EXPERIMENTAL_BENCHMARKS", "true") };
        assert!(!ConfigExperimental::from_env().contains(&ConfigExperimental::Benchmarks));

        // SetupScripts and WrapperScripts have no env vars, so they are never
        // enabled via from_env.
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("NEXTEST_EXPERIMENTAL_BENCHMARKS", "1") };
        let set = ConfigExperimental::from_env();
        assert!(!set.contains(&ConfigExperimental::SetupScripts));
        assert!(!set.contains(&ConfigExperimental::WrapperScripts));
    }

    #[test]
    fn test_experimental_formats() {
        // For the array format, valid features should parse correctly.
        let input = r#"experimental = ["setup-scripts", "benchmarks"]"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert_eq!(
            d.experimental.known,
            BTreeSet::from([
                ConfigExperimental::SetupScripts,
                ConfigExperimental::Benchmarks
            ]),
            "expected 2 known features"
        );
        assert!(d.experimental.unknown.is_empty());

        // An empty array is empty.
        let input = r#"experimental = []"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert!(
            d.experimental.is_empty(),
            "expected empty, got {:?}",
            d.experimental
        );

        // Unknown features in the array format are recorded.
        let input = r#"experimental = ["setup-scripts", "unknown-feature"]"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert_eq!(
            d.experimental.known,
            BTreeSet::from([ConfigExperimental::SetupScripts])
        );
        assert_eq!(
            d.experimental.unknown,
            BTreeSet::from(["unknown-feature".to_owned()])
        );

        // Table format: valid features parse correctly.
        let input = r#"
[experimental]
setup-scripts = true
benchmarks = true
"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert_eq!(
            d.experimental.known,
            BTreeSet::from([
                ConfigExperimental::SetupScripts,
                ConfigExperimental::Benchmarks
            ])
        );
        assert!(d.experimental.unknown.is_empty());

        // Empty table is empty.
        let input = r#"[experimental]"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert!(
            d.experimental.is_empty(),
            "expected empty, got {:?}",
            d.experimental
        );

        // If all features are false, the result is empty.
        let input = r#"
[experimental]
setup-scripts = false
"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert!(
            d.experimental.is_empty(),
            "expected empty, got {:?}",
            d.experimental
        );

        // Unknown features in the table format are recorded.
        let input = r#"
[experimental]
setup-scripts = true
unknown-feature = true
"#;
        let d: VersionOnlyDeserialize = toml::from_str(input).unwrap();
        assert_eq!(
            d.experimental.known,
            BTreeSet::from([ConfigExperimental::SetupScripts])
        );
        assert!(d.experimental.unknown.contains("unknown-feature"));

        // An invalid type shows a helpful error mentioning both formats.
        let input = r#"experimental = 42"#;
        let err = toml::from_str::<VersionOnlyDeserialize>(input).unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("expected a table") && err_str.contains("or an array"),
            "expected error to mention both formats, got: {}",
            err_str
        );

        let input = r#"experimental = "setup-scripts""#;
        let err = toml::from_str::<VersionOnlyDeserialize>(input).unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("expected a table") && err_str.contains("or an array"),
            "expected error to mention both formats, got: {}",
            err_str
        );
    }
}
