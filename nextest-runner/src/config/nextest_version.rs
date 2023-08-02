// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Nextest version configuration.

use super::{NextestConfig, ToolConfigFile};
use crate::errors::{ConfigParseError, ConfigParseErrorKind};
use camino::Utf8Path;
use semver::Version;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

/// A "version-only" form of the nextest configuration.
///
/// This is used as a first pass to determine the required nextest version before parsing the rest
/// of the configuration. That avoids issues parsing incompatible configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VersionOnlyConfig {
    /// The nextest version configuration.
    nextest_version: NextestVersionConfig,
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
        let nextest_version =
            Self::read_from_sources(workspace_root, config_file, tool_config_files_rev)?;
        Ok(Self { nextest_version })
    }

    /// Returns the nextest version requirement.
    pub fn nextest_version(&self) -> &NextestVersionConfig {
        &self.nextest_version
    }

    fn read_from_sources<'a>(
        workspace_root: &Utf8Path,
        config_file: Option<&Utf8Path>,
        tool_config_files_rev: impl Iterator<Item = &'a ToolConfigFile>,
    ) -> Result<NextestVersionConfig, ConfigParseError> {
        let mut nextest_version = NextestVersionConfig::default();

        // Merge in tool configs.
        for ToolConfigFile { config_file, tool } in tool_config_files_rev {
            if let Some(v) = Self::read_and_deserialize(config_file, Some(tool))? {
                nextest_version.accumulate(v, Some(tool));
            }
        }

        // Finally, merge in the repo config.
        let config_file = match config_file {
            Some(file) => Cow::Borrowed(file),
            None => Cow::Owned(workspace_root.join(NextestConfig::CONFIG_PATH)),
        };
        if let Some(v) = Self::read_and_deserialize(&config_file, None)? {
            nextest_version.accumulate(v, None);
        }

        Ok(nextest_version)
    }

    fn read_and_deserialize(
        config_file: &Utf8Path,
        tool: Option<&str>,
    ) -> Result<Option<NextestVersionDeserialize>, ConfigParseError> {
        let toml_str = std::fs::read_to_string(config_file.as_str()).map_err(|error| {
            ConfigParseError::new(
                config_file.clone(),
                tool,
                ConfigParseErrorKind::VersionOnlyReadError(error),
            )
        })?;
        let toml_de = toml::de::Deserializer::new(&toml_str);
        let v: VersionOnlyDeserialize =
            serde_path_to_error::deserialize(toml_de).map_err(|error| {
                ConfigParseError::new(
                    config_file.clone(),
                    tool,
                    ConfigParseErrorKind::VersionOnlyDeserializeError(Box::new(error)),
                )
            })?;
        Ok(v.nextest_version)
    }
}

/// A version of configuration that only deserializes the nextest version.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct VersionOnlyDeserialize {
    #[serde(default)]
    nextest_version: Option<NextestVersionDeserialize>,
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
    pub(crate) fn accumulate(&mut self, v: NextestVersionDeserialize, v_tool: Option<&str>) {
        if let Some(v) = v.required {
            self.required.accumulate(v, v_tool);
        }
        if let Some(v) = v.recommended {
            self.recommended.accumulate(v, v_tool);
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
                        tool: tool.map(|s| s.to_owned()),
                    };
                } else {
                    return NextestVersionEval::Error {
                        required: required.clone(),
                        current: current_version.clone(),
                        tool: tool.map(|s| s.to_owned()),
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
                        tool: tool.map(|s| s.to_owned()),
                    }
                } else {
                    NextestVersionEval::Warn {
                        recommended: recommended.clone(),
                        current: current_version.clone(),
                        tool: tool.map(|s| s.to_owned()),
                    }
                }
            }
        }
    }
}

/// Specification for a nextest version. Part of [`NextestVersion`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum NextestVersionReq {
    /// A version was specified.
    Version {
        /// The version to warn before.
        version: Version,

        /// The tool which produced this version specification.
        tool: Option<String>,
    },

    /// No version was specified.
    #[default]
    None,
}

impl NextestVersionReq {
    fn accumulate(&mut self, v: Version, v_tool: Option<&str>) {
        match self {
            NextestVersionReq::Version { version, tool } => {
                // This is v >= version rather than v > version, so that if multiple tools specify
                // the same version, the last tool wins.
                if &v >= version {
                    *version = v;
                    *tool = v_tool.map(|s| s.to_owned());
                }
            }
            NextestVersionReq::None => {
                *self = NextestVersionReq::Version {
                    version: v,
                    tool: v_tool.map(|s| s.to_owned()),
                };
            }
        }
    }

    fn satisfies(&self, version: &Version) -> Result<(), (&Version, Option<&str>)> {
        match self {
            NextestVersionReq::Version {
                version: required,
                tool,
            } => {
                if version >= required {
                    Ok(())
                } else {
                    Err((required, tool.as_deref()))
                }
            }
            NextestVersionReq::None => Ok(()),
        }
    }
}

/// The result of checking whether a [`NextestVersion`] satisfies a requirement.
///
/// Returned by [`NextestVersion::eval`].
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
        tool: Option<String>,
    },

    /// A warning should be produced.
    Warn {
        /// The minimum version recommended.
        recommended: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<String>,
    },

    /// An error should be produced but the version is overridden.
    ErrorOverride {
        /// The minimum version recommended.
        required: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<String>,
    },

    /// A warning should be produced but the version is overridden.
    WarnOverride {
        /// The minimum version recommended.
        recommended: Version,
        /// The current version.
        current: Version,
        /// The tool which produced this version specification.
        tool: Option<String>,
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

                if let (Some(required), Some(recommended)) = (&required, &recommended) {
                    if required >= recommended {
                        return Err(serde::de::Error::custom(format!(
                            "required version ({}) must be less than recommended version ({})",
                            required, recommended
                        )));
                    }
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

/// The mode to use when checking the nextest version: whether to produce an error or a warning.
#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NextestVersionMode {
    /// Produce an error if the nextest version is not at least the specified version.
    Error,
    /// Only produce a warning if the nextest version is not at least the specified version.
    Warning,
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
        "required version (0.9.20) must be less than recommended version (0.9.10)" ; "error greater than warning"
    )]
    fn test_invalid_nextest_version(input: &str, error_message: &str) {
        let err = toml::from_str::<VersionOnlyDeserialize>(input).unwrap_err();
        assert!(
            err.to_string().contains(error_message),
            "error `{}` contains `{}`",
            err,
            error_message
        );
    }

    #[test]
    fn test_accumulate() {
        let mut nextest_version = NextestVersionConfig::default();
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: Some("0.9.20".parse().unwrap()),
                recommended: None,
            },
            Some("tool1"),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: Some("0.9.30".parse().unwrap()),
                recommended: Some("0.9.35".parse().unwrap()),
            },
            Some("tool2"),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                required: None,
                // This recommended version is ignored since it is less than the last recommended
                // version.
                recommended: Some("0.9.25".parse().unwrap()),
            },
            Some("tool3"),
        );
        nextest_version.accumulate(
            NextestVersionDeserialize {
                // This is accepted because it is the same as the last required version, and the
                // last tool wins.
                required: Some("0.9.30".parse().unwrap()),
                recommended: None,
            },
            Some("tool4"),
        );

        assert_eq!(
            nextest_version,
            NextestVersionConfig {
                required: NextestVersionReq::Version {
                    version: "0.9.30".parse().unwrap(),
                    tool: Some("tool4".to_owned()),
                },
                recommended: NextestVersionReq::Version {
                    version: "0.9.35".parse().unwrap(),
                    tool: Some("tool2".to_owned()),
                },
            }
        );
    }
}
