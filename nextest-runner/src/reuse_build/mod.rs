// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reuse builds performed earlier.
//!
//! Nextest allows users to reuse builds done on one machine. This module contains support for that.
//!
//! The main data structures here are [`ReuseBuildInfo`] and [`PathMapper`].

use crate::{
    errors::{
        ArchiveExtractError, ArchiveReadError, PathMapperConstructError, PathMapperConstructKind,
    },
    list::BinaryList,
};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use guppy::graph::PackageGraph;
use std::{fs, io, sync::Arc};

mod archive_reporter;
mod archiver;
mod unarchiver;

pub use archive_reporter::*;
pub use archiver::*;
pub use unarchiver::*;

/// The name of the file in which Cargo metadata is stored.
pub const CARGO_METADATA_FILE_NAME: &str = "target/nextest/cargo-metadata.json";

/// The name of the file in which binaries metadata is stored.
pub const BINARIES_METADATA_FILE_NAME: &str = "target/nextest/binaries-metadata.json";

/// Reuse build information.
#[derive(Debug, Default)]
pub struct ReuseBuildInfo {
    /// Cargo metadata and remapping for the target directory.
    pub cargo_metadata: Option<MetadataWithRemap<(String, PackageGraph)>>,

    /// Binaries metadata JSON and remapping for the target directory.
    pub binaries_metadata: Option<MetadataWithRemap<BinaryList>>,

    /// Optional temporary directory used for cleanup.
    _temp_dir: Option<Utf8TempDir>,
}

impl ReuseBuildInfo {
    /// Creates a new [`ReuseBuildInfo`] from the given cargo and binaries metadata information.
    pub fn new(
        cargo_metadata: Option<MetadataWithRemap<(String, PackageGraph)>>,
        binaries_metadata: Option<MetadataWithRemap<BinaryList>>,
    ) -> Self {
        Self {
            cargo_metadata,
            binaries_metadata,
            _temp_dir: None,
        }
    }

    /// Extracts an archive and constructs a [`ReuseBuildInfo`] from it.
    pub fn extract_archive<F>(
        archive_file: &Utf8Path,
        format: ArchiveFormat,
        dest: ExtractDestination,
        callback: F,
        workspace_remap: Option<&Utf8Path>,
    ) -> Result<Self, ArchiveExtractError>
    where
        F: for<'e> FnMut(ArchiveEvent<'e>) -> io::Result<()>,
    {
        let mut file = fs::File::open(archive_file)
            .map_err(|err| ArchiveExtractError::Read(ArchiveReadError::Io(err)))?;

        let mut unarchiver = Unarchiver::new(&mut file, format);
        let ExtractInfo {
            dest_dir,
            temp_dir,
            binary_list,
            cargo_metadata_json,
            graph,
        } = unarchiver.extract(dest, callback)?;

        let cargo_metadata = MetadataWithRemap {
            metadata: MetadataOrPath::metadata((cargo_metadata_json, graph)),
            remap: workspace_remap.map(|p| p.to_owned()),
        };
        let binaries_metadata = MetadataWithRemap {
            metadata: MetadataOrPath::metadata(binary_list),
            remap: Some(dest_dir.join("target")),
        };

        Ok(Self {
            cargo_metadata: Some(cargo_metadata),
            binaries_metadata: Some(binaries_metadata),
            _temp_dir: temp_dir,
        })
    }

    /// Returns the Cargo metadata.
    pub fn cargo_metadata(&self) -> Option<&MetadataOrPath<(String, PackageGraph)>> {
        self.cargo_metadata.as_ref().map(|m| &m.metadata)
    }

    /// Returns the binaries metadata.
    pub fn binaries_metadata(&self) -> Option<&MetadataOrPath<BinaryList>> {
        self.binaries_metadata.as_ref().map(|m| &m.metadata)
    }

    /// Returns true if any component of the build is being reused.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.cargo_metadata.is_some() || self.binaries_metadata.is_some()
    }

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

/// Metadata as either deserialized contents or a path, along with a possible directory remap.
#[derive(Clone, Debug)]
pub struct MetadataWithRemap<T> {
    /// Metadata as either a path to data or as data that's already been read.
    pub metadata: MetadataOrPath<T>,

    /// The remapped directory.
    pub remap: Option<Utf8PathBuf>,
}

/// Represents either a path to metadata or actual deserialized metadata.
///
/// Part of [`MetadataWithRemap`].
#[derive(Clone, Debug)]
pub enum MetadataOrPath<T> {
    /// Deserialized metadata.
    Metadata(Arc<T>),

    /// Path to metadata.
    Path(Utf8PathBuf),
}

impl<T> MetadataOrPath<T> {
    /// Creates a new [`MetadataOrPath`] with actual metadata.
    #[inline]
    pub fn metadata(metadata: T) -> Self {
        Self::Metadata(Arc::new(metadata))
    }
}

impl<T> From<Utf8PathBuf> for MetadataOrPath<T> {
    #[inline]
    fn from(path: Utf8PathBuf) -> Self {
        Self::Path(path)
    }
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

    /// Ensure that PathMapper turns relative paths into absolute ones.
    #[test]
    fn test_path_mapper_relative() {
        let current_dir: Utf8PathBuf = std::env::current_dir()
            .expect("current dir obtained")
            .try_into()
            .expect("current dir is valid UTF-8");

        let temp_workspace_root = Utf8TempDir::new().expect("new temp dir created");
        let workspace_root_path: Utf8PathBuf = temp_workspace_root
            .path()
            // On Mac, the temp dir is a symlink, so canonicalize it.
            .canonicalize()
            .expect("workspace root canonicalized correctly")
            .try_into()
            .expect("workspace root is valid UTF-8");
        let rel_workspace_root = pathdiff::diff_utf8_paths(&workspace_root_path, &current_dir)
            .expect("abs to abs diff is non-None");

        let temp_target_dir = Utf8TempDir::new().expect("new temp dir created");
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
