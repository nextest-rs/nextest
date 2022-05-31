// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ArchiveEvent, BINARIES_METADATA_FILE_NAME, CARGO_METADATA_FILE_NAME};
use crate::{
    errors::{ArchiveExtractError, ArchiveReadError},
    list::BinaryList,
};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use guppy::graph::PackageGraph;
use itertools::Either;
use nextest_metadata::BinaryListSummary;
use std::{
    fs,
    io::{self, Read, Seek},
};
use tempfile::TempDir;

#[derive(Clone, Debug)]
pub(crate) struct ArchiveInfo {
    pub binary_list: BinaryList,
    pub file_count: usize,
}

#[derive(Debug)]
pub(crate) struct Unarchiver<'a> {
    file: &'a mut fs::File,
}

impl<'a> Unarchiver<'a> {
    pub(crate) fn new(file: &'a mut fs::File) -> Self {
        Self { file }
    }

    pub(crate) fn get_info(&mut self) -> Result<ArchiveInfo, ArchiveReadError> {
        self.file
            .seek(io::SeekFrom::Start(0))
            .map_err(ArchiveReadError::Io)?;
        let mut archive_reader = ArchiveReader::new(self.file)?;

        let mut file_count = 0;
        let mut binary_list = None;
        let mut found_cargo_metadata = false;

        let binaries_metadata_path = Utf8Path::new(BINARIES_METADATA_FILE_NAME);
        let cargo_metadata_path = Utf8Path::new(CARGO_METADATA_FILE_NAME);

        for entry in archive_reader.entries()? {
            let (entry, path) = entry?;
            file_count += 1;

            if path == binaries_metadata_path {
                // Try reading the binary list out of this entry.
                let summary: BinaryListSummary =
                    serde_json::from_reader(entry).map_err(|error| {
                        ArchiveReadError::MetadataDeserializeError {
                            path: binaries_metadata_path,
                            error,
                        }
                    })?;
                binary_list = Some(BinaryList::from_summary(summary));
            } else if path == cargo_metadata_path {
                found_cargo_metadata = true;
            }
        }

        let binary_list = match binary_list {
            Some(binary_list) => binary_list,
            None => {
                return Err(ArchiveReadError::MetadataFileNotFound(
                    binaries_metadata_path,
                ))
            }
        };
        if !found_cargo_metadata {
            return Err(ArchiveReadError::MetadataFileNotFound(cargo_metadata_path));
        }

        Ok(ArchiveInfo {
            file_count,
            binary_list,
        })
    }

    pub(crate) fn extract<F>(
        &mut self,
        dest: ExtractDestination,
        mut callback: F,
    ) -> Result<ExtractInfo, ArchiveExtractError>
    where
        F: for<'e> FnMut(ArchiveEvent<'e>) -> io::Result<()>,
    {
        let (dest_dir, temp_dir) = match dest {
            ExtractDestination::TempDir { persist } => {
                // Create a new temporary directory and extract contents to it.
                let temp_dir = tempfile::Builder::new()
                    .prefix("nextest-archive-")
                    .tempdir()
                    .map_err(ArchiveExtractError::TempDirCreate)?;
                let dest_dir: Utf8PathBuf =
                    temp_dir.path().to_path_buf().try_into().map_err(|err| {
                        ArchiveExtractError::TempDirCreate(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            err,
                        ))
                    })?;

                let dest_dir = dest_dir.canonicalize_utf8().map_err(|error| {
                    ArchiveExtractError::DestDirCanonicalization {
                        dir: dest_dir.to_owned(),
                        error,
                    }
                })?;

                let temp_dir = if persist {
                    // Persist the temporary directory.
                    let _ = temp_dir.into_path();
                    None
                } else {
                    Some(temp_dir)
                };

                (dest_dir, temp_dir)
            }
            ExtractDestination::Destination { dir, overwrite } => {
                // Extract contents to the destination directory.
                let dest_dir = dir
                    .canonicalize_utf8()
                    .map_err(|error| ArchiveExtractError::DestDirCanonicalization { dir, error })?;

                let dest_target = dest_dir.join("target");
                if dest_target.exists() && !overwrite {
                    return Err(ArchiveExtractError::DestinationExists(dest_target));
                }

                (dest_dir, None)
            }
        };

        // Read archive and validate it.
        let archive_info = self.get_info().map_err(ArchiveExtractError::Read)?;
        let file_count = archive_info.file_count;
        let binary_list = archive_info.binary_list;
        let test_binary_count = binary_list.rust_binaries.len();
        let non_test_binary_count = binary_list.rust_build_meta.non_test_binaries.len();
        let linked_path_count = binary_list.rust_build_meta.linked_paths.len();

        // Report begin extraction.
        callback(ArchiveEvent::ExtractStarted {
            file_count,
            test_binary_count,
            non_test_binary_count,
            linked_path_count,
            dest_dir: &dest_dir,
        })
        .map_err(ArchiveExtractError::ReporterIo)?;

        // Extract the archive.
        self.file
            .seek(io::SeekFrom::Start(0))
            .map_err(|error| ArchiveExtractError::Read(ArchiveReadError::Io(error)))?;
        let mut archive_reader =
            ArchiveReader::new(self.file).map_err(ArchiveExtractError::Read)?;

        // Will be filled out by the for loop below
        let mut cargo_metadata = None;
        let cargo_metadata_path = Utf8Path::new(CARGO_METADATA_FILE_NAME);

        for entry in archive_reader
            .entries()
            .map_err(ArchiveExtractError::Read)?
        {
            let (mut entry, path) = entry.map_err(ArchiveExtractError::Read)?;
            if path == Utf8Path::new(BINARIES_METADATA_FILE_NAME) {
                // The BinaryList was already read in the ArchiveInfo above -- no need to re-read or
                // extract it.
                continue;
            } else if path == Utf8Path::new(CARGO_METADATA_FILE_NAME) {
                // Parse the input Cargo metadata as a `PackageGraph`.
                let mut json = String::with_capacity(entry.size() as usize);
                entry
                    .read_to_string(&mut json)
                    .map_err(|error| ArchiveExtractError::Read(ArchiveReadError::Io(error)))?;

                let package_graph = PackageGraph::from_json(&json).map_err(|error| {
                    ArchiveExtractError::Read(ArchiveReadError::PackageGraphConstructError {
                        path: cargo_metadata_path,
                        error,
                    })
                })?;
                cargo_metadata = Some((json, package_graph));
                continue;
            }

            // Extract all other files.
            entry
                .unpack_in(&dest_dir)
                .map_err(|error| ArchiveExtractError::WriteFile { path, error })?;
        }

        let (cargo_metadata_json, graph) =
            cargo_metadata.expect("get_info already verified that Cargo metadata exists");

        // Report end extraction.
        callback(ArchiveEvent::Extracted {
            file_count,
            dest_dir: &dest_dir,
        })
        .map_err(ArchiveExtractError::ReporterIo)?;

        Ok(ExtractInfo {
            dest_dir,
            temp_dir,
            binary_list,
            cargo_metadata_json,
            graph,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ExtractInfo {
    /// The destination directory.
    pub dest_dir: Utf8PathBuf,

    /// An optional [`TempDir`], used for cleanup.
    pub temp_dir: Option<TempDir>,

    /// The [`BinaryList`] read from the archive.
    pub binary_list: BinaryList,

    /// The Cargo metadata JSON.
    pub cargo_metadata_json: String,

    /// The [`PackageGraph`] read from the archive.
    pub graph: PackageGraph,
}

struct ArchiveReader<'a> {
    archive: tar::Archive<zstd::Decoder<'static, io::BufReader<&'a mut fs::File>>>,
}

impl<'a> ArchiveReader<'a> {
    fn new(file: &'a mut fs::File) -> Result<Self, ArchiveReadError> {
        let decoder = zstd::Decoder::new(file).map_err(ArchiveReadError::Io)?;
        let archive = tar::Archive::new(decoder);
        Ok(Self { archive })
    }

    fn entries<'r>(
        &'r mut self,
    ) -> Result<
        impl Iterator<Item = Result<(ArchiveEntry<'r, 'a>, Utf8PathBuf), ArchiveReadError>>,
        ArchiveReadError,
    > {
        let entries = self.archive.entries().map_err(ArchiveReadError::Io)?;
        Ok(entries.map(|entry| {
            let entry = entry.map_err(ArchiveReadError::Io)?;

            // Validation: entry paths must be valid UTF-8.
            let path = entry_path(&entry)?;

            // Validation: paths start with "target".
            if !path.starts_with("target") {
                return Err(ArchiveReadError::NoTargetPrefix(path));
            }

            // Validation: paths only contain normal components.
            for component in path.components() {
                match component {
                    Utf8Component::Normal(_) => {}
                    other => {
                        return Err(ArchiveReadError::InvalidComponent {
                            path: path.clone(),
                            component: other.as_str().to_owned(),
                        });
                    }
                }
            }

            // Validation: checksum matches.
            let mut header = entry.header().clone();
            let actual_cksum = header
                .cksum()
                .map_err(|err| ArchiveReadError::InvalidChecksum {
                    path: path.clone(),
                    payload: Either::Right(err),
                })?;

            header.set_cksum();
            let expected_cksum = header
                .cksum()
                .expect("checksum that was just set can't be invalid");

            if expected_cksum != actual_cksum {
                return Err(ArchiveReadError::InvalidChecksum {
                    path,
                    payload: Either::Left((expected_cksum, actual_cksum)),
                });
            }

            Ok((entry, path))
        }))
    }
}

/// Given an entry, returns its path as a `Utf8Path`.
fn entry_path(entry: &ArchiveEntry<'_, '_>) -> Result<Utf8PathBuf, ArchiveReadError> {
    let path_bytes = entry.path_bytes();
    let path_str = std::str::from_utf8(&path_bytes)
        .map_err(|_| ArchiveReadError::NonUtf8Path(path_bytes.to_vec()))?;
    let utf8_path = Utf8Path::new(path_str);
    Ok(utf8_path.to_owned())
}

/// Where to extract a nextest archive to.
#[derive(Clone, Debug)]
pub enum ExtractDestination {
    /// Extract the archive to a new temporary directory.
    TempDir {
        /// Whether to persist the temporary directory at the end of execution.
        persist: bool,
    },
    /// Extract the archive to a custom destination.
    Destination {
        /// The directory to extract to.
        dir: Utf8PathBuf,
        /// Whether to overwrite existing contents.
        overwrite: bool,
    },
}

type ArchiveEntry<'r, 'a> = tar::Entry<'r, zstd::Decoder<'static, io::BufReader<&'a mut fs::File>>>;
