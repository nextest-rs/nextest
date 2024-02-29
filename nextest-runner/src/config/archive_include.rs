// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use serde::Deserialize;

/// Type for the archive-include key.
///
/// # Notes
///
/// This is `deny_unknown_fields` because if we take additional arguments in the future, they're
/// likely to change semantics in an incompatible way.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ArchiveInclude {
    pub(crate) path: Utf8PathBuf,
    pub(crate) relative_to: ArchiveRelativeTo,
}

/// Defines the base of the path
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveRelativeTo {
    /// Path starts at the target directory
    Target,
    // TODO: add support for profile relative
    //TargetProfile,
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
                { path = "bar", relative-to = "target" },
            ]

            [profile.profile1]
            archive-include = [
                { path = "baz", relative-to = "target" },
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
        assert_eq!(
            config
                .profile("default")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &vec![
                ArchiveInclude {
                    path: "foo".into(),
                    relative_to: ArchiveRelativeTo::Target
                },
                ArchiveInclude {
                    path: "bar".into(),
                    relative_to: ArchiveRelativeTo::Target
                }
            ],
            "default matches"
        );

        assert_eq!(
            config
                .profile("profile1")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &vec![ArchiveInclude {
                path: "baz".into(),
                relative_to: ArchiveRelativeTo::Target
            }],
            "profile1 matches"
        );

        assert_eq!(
            config
                .profile("profile2")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &vec![],
            "profile2 matches"
        );

        assert_eq!(
            config
                .profile("profile3")
                .expect("default profile exists")
                .apply_build_platforms(&build_platforms())
                .archive_include(),
            &vec![
                ArchiveInclude {
                    path: "foo".into(),
                    relative_to: ArchiveRelativeTo::Target
                },
                ArchiveInclude {
                    path: "bar".into(),
                    relative_to: ArchiveRelativeTo::Target
                }
            ],
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
