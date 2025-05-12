// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{
    ArchiveEvent, ArchiveFormat, BINARIES_METADATA_FILE_NAME, CARGO_METADATA_FILE_NAME,
    LIBDIRS_BASE_DIR, LibdirMapper, PlatformLibdirMapper,
};
use crate::{
    errors::{ArchiveExtractError, ArchiveReadError},
    helpers::convert_rel_path_to_main_sep,
    list::BinaryList,
};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use guppy::{CargoMetadata, graph::PackageGraph};
use nextest_metadata::BinaryListSummary;
use std::{
    fs,
    io::{self, Seek},
    time::Instant,
};

#[derive(Debug)]
pub(crate) struct Unarchiver<'a> {
    file: &'a mut fs::File,
    format: ArchiveFormat,
}

impl<'a> Unarchiver<'a> {
    pub(crate) fn new(file: &'a mut fs::File, format: ArchiveFormat) -> Self {
        Self { file, format }
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
                let temp_dir = camino_tempfile::Builder::new()
                    .prefix("nextest-archive-")
                    .tempdir()
                    .map_err(ArchiveExtractError::TempDirCreate)?;

                let dest_dir: Utf8PathBuf = temp_dir.path().to_path_buf();
                let dest_dir = temp_dir.path().canonicalize_utf8().map_err(|error| {
                    ArchiveExtractError::DestDirCanonicalization {
                        dir: dest_dir,
                        error,
                    }
                })?;

                let temp_dir = if persist {
                    // Persist the temporary directory.
                    let _ = temp_dir.keep();
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

        let start_time = Instant::now();

        // Extract the archive.
        self.file
            .rewind()
            .map_err(|error| ArchiveExtractError::Read(ArchiveReadError::Io(error)))?;
        let mut archive_reader =
            ArchiveReader::new(self.file, self.format).map_err(ArchiveExtractError::Read)?;

        // Will be filled out by the for loop below.
        let mut binary_list = None;
        let mut graph_data = None;
        let mut host_libdir = PlatformLibdirMapper::Unavailable;
        let mut target_libdir = PlatformLibdirMapper::Unavailable;
        let binaries_metadata_path = Utf8Path::new(BINARIES_METADATA_FILE_NAME);
        let cargo_metadata_path = Utf8Path::new(CARGO_METADATA_FILE_NAME);

        let mut file_count = 0;

        for entry in archive_reader
            .entries()
            .map_err(ArchiveExtractError::Read)?
        {
            file_count += 1;
            let (mut entry, path) = entry.map_err(ArchiveExtractError::Read)?;

            entry
                .unpack_in(&dest_dir)
                .map_err(|error| ArchiveExtractError::WriteFile {
                    path: path.clone(),
                    error,
                })?;

            // For archives created by nextest, binaries_metadata_path should be towards the beginning
            // so this should report the ExtractStarted event instantly.
            if path == binaries_metadata_path {
                // Try reading the binary list from the file on disk.
                let mut file = fs::File::open(dest_dir.join(binaries_metadata_path))
                    .map_err(|error| ArchiveExtractError::WriteFile { path, error })?;

                let summary: BinaryListSummary =
                    serde_json::from_reader(&mut file).map_err(|error| {
                        ArchiveExtractError::Read(ArchiveReadError::MetadataDeserializeError {
                            path: binaries_metadata_path,
                            error,
                        })
                    })?;

                let this_binary_list = BinaryList::from_summary(summary)?;
                let test_binary_count = this_binary_list.rust_binaries.len();
                let non_test_binary_count =
                    this_binary_list.rust_build_meta.non_test_binaries.len();
                let build_script_out_dir_count =
                    this_binary_list.rust_build_meta.build_script_out_dirs.len();
                let linked_path_count = this_binary_list.rust_build_meta.linked_paths.len();

                // TODO: also store a manifest of extra paths, and report them here.

                // Report begin extraction.
                callback(ArchiveEvent::ExtractStarted {
                    test_binary_count,
                    non_test_binary_count,
                    build_script_out_dir_count,
                    linked_path_count,
                    dest_dir: &dest_dir,
                })
                .map_err(ArchiveExtractError::ReporterIo)?;

                binary_list = Some(this_binary_list);
            } else if path == cargo_metadata_path {
                // Parse the input Cargo metadata as a `PackageGraph`.
                let json = fs::read_to_string(dest_dir.join(cargo_metadata_path))
                    .map_err(|error| ArchiveExtractError::WriteFile { path, error })?;

                // Doing this in multiple steps results in better error messages.
                let cargo_metadata: CargoMetadata =
                    serde_json::from_str(&json).map_err(|error| {
                        ArchiveExtractError::Read(ArchiveReadError::MetadataDeserializeError {
                            path: binaries_metadata_path,
                            error,
                        })
                    })?;

                let package_graph = cargo_metadata.build_graph().map_err(|error| {
                    ArchiveExtractError::Read(ArchiveReadError::PackageGraphConstructError {
                        path: cargo_metadata_path,
                        error,
                    })
                })?;
                graph_data = Some((json, package_graph));
                continue;
            } else if let Ok(suffix) = path.strip_prefix(LIBDIRS_BASE_DIR) {
                if suffix.starts_with("host") {
                    host_libdir = PlatformLibdirMapper::Path(dest_dir.join(
                        convert_rel_path_to_main_sep(&Utf8Path::new(LIBDIRS_BASE_DIR).join("host")),
                    ));
                } else if suffix.starts_with("target/0") {
                    // Currently we only support one target, so just check explicitly for target/0.
                    target_libdir =
                        PlatformLibdirMapper::Path(dest_dir.join(convert_rel_path_to_main_sep(
                            &Utf8Path::new(LIBDIRS_BASE_DIR).join("target/0"),
                        )));
                }
            }
        }

        let binary_list = match binary_list {
            Some(binary_list) => binary_list,
            None => {
                return Err(ArchiveExtractError::Read(
                    ArchiveReadError::MetadataFileNotFound(binaries_metadata_path),
                ));
            }
        };

        let (cargo_metadata_json, graph) = match graph_data {
            Some(x) => x,
            None => {
                return Err(ArchiveExtractError::Read(
                    ArchiveReadError::MetadataFileNotFound(cargo_metadata_path),
                ));
            }
        };

        let elapsed = start_time.elapsed();
        // Report end extraction.
        callback(ArchiveEvent::Extracted {
            file_count,
            dest_dir: &dest_dir,
            elapsed,
        })
        .map_err(ArchiveExtractError::ReporterIo)?;

        Ok(ExtractInfo {
            dest_dir,
            temp_dir,
            binary_list,
            cargo_metadata_json,
            graph,
            libdir_mapper: LibdirMapper {
                host: host_libdir,
                target: target_libdir,
            },
        })
    }
}

#[derive(Debug)]
pub(crate) struct ExtractInfo {
    /// The destination directory.
    pub dest_dir: Utf8PathBuf,

    /// An optional [`Utf8TempDir`], used for cleanup.
    pub temp_dir: Option<Utf8TempDir>,

    /// The [`BinaryList`] read from the archive.
    pub binary_list: BinaryList,

    /// The Cargo metadata JSON.
    pub cargo_metadata_json: String,

    /// The [`PackageGraph`] read from the archive.
    pub graph: PackageGraph,

    /// A remapper for the Rust libdir.
    pub libdir_mapper: LibdirMapper,
}

struct ArchiveReader<'a> {
    archive: tar::Archive<zstd::Decoder<'static, io::BufReader<&'a mut fs::File>>>,
}

impl<'a> ArchiveReader<'a> {
    fn new(file: &'a mut fs::File, format: ArchiveFormat) -> Result<Self, ArchiveReadError> {
        let archive = match format {
            ArchiveFormat::TarZst => {
                let decoder = zstd::Decoder::new(file).map_err(ArchiveReadError::Io)?;
                tar::Archive::new(decoder)
            }
        };
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
                .map_err(|error| ArchiveReadError::ChecksumRead {
                    path: path.clone(),
                    error,
                })?;

            header.set_cksum();
            let expected_cksum = header
                .cksum()
                .expect("checksum that was just set can't be invalid");

            if expected_cksum != actual_cksum {
                return Err(ArchiveReadError::InvalidChecksum {
                    path,
                    expected: expected_cksum,
                    actual: actual_cksum,
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
