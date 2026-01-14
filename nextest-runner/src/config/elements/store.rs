// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration for the nextest store directory.

use crate::config::utils::deserialize_relative_path;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use std::fmt;

/// Store configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::config) struct StoreConfigImpl {
    dir: StoreDir,
}

impl StoreConfigImpl {
    /// Resolves the store directory based on the workspace root and target
    /// directory.
    pub(in crate::config) fn resolve_store_dir(
        &self,
        workspace_root: &Utf8Path,
        target_dir: &Utf8Path,
    ) -> Utf8PathBuf {
        match &self.dir {
            StoreDir::Path(path) => workspace_root.join(path),
            StoreDir::RelativeTo { dir, relative_to } => match relative_to {
                StoreRelativeTo::WorkspaceRoot => workspace_root.join(dir),
                StoreRelativeTo::TargetDir => target_dir.join(dir),
            },
        }
    }
}

/// The store directory configuration.
///
/// This can either be a simple path (relative to the workspace root), or a map
/// specifying what the path is relative to.
#[derive(Clone, Debug)]
enum StoreDir {
    /// A path relative to the workspace root.
    Path(Utf8PathBuf),
    /// A path relative to a specified location.
    RelativeTo {
        dir: Utf8PathBuf,
        relative_to: StoreRelativeTo,
    },
}

impl<'de> Deserialize<'de> for StoreDir {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;

        impl<'de2> Visitor<'de2> for V {
            type Value = StoreDir;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "a path relative to the workspace root, \
                     or a map: { dir = \"nextest\", relative-to = \"target-dir\" }",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // Don't validate the path here for backwards compatibility. The
                // previous string form allowed arbitrary paths here, including
                // absolute ones. The new map form validates the path via
                // deserialize_relative_path.
                //
                // The string form accepting arbitrary paths was probably a
                // mistake, which we should deprecate and eventually remove as a
                // behavior change.
                Ok(StoreDir::Path(v.into()))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de2>,
            {
                let de = de::value::MapAccessDeserializer::new(map);
                let map = StoreDirMap::deserialize(de)?;
                Ok(StoreDir::RelativeTo {
                    dir: map.dir,
                    relative_to: map.relative_to,
                })
            }
        }

        deserializer.deserialize_any(V)
    }
}

/// A deserializer for `{ dir = "nextest", relative-to = "target-dir" }`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct StoreDirMap {
    #[serde(deserialize_with = "deserialize_relative_path")]
    dir: Utf8PathBuf,
    relative_to: StoreRelativeTo,
}

/// What the store directory is relative to.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoreRelativeTo {
    /// Relative to the workspace root.
    WorkspaceRoot,
    /// Relative to the target directory.
    TargetDir,
}

#[cfg(test)]
mod tests {
    use crate::config::{
        core::NextestConfig,
        utils::test_helpers::{build_platforms, temp_workspace},
    };

    use super::*;
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use test_case::test_case;

    #[test_case(
        "",
        Utf8PathBuf::from("target"),
        Ok(Utf8PathBuf::from("nextest/default")),
        false
        ; "no config"
    )]
    #[test_case(
        indoc! {r#" 
            [store]
            dir = { path = "nexte", relative-to = "tig" }
        "#},
        Utf8PathBuf::from("target"),
        Err("does not have variant constructor tig"),
        true
        ; "invalid relative-to"
    )]
    #[test_case(
        indoc! {r#"
            [store]
            dir = "my-store"
        "#} ,
        Utf8PathBuf::from("target"),
        Ok(Utf8PathBuf::from("my-store/default")),
        false
        ; "valid dir"
    )]
    #[test_case(
        indoc! {r#"
            [store]
            dir = { dir = "my-store", relative-to = "target-dir" }
        "#},
        Utf8PathBuf::from("target"),
        Ok(Utf8PathBuf::from("my-store/default")),
        true
        ; "valid target dir"
    )]
    #[test_case(
        indoc! {r#"
            [store]
            dir = { dir = "store", relative-to = "workspace-root" }
        "#},
        Utf8PathBuf::from(""),
        Ok(Utf8PathBuf::from("store/default")),
        true
        ; "valid workspace root"
    )]
    fn store_config_deserialization(
        config_contents: &str,
        target_dir: Utf8PathBuf,
        expected: Result<Utf8PathBuf, &str>,
        relative_to: bool,
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let nextest_config_result = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        );

        match expected {
            Ok(expected_default) => {
                let nextest_config = nextest_config_result.expect("config file should parse");

                let store_dir = nextest_config
                    .profile("default")
                    .expect("default profile should exist")
                    .into_evaluatable(&build_platforms())
                    .store_dir(&target_dir);

                if relative_to {
                    assert!(store_dir.ends_with(target_dir.join(expected_default)));
                } else {
                    assert!(store_dir.ends_with(expected_default));
                }
            }

            Err(expected_err_str) => {
                let err_str = format!("{:?}", nextest_config_result.unwrap_err());

                assert!(
                    err_str.contains(expected_err_str),
                    "expected error string not found: {err_str}",
                )
            }
        }
    }

    #[test_case(
        r#"dir = "target/nextest""#,
        "/workspace",
        "/workspace/target",
        "/workspace/target/nextest"
        ; "simple path"
    )]
    #[test_case(
        r#"dir = { dir = "nextest", relative-to = "workspace-root" }"#,
        "/workspace",
        "/workspace/target",
        "/workspace/nextest"
        ; "explicit workspace root"
    )]
    #[test_case(
        r#"dir = { dir = "nextest", relative-to = "target-dir" }"#,
        "/workspace",
        "/workspace/target",
        "/workspace/target/nextest"
        ; "relative to target dir"
    )]
    #[test_case(
        r#"dir = { dir = "nextest", relative-to = "target-dir" }"#,
        "/workspace",
        "/tmp/archive-target",
        "/tmp/archive-target/nextest"
        ; "relative to remapped target dir"
    )]
    #[test_case(
        r#"dir = { dir = "emoji🚀test", relative-to = "workspace-root" }"#,
        "/workspace",
        "/workspace/target",
        "/workspace/emoji🚀test"
        ; "emoji unicode path"
    )]
    #[test_case(
        r#"dir = { dir = "café/naïve", relative-to = "target-dir" }"#,
        "/workspace",
        "/tmp/target",
        "/tmp/target/café/naïve"
        ; "accented unicode path"
    )]
    // Edge case boundary conditions
    #[test_case(
        r#"dir = { dir = "", relative-to = "workspace-root" }"#,
        "/workspace",
        "/workspace/target",
        "/workspace"
        ; "empty path component"
    )]
    #[test_case(
        r#"dir = { dir = ".", relative-to = "target-dir" }"#,
        "/workspace",
        "/tmp/target",
        "/tmp/target"
        ; "current directory reference"
    )]
    fn test_store_dir_resolution(
        toml: &str,
        workspace_root: &str,
        target_dir: &str,
        expected: &str,
    ) {
        let config: StoreConfigImpl = toml::from_str(toml).expect("valid TOML");
        let resolved =
            config.resolve_store_dir(Utf8Path::new(workspace_root), Utf8Path::new(target_dir));
        assert_eq!(resolved, Utf8Path::new(expected));
    }

    #[test]
    fn test_store_dir_escape_target_dir() {
        let result = toml::from_str::<StoreConfigImpl>(
            r#"dir = { dir = "../escape-test", relative-to = "target-dir" }"#,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected a relative path with no parent components")
        );
    }

    #[test]
    fn test_store_dir_parent_dir_in_path() {
        let result = toml::from_str::<StoreConfigImpl>(
            r#"dir = { dir = "sub/../sneaky", relative-to = "workspace-root" }"#,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected a relative path with no parent components")
        );
    }
}
