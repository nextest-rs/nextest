// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::TrackDefault;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{de::Unexpected, Deserialize};
use std::fmt;

/// Type for the archive-include key.
///
/// # Notes
///
/// This is `deny_unknown_fields` because if we take additional arguments in the future, they're
/// likely to change semantics in an incompatible way.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ArchiveInclude {
    path: Utf8PathBuf,
    relative_to: ArchiveRelativeTo,
    #[serde(default = "default_depth")]
    depth: TrackDefault<RecursionDepth>,
}

impl ArchiveInclude {
    /// The maximum depth of recursion.
    pub fn depth(&self) -> RecursionDepth {
        self.depth.value
    }

    /// Whether the depth was deserialized. If false, the default value was used.
    pub fn is_depth_deserialized(&self) -> bool {
        self.depth.is_deserialized
    }

    /// Join the path with the given target dir.
    pub fn join_path(&self, target_dir: &Utf8Path) -> Utf8PathBuf {
        match self.relative_to {
            ArchiveRelativeTo::Target => target_dir.join(&self.path),
        }
    }
}

fn default_depth() -> TrackDefault<RecursionDepth> {
    // We use a high-but-not-infinite depth.
    TrackDefault::with_default_value(RecursionDepth::Finite(8))
}

/// Defines the base of the path
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ArchiveRelativeTo {
    /// Path starts at the target directory
    Target,
    // TODO: add support for profile relative
    //TargetProfile,
}

/// Recursion depth.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RecursionDepth {
    /// A specific depth.
    Finite(usize),

    /// Infinite recursion.
    Infinite,
}

impl RecursionDepth {
    pub(crate) const ZERO: RecursionDepth = RecursionDepth::Finite(0);

    pub(crate) fn is_zero(self) -> bool {
        self == Self::ZERO
    }

    pub(crate) fn decrement(self) -> Self {
        match self {
            Self::ZERO => panic!("attempted to decrement zero"),
            Self::Finite(n) => Self::Finite(n - 1),
            Self::Infinite => Self::Infinite,
        }
    }

    pub(crate) fn unwrap_finite(self) -> usize {
        match self {
            Self::Finite(n) => n,
            Self::Infinite => panic!("expected finite recursion depth"),
        }
    }
}

impl fmt::Display for RecursionDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finite(n) => write!(f, "{}", n),
            Self::Infinite => write!(f, "infinite"),
        }
    }
}

impl<'de> Deserialize<'de> for RecursionDepth {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RecursionDepthVisitor;

        impl<'de> serde::de::Visitor<'de> for RecursionDepthVisitor {
            type Value = RecursionDepth;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a non-negative integer or \"infinite\"")
            }

            // TOML uses i64, not u64
            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value < 0 {
                    return Err(serde::de::Error::invalid_value(
                        Unexpected::Signed(value),
                        &self,
                    ));
                }
                Ok(RecursionDepth::Finite(value as usize))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "infinite" => Ok(RecursionDepth::Infinite),
                    _ => Err(serde::de::Error::invalid_value(
                        Unexpected::Str(value),
                        &self,
                    )),
                }
            }
        }

        deserializer.deserialize_any(RecursionDepthVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            test_helpers::{build_platforms, temp_workspace},
            NextestConfig,
        },
        errors::ConfigParseErrorKind,
    };
    use camino::Utf8Path;
    use camino_tempfile::tempdir;
    use config::ConfigError;
    use indoc::indoc;
    use test_case::test_case;

    #[test]
    fn parse_valid() {
        let config_contents = indoc! {r#"
            [profile.default]
            archive-include = [
                { path = "foo", relative-to = "target" },
                { path = "bar", relative-to = "target", depth = 1 },
            ]

            [profile.profile1]
            archive-include = [
                { path = "baz", relative-to = "target", depth = 0 },
            ]

            [profile.profile2]
            archive-include = []

            [profile.profile3]
        "#};

        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(workspace_dir.path(), config_contents);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &graph,
            None,
            [],
            &Default::default(),
        )
        .expect("config is valid");

        let default_config = &[
            ArchiveInclude {
                path: "foo".into(),
                relative_to: ArchiveRelativeTo::Target,
                depth: default_depth(),
            },
            ArchiveInclude {
                path: "bar".into(),
                relative_to: ArchiveRelativeTo::Target,
                depth: TrackDefault::with_deserialized_value(RecursionDepth::Finite(1)),
            },
        ];

        assert_eq!(
            config
                .profile("default")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            default_config,
            "default matches"
        );

        assert_eq!(
            config
                .profile("profile1")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &[ArchiveInclude {
                path: "baz".into(),
                relative_to: ArchiveRelativeTo::Target,
                depth: TrackDefault::with_deserialized_value(RecursionDepth::ZERO),
            }],
            "profile1 matches"
        );

        assert_eq!(
            config
                .profile("profile2")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &[],
            "profile2 matches"
        );

        assert_eq!(
            config
                .profile("profile3")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            default_config,
            "profile3 matches"
        );
    }

    #[test_case(
        indoc!{r#"
            [profile.default]
            archive-include = { path = "foo", relative-to = "target" }
        "#},
        r#"invalid type: map, expected a sequence"#
        ; "missing list")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            archive-include = [
                { path = "foo" }
            ]
        "#},
        r#"missing field `relative-to`"#
        ; "missing relative-to")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            archive-include = [
                { path = "bar", relative-to = "unknown" }
            ]
        "#},
        r#"enum ArchiveRelativeTo does not have variant constructor unknown"#
        ; "invalid relative-to")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            archive-include = [
                { path = "bar", relative-to = "target", depth = -1 }
            ]
        "#},
        r#"invalid value: integer `-1`, expected a non-negative integer or "infinite""#
        ; "negative depth")]
    fn parse_invalid(config_contents: &str, expected_message: &str) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path();

        let graph = temp_workspace(workspace_path, config_contents);

        let config_err = NextestConfig::from_sources(
            graph.workspace().root(),
            &graph,
            None,
            [],
            &Default::default(),
        )
        .expect_err("config expected to be invalid");

        let message = match config_err.kind() {
            ConfigParseErrorKind::DeserializeError(path_error) => match path_error.inner() {
                ConfigError::Message(message) => message,
                other => {
                    panic!("for config error {config_err:?}, expected ConfigError::Message for inner error {other:?}");
                }
            },
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::DeserializeError"
                );
            }
        };

        assert!(
            message.contains(expected_message),
            "expected message: {expected_message}\nactual message: {message}"
        );
    }
}
