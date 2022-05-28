// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reuse builds performed earlier.
//!
//! Nextest allows users to reuse builds done on one machine. This module contains support for that.
//!
//! The main data structures here are [`ReuseBuildInfo`] and [`PathMapper`].

use crate::errors::{PathMapperConstructError, PathMapperConstructKind};
use camino::{Utf8Path, Utf8PathBuf};

/// Reuse build information.
#[derive(Clone, Debug, Default)]
pub struct ReuseBuildInfo {
    /// Cargo metadata and remapping for the target directory.
    pub cargo_metadata: Option<MetadataJsonWithRemap>,

    /// Binaries metadata JSON and remapping for the target directory.
    pub binaries_metadata: Option<MetadataJsonWithRemap>,
}

impl ReuseBuildInfo {
    /// Returns the new workspace directory.
    pub fn workspace_remap(&self) -> Option<&Utf8Path> {
        self.cargo_metadata
            .as_ref()
            .and_then(|m| m.remap.as_deref())
    }

    /// Returns the new target directory.
    pub fn target_dir_remap(&self) -> Option<&Utf8Path> {
        self.binaries_metadata
            .as_ref()
            .and_then(|m| m.remap.as_deref())
    }
}

/// A metadata JSON file path, along with a possible directory remap.
#[derive(Clone, Debug)]
pub struct MetadataJsonWithRemap {
    /// Path to the metadata JSON file.
    pub path: Utf8PathBuf,

    /// The remapped directory.
    pub remap: Option<Utf8PathBuf>,
}

/// A helper for path remapping.
///
/// This is useful when running tests in a different directory, or a different computer, from building them.
#[derive(Clone, Debug, Default)]
pub struct PathMapper {
    workspace: Option<(Utf8PathBuf, Utf8PathBuf)>,
    target_dir: Option<(Utf8PathBuf, Utf8PathBuf)>,
}

impl PathMapper {
    /// Constructs the path mapper.
    pub fn new(
        orig_workspace_root: impl Into<Utf8PathBuf>,
        workspace_remap: Option<&Utf8Path>,
        orig_target_dir: impl Into<Utf8PathBuf>,
        target_dir_remap: Option<&Utf8Path>,
    ) -> Result<Self, PathMapperConstructError> {
        let workspace_root = workspace_remap
            .map(|root| Self::canonicalize_dir(root, PathMapperConstructKind::WorkspaceRoot))
            .transpose()?;
        let target_dir = target_dir_remap
            .map(|dir| Self::canonicalize_dir(dir, PathMapperConstructKind::WorkspaceRoot))
            .transpose()?;

        Ok(Self {
            workspace: workspace_root.map(|w| (orig_workspace_root.into(), w)),
            target_dir: target_dir.map(|d| (orig_target_dir.into(), d)),
        })
    }

    /// Constructs a no-op path mapper.
    pub fn noop() -> Self {
        Self {
            workspace: None,
            target_dir: None,
        }
    }

    fn canonicalize_dir(
        input: &Utf8Path,
        kind: PathMapperConstructKind,
    ) -> Result<Utf8PathBuf, PathMapperConstructError> {
        let canonicalized_path =
            input
                .canonicalize()
                .map_err(|err| PathMapperConstructError::Canonicalization {
                    kind,
                    input: input.into(),
                    err,
                })?;
        let canonicalized_path: Utf8PathBuf =
            canonicalized_path
                .try_into()
                .map_err(|err| PathMapperConstructError::NonUtf8Path {
                    kind,
                    input: input.into(),
                    err,
                })?;
        if !canonicalized_path.is_dir() {
            return Err(PathMapperConstructError::NotADirectory {
                kind,
                input: input.into(),
                canonicalized_path,
            });
        }

        Ok(canonicalized_path)
    }

    pub(super) fn new_target_dir(&self) -> Option<&Utf8Path> {
        self.target_dir.as_ref().map(|(_, new)| new.as_path())
    }

    pub(crate) fn map_cwd(&self, path: Utf8PathBuf) -> Utf8PathBuf {
        match &self.workspace {
            Some((from, to)) => match path.strip_prefix(from) {
                Ok(p) => to.join(p),
                Err(_) => path,
            },
            None => path,
        }
    }

    pub(crate) fn map_binary(&self, path: Utf8PathBuf) -> Utf8PathBuf {
        match &self.target_dir {
            Some((from, to)) => match path.strip_prefix(from) {
                Ok(p) => to.join(p),
                Err(_) => path,
            },
            None => path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Ensure that PathMapper turns relative paths into absolute ones.
    #[test]
    fn test_path_mapper_relative() {
        let current_dir: Utf8PathBuf = std::env::current_dir()
            .expect("current dir obtained")
            .try_into()
            .expect("current dir is valid UTF-8");

        let temp_workspace_root = TempDir::new().expect("new temp dir created");
        let workspace_root_path: Utf8PathBuf = temp_workspace_root
            .path()
            // On Mac, the temp dir is a symlink, so canonicalize it.
            .canonicalize()
            .expect("workspace root canonicalized correctly")
            .try_into()
            .expect("workspace root is valid UTF-8");
        let rel_workspace_root = pathdiff::diff_utf8_paths(&workspace_root_path, &current_dir)
            .expect("abs to abs diff is non-None");

        let temp_target_dir = TempDir::new().expect("new temp dir created");
        let target_dir_path: Utf8PathBuf = temp_target_dir
            .path()
            .canonicalize()
            .expect("target dir canonicalized correctly")
            .try_into()
            .expect("target dir is valid UTF-8");
        let rel_target_dir = pathdiff::diff_utf8_paths(&target_dir_path, &current_dir)
            .expect("abs to abs diff is non-None");

        // These aren't really used other than to do mapping against.
        let orig_workspace_root = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
        let orig_target_dir = orig_workspace_root.join("target");

        let path_mapper = PathMapper::new(
            orig_workspace_root,
            Some(&rel_workspace_root),
            &orig_target_dir,
            Some(&rel_target_dir),
        )
        .expect("remapped paths exist");

        assert_eq!(
            path_mapper.map_cwd(orig_workspace_root.join("foobar")),
            workspace_root_path.join("foobar")
        );
        assert_eq!(
            path_mapper.map_binary(orig_target_dir.join("foobar")),
            target_dir_path.join("foobar")
        );
    }
}
