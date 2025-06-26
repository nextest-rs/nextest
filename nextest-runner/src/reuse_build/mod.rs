// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reuse builds performed earlier.
//!
//! Nextest allows users to reuse builds done on one machine. This module contains support for that.
//!
//! The main data structures here are [`ReuseBuildInfo`] and [`PathMapper`].

use crate::{
    errors::{
        ArchiveExtractError, ArchiveReadError, MetadataMaterializeError, PathMapperConstructError,
        PathMapperConstructKind,
    },
    list::BinaryList,
    platform::PlatformLibdir,
};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use guppy::graph::PackageGraph;
use nextest_metadata::{BinaryListSummary, PlatformLibdirUnavailable};
use std::{fmt, fs, io, sync::Arc};

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

/// The name of the directory in which libdirs are stored.
pub const LIBDIRS_BASE_DIR: &str = "target/nextest/libdirs";

/// Reuse build information.
#[derive(Debug, Default)]
pub struct ReuseBuildInfo {
    /// Cargo metadata and remapping for the target directory.
    pub cargo_metadata: Option<MetadataWithRemap<ReusedCargoMetadata>>,

    /// Binaries metadata JSON and remapping for the target directory.
    pub binaries_metadata: Option<MetadataWithRemap<ReusedBinaryList>>,

    /// A remapper for libdirs.
    pub libdir_mapper: LibdirMapper,

    /// Optional temporary directory used for cleanup.
    _temp_dir: Option<Utf8TempDir>,
}

impl ReuseBuildInfo {
    /// Creates a new [`ReuseBuildInfo`] from the given cargo and binaries metadata information.
    pub fn new(
        cargo_metadata: Option<MetadataWithRemap<ReusedCargoMetadata>>,
        binaries_metadata: Option<MetadataWithRemap<ReusedBinaryList>>,
        // TODO: accept libdir_mapper as an argument, as well
    ) -> Self {
        Self {
            cargo_metadata,
            binaries_metadata,
            libdir_mapper: LibdirMapper::default(),
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
            libdir_mapper,
        } = unarchiver.extract(dest, callback)?;

        let cargo_metadata = MetadataWithRemap {
            metadata: ReusedCargoMetadata::new((cargo_metadata_json, graph)),
            remap: workspace_remap.map(|p| p.to_owned()),
        };
        let binaries_metadata = MetadataWithRemap {
            metadata: ReusedBinaryList::new(binary_list),
            remap: Some(dest_dir.join("target")),
        };

        Ok(Self {
            cargo_metadata: Some(cargo_metadata),
            binaries_metadata: Some(binaries_metadata),
            libdir_mapper,
            _temp_dir: temp_dir,
        })
    }

    /// Returns the Cargo metadata.
    pub fn cargo_metadata(&self) -> Option<&ReusedCargoMetadata> {
        self.cargo_metadata.as_ref().map(|m| &m.metadata)
    }

    /// Returns the binaries metadata, reading it from disk if necessary.
    pub fn binaries_metadata(&self) -> Option<&ReusedBinaryList> {
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
    /// The metadata.
    pub metadata: T,

    /// The remapped directory.
    pub remap: Option<Utf8PathBuf>,
}

/// Type parameter for [`MetadataWithRemap`].
pub trait MetadataKind: Clone + fmt::Debug {
    /// The type of metadata stored.
    type MetadataType: Sized;

    /// Constructs a new [`MetadataKind`] from the given metadata.
    fn new(metadata: Self::MetadataType) -> Self;

    /// Reads a path, resolving it into this data type.
    fn materialize(path: &Utf8Path) -> Result<Self, MetadataMaterializeError>;
}

/// [`MetadataKind`] for a [`BinaryList`].
#[derive(Clone, Debug)]
pub struct ReusedBinaryList {
    /// The binary list.
    pub binary_list: Arc<BinaryList>,
}

impl MetadataKind for ReusedBinaryList {
    type MetadataType = BinaryList;

    fn new(binary_list: Self::MetadataType) -> Self {
        Self {
            binary_list: Arc::new(binary_list),
        }
    }

    fn materialize(path: &Utf8Path) -> Result<Self, MetadataMaterializeError> {
        // Three steps: read the contents, turn it into a summary, and then turn it into a
        // BinaryList.
        //
        // Buffering the contents in memory is generally much faster than trying to read it
        // using a BufReader.
        let contents =
            fs::read_to_string(path).map_err(|error| MetadataMaterializeError::Read {
                path: path.to_owned(),
                error,
            })?;

        let summary: BinaryListSummary = serde_json::from_str(&contents).map_err(|error| {
            MetadataMaterializeError::Deserialize {
                path: path.to_owned(),
                error,
            }
        })?;

        let binary_list = BinaryList::from_summary(summary).map_err(|error| {
            MetadataMaterializeError::RustBuildMeta {
                path: path.to_owned(),
                error,
            }
        })?;

        Ok(Self::new(binary_list))
    }
}

/// [`MetadataKind`] for Cargo metadata.
#[derive(Clone, Debug)]
pub struct ReusedCargoMetadata {
    /// Cargo metadata JSON.
    pub json: Arc<String>,

    /// The package graph.
    pub graph: Arc<PackageGraph>,
}

impl MetadataKind for ReusedCargoMetadata {
    type MetadataType = (String, PackageGraph);

    fn new((json, graph): Self::MetadataType) -> Self {
        Self {
            json: Arc::new(json),
            graph: Arc::new(graph),
        }
    }

    fn materialize(path: &Utf8Path) -> Result<Self, MetadataMaterializeError> {
        // Read the contents into memory, then parse them as a `PackageGraph`.
        let json =
            std::fs::read_to_string(path).map_err(|error| MetadataMaterializeError::Read {
                path: path.to_owned(),
                error,
            })?;
        let graph = PackageGraph::from_json(&json).map_err(|error| {
            MetadataMaterializeError::PackageGraphConstruct {
                path: path.to_owned(),
                error,
            }
        })?;

        Ok(Self::new((json, graph)))
    }
}

/// A helper for path remapping.
///
/// This is useful when running tests in a different directory, or a different computer, from
/// building them.
#[derive(Clone, Debug, Default)]
pub struct PathMapper {
    workspace: Option<(Utf8PathBuf, Utf8PathBuf)>,
    target_dir: Option<(Utf8PathBuf, Utf8PathBuf)>,
    libdir_mapper: LibdirMapper,
}

impl PathMapper {
    /// Constructs the path mapper.
    pub fn new(
        orig_workspace_root: impl Into<Utf8PathBuf>,
        workspace_remap: Option<&Utf8Path>,
        orig_target_dir: impl Into<Utf8PathBuf>,
        target_dir_remap: Option<&Utf8Path>,
        libdir_mapper: LibdirMapper,
    ) -> Result<Self, PathMapperConstructError> {
        let workspace_root = workspace_remap
            .map(|root| Self::canonicalize_dir(root, PathMapperConstructKind::WorkspaceRoot))
            .transpose()?;
        let target_dir = target_dir_remap
            .map(|dir| Self::canonicalize_dir(dir, PathMapperConstructKind::TargetDir))
            .transpose()?;

        Ok(Self {
            workspace: workspace_root.map(|w| (orig_workspace_root.into(), w)),
            target_dir: target_dir.map(|d| (orig_target_dir.into(), d)),
            libdir_mapper,
        })
    }

    /// Constructs a no-op path mapper.
    pub fn noop() -> Self {
        Self {
            workspace: None,
            target_dir: None,
            libdir_mapper: LibdirMapper::default(),
        }
    }

    /// Returns the libdir mapper.
    pub fn libdir_mapper(&self) -> &LibdirMapper {
        &self.libdir_mapper
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

        Ok(os_imp::strip_verbatim(canonicalized_path))
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

/// A mapper for lib dirs.
///
/// Archives store parts of lib dirs, which must be remapped to the new target directory.
#[derive(Clone, Debug, Default)]
pub struct LibdirMapper {
    /// The host libdir mapper.
    pub(crate) host: PlatformLibdirMapper,

    /// The target libdir mapper.
    pub(crate) target: PlatformLibdirMapper,
}

/// A mapper for an individual platform libdir.
///
/// Part of [`LibdirMapper`].
#[derive(Clone, Debug, Default)]
pub(crate) enum PlatformLibdirMapper {
    Path(Utf8PathBuf),
    Unavailable,
    #[default]
    NotRequested,
}

impl PlatformLibdirMapper {
    pub(crate) fn map(&self, original: &PlatformLibdir) -> PlatformLibdir {
        match self {
            PlatformLibdirMapper::Path(new) => {
                // Just use the new path. (We may have to check original in the future, but it
                // doesn't seem necessary for now -- if a libdir has been provided to the remapper,
                // that's that.)
                PlatformLibdir::Available(new.clone())
            }
            PlatformLibdirMapper::Unavailable => {
                // In this case, the original value is ignored -- we expected a libdir to be
                // present, but it wasn't.
                PlatformLibdir::Unavailable(PlatformLibdirUnavailable::NOT_IN_ARCHIVE)
            }
            PlatformLibdirMapper::NotRequested => original.clone(),
        }
    }
}

#[cfg(windows)]
mod os_imp {
    use camino::Utf8PathBuf;
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

    /// Strips verbatim prefix from a path if possible.
    pub(super) fn strip_verbatim(path: Utf8PathBuf) -> Utf8PathBuf {
        let path_str = String::from(path);
        if path_str.starts_with(r"\\?\UNC") {
            // In general we don't expect UNC paths, so just return the path as is.
            path_str.into()
        } else if path_str.starts_with(r"\\?\") {
            const START_LEN: usize = r"\\?\".len();

            let is_absolute_exact = {
                let mut v = path_str[START_LEN..].encode_utf16().collect::<Vec<u16>>();
                // Ensure null termination.
                v.push(0);
                is_absolute_exact(&v)
            };

            if is_absolute_exact {
                path_str[START_LEN..].into()
            } else {
                path_str.into()
            }
        } else {
            // Not a verbatim path, so return it as is.
            path_str.into()
        }
    }

    /// Test that the path is absolute, fully qualified and unchanged when processed by the Windows API.Add commentMore actions
    ///
    /// For example:
    ///
    /// - `C:\path\to\file` will return true.
    /// - `C:\path\to\nul` returns false because the Windows API will convert it to \\.\NUL
    /// - `C:\path\to\..\file` returns false because it will be resolved to `C:\path\file`.
    ///
    /// This is a useful property because it means the path can be converted from and to and verbatim
    /// path just by changing the prefix.
    fn is_absolute_exact(path: &[u16]) -> bool {
        // Adapted from the Rust project: https://github.com/rust-lang/rust/commit/edfc74722556c659de6fa03b23af3b9c8ceb8ac2

        // This is implemented by checking that passing the path through
        // GetFullPathNameW does not change the path in any way.

        // Windows paths are limited to i16::MAX length
        // though the API here accepts a u32 for the length.
        if path.is_empty() || path.len() > u32::MAX as usize || path.last() != Some(&0) {
            return false;
        }
        // The path returned by `GetFullPathNameW` must be the same length as the
        // given path, otherwise they're not equal.
        let buffer_len = path.len();
        let mut new_path = Vec::with_capacity(buffer_len);
        let result = unsafe {
            GetFullPathNameW(
                path.as_ptr(),
                new_path.capacity() as u32,
                new_path.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        // Note: if non-zero, the returned result is the length of the buffer without the null termination
        if result == 0 || result as usize != buffer_len - 1 {
            false
        } else {
            // SAFETY: `GetFullPathNameW` initialized `result` bytes and does not exceed `nBufferLength - 1` (capacity).
            unsafe {
                new_path.set_len((result as usize) + 1);
            }
            path == new_path
        }
    }
}

#[cfg(unix)]
mod os_imp {
    use camino::Utf8PathBuf;

    pub(super) fn strip_verbatim(path: Utf8PathBuf) -> Utf8PathBuf {
        // On Unix, there aren't any verbatin paths, so just return the path as
        // is.
        path
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
        let workspace_root_path: Utf8PathBuf = os_imp::strip_verbatim(
            temp_workspace_root
                .path()
                // On Mac, the temp dir is a symlink, so canonicalize it.
                .canonicalize()
                .expect("workspace root canonicalized correctly")
                .try_into()
                .expect("workspace root is valid UTF-8"),
        );
        let rel_workspace_root = pathdiff::diff_utf8_paths(&workspace_root_path, &current_dir)
            .expect("abs to abs diff is non-None");

        let temp_target_dir = Utf8TempDir::new().expect("new temp dir created");
        let target_dir_path: Utf8PathBuf = os_imp::strip_verbatim(
            temp_target_dir
                .path()
                .canonicalize()
                .expect("target dir canonicalized correctly")
                .try_into()
                .expect("target dir is valid UTF-8"),
        );
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
            LibdirMapper::default(),
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
