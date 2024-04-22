// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ArchiveEvent, BINARIES_METADATA_FILE_NAME, CARGO_METADATA_FILE_NAME};
use crate::{
    config::{get_num_cpus, ArchiveInclude, FinalConfig, NextestProfile, RecursionDepth},
    errors::{ArchiveCreateError, UnknownArchiveFormat},
    helpers::{convert_rel_path_to_forward_slash, rel_path_join},
    list::{BinaryList, OutputFormat, SerializableFormat},
    reuse_build::PathMapper,
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use guppy::{graph::PackageGraph, PackageId};
use std::{
    collections::HashSet,
    fs,
    io::{self, BufWriter, Write},
    time::{Instant, SystemTime},
};
use zstd::Encoder;

/// Archive format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveFormat {
    /// A Zstandard-compressed tarball.
    TarZst,
}

impl ArchiveFormat {
    /// The list of supported formats as a list of (file extension, format) pairs.
    pub const SUPPORTED_FORMATS: &'static [(&'static str, Self)] = &[(".tar.zst", Self::TarZst)];

    /// Automatically detects an archive format from a given file name, and returns an error if the
    /// detection failed.
    pub fn autodetect(archive_file: &Utf8Path) -> Result<Self, UnknownArchiveFormat> {
        let file_name = archive_file.file_name().unwrap_or("");
        for (extension, format) in Self::SUPPORTED_FORMATS {
            if file_name.ends_with(extension) {
                return Ok(*format);
            }
        }

        Err(UnknownArchiveFormat {
            file_name: file_name.to_owned(),
        })
    }
}

/// Archives test binaries along with metadata to the given file.
///
/// The output file is a Zstandard-compressed tarball (`.tar.zst`).
#[allow(clippy::too_many_arguments)]
pub fn archive_to_file<'a, F>(
    profile: NextestProfile<'a, FinalConfig>,
    binary_list: &'a BinaryList,
    cargo_metadata: &'a str,
    graph: &'a PackageGraph,
    path_mapper: &'a PathMapper,
    format: ArchiveFormat,
    zstd_level: i32,
    output_file: &'a Utf8Path,
    mut callback: F,
) -> Result<(), ArchiveCreateError>
where
    F: for<'b> FnMut(ArchiveEvent<'b>) -> io::Result<()>,
{
    let file = AtomicFile::new(output_file, OverwriteBehavior::AllowOverwrite);
    let test_binary_count = binary_list.rust_binaries.len();
    let non_test_binary_count = binary_list.rust_build_meta.non_test_binaries.len();
    let build_script_out_dir_count = binary_list.rust_build_meta.build_script_out_dirs.len();
    let linked_path_count = binary_list.rust_build_meta.linked_paths.len();
    let start_time = Instant::now();

    let file_count = file
        .write(|file| {
            callback(ArchiveEvent::ArchiveStarted {
                test_binary_count,
                non_test_binary_count,
                build_script_out_dir_count,
                linked_path_count,
                output_file,
            })
            .map_err(ArchiveCreateError::ReporterIo)?;
            // Write out the archive.
            let archiver = Archiver::new(
                &profile,
                binary_list,
                cargo_metadata,
                graph,
                path_mapper,
                format,
                zstd_level,
                file,
            )?;
            let (_, file_count) = archiver.archive(&mut callback)?;
            Ok(file_count)
        })
        .map_err(|err| match err {
            atomicwrites::Error::Internal(err) => ArchiveCreateError::OutputArchiveIo(err),
            atomicwrites::Error::User(err) => err,
        })?;

    let elapsed = start_time.elapsed();

    callback(ArchiveEvent::Archived {
        file_count,
        output_file,
        elapsed,
    })
    .map_err(ArchiveCreateError::ReporterIo)?;

    Ok(())
}

struct Archiver<'a, W: Write> {
    binary_list: &'a BinaryList,
    cargo_metadata: &'a str,
    graph: &'a PackageGraph,
    path_mapper: &'a PathMapper,
    builder: tar::Builder<Encoder<'static, BufWriter<W>>>,
    unix_timestamp: u64,
    added_files: HashSet<Utf8PathBuf>,
    archive_include: &'a [ArchiveInclude],
}

impl<'a, W: Write> Archiver<'a, W> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        profile: &'a NextestProfile<'a, FinalConfig>,
        binary_list: &'a BinaryList,
        cargo_metadata: &'a str,
        graph: &'a PackageGraph,
        path_mapper: &'a PathMapper,
        format: ArchiveFormat,
        compression_level: i32,
        writer: W,
    ) -> Result<Self, ArchiveCreateError> {
        let buf_writer = BufWriter::new(writer);
        let builder = match format {
            ArchiveFormat::TarZst => {
                let mut encoder = zstd::Encoder::new(buf_writer, compression_level)
                    .map_err(ArchiveCreateError::OutputArchiveIo)?;
                encoder
                    .include_checksum(true)
                    .map_err(ArchiveCreateError::OutputArchiveIo)?;
                encoder
                    .multithread(get_num_cpus() as u32)
                    .map_err(ArchiveCreateError::OutputArchiveIo)?;
                tar::Builder::new(encoder)
            }
        };

        let unix_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("current time should be after 1970-01-01")
            .as_secs();

        Ok(Self {
            binary_list,
            cargo_metadata,
            graph,
            path_mapper,
            builder,
            unix_timestamp,
            added_files: HashSet::new(),
            archive_include: profile.archive_include(),
        })
    }

    fn archive<F>(mut self, callback: &mut F) -> Result<(W, usize), ArchiveCreateError>
    where
        F: for<'b> FnMut(ArchiveEvent<'b>) -> io::Result<()>,
    {
        // Add the binaries metadata first so that while unarchiving, reports are instant.
        let binaries_metadata = self
            .binary_list
            .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
            .map_err(ArchiveCreateError::CreateBinaryList)?;

        self.append_from_memory(BINARIES_METADATA_FILE_NAME, &binaries_metadata)?;

        self.append_from_memory(CARGO_METADATA_FILE_NAME, self.cargo_metadata)?;

        let target_dir = &self.binary_list.rust_build_meta.target_directory;

        // Check that all archive-include paths exist.
        let archive_include_paths: Vec<_> = self
            .archive_include
            .iter()
            .filter_map(|include| {
                let src_path = target_dir.join(&include.path);
                let src_path = self.path_mapper.map_binary(src_path);

                if src_path.exists() {
                    Some((include, src_path))
                } else {
                    log::debug!(
                        target: "nextest-runner",
                        "archive-include path `{}` does not exist, ignoring",
                        include.path
                    );
                    None
                }
            })
            .collect();

        // Write all discovered binaries into the archive.
        for binary in &self.binary_list.rust_binaries {
            let rel_path = binary
                .path
                .strip_prefix(target_dir)
                .expect("binary paths must be within target directory");
            // The target directory might not be called "target", so strip all of it then add
            // "target" to the beginning.
            let rel_path = Utf8Path::new("target").join(rel_path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            self.append_file(&binary.path, &rel_path)?;
        }
        for non_test_binary in self
            .binary_list
            .rust_build_meta
            .non_test_binaries
            .iter()
            .flat_map(|(_, binaries)| binaries)
        {
            let src_path = self
                .binary_list
                .rust_build_meta
                .target_directory
                .join(&non_test_binary.path);
            let src_path = self.path_mapper.map_binary(src_path);

            let rel_path = Utf8Path::new("target").join(&non_test_binary.path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            self.append_file(&src_path, &rel_path)?;
        }

        // Write build script output directories to the archive.
        for build_script_out_dir in self
            .binary_list
            .rust_build_meta
            .build_script_out_dirs
            .values()
        {
            let src_path = self
                .binary_list
                .rust_build_meta
                .target_directory
                .join(build_script_out_dir);
            let src_path = self.path_mapper.map_binary(src_path);

            let rel_path = Utf8Path::new("target").join(build_script_out_dir);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            // XXX: For now, we only archive one level of build script output directories as a
            // conservative solution. If necessary, we may have to either broaden this by default or
            // add configuration for this. Archiving too much can cause unnecessary slowdowns.
            self.append_path_recursive(
                &src_path,
                &rel_path,
                RecursionDepth::Finite(1),
                false,
                callback,
            )?;
        }

        // Write linked paths to the archive.
        for (linked_path, requested_by) in &self.binary_list.rust_build_meta.linked_paths {
            // Linked paths are relative, e.g. debug/foo/bar. We need to prepend the target
            // directory.
            let src_path = self
                .binary_list
                .rust_build_meta
                .target_directory
                .join(linked_path);
            let src_path = self.path_mapper.map_binary(src_path);

            // Some crates produce linked paths that don't exist. This is a bug in those libraries.
            if !src_path.exists() {
                // Map each requested_by to its package name and version.
                let mut requested_by: Vec<_> = requested_by
                    .iter()
                    .map(|package_id| {
                        self.graph
                            .metadata(&PackageId::new(package_id.clone()))
                            .map_or_else(
                                |_| {
                                    // If a package ID is not found in the graph, it's strange but not
                                    // fatal -- just use the ID.
                                    package_id.to_owned()
                                },
                                |metadata| format!("{} v{}", metadata.name(), metadata.version()),
                            )
                    })
                    .collect();
                requested_by.sort_unstable();

                callback(ArchiveEvent::LinkedPathNotFound {
                    path: &src_path,
                    requested_by: &requested_by,
                })
                .map_err(ArchiveCreateError::ReporterIo)?;
                continue;
            }

            let rel_path = Utf8Path::new("target").join(linked_path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);
            // Since LD_LIBRARY_PATH etc aren't recursive, we only need to add the top-level files
            // from linked paths.
            self.append_path_recursive(
                &src_path,
                &rel_path,
                RecursionDepth::Finite(1),
                false,
                callback,
            )?;
        }

        // Also include extra paths.
        for (include, src_path) in archive_include_paths {
            let rel_path = Utf8Path::new("target").join(&include.path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            if src_path.exists() {
                // Warn if the implicit depth limit for these paths is in use.
                let warn_on_exceed_depth = !include.depth.is_deserialized;
                self.append_path_recursive(
                    &src_path,
                    &rel_path,
                    include.depth.value,
                    warn_on_exceed_depth,
                    callback,
                )?;
            }
        }

        // Finish writing the archive.
        let encoder = self
            .builder
            .into_inner()
            .map_err(ArchiveCreateError::OutputArchiveIo)?;
        // Finish writing the zstd stream.
        let buf_writer = encoder
            .finish()
            .map_err(ArchiveCreateError::OutputArchiveIo)?;
        let writer = buf_writer
            .into_inner()
            .map_err(|err| ArchiveCreateError::OutputArchiveIo(err.into_error()))?;

        Ok((writer, self.added_files.len()))
    }

    // ---
    // Helper methods
    // ---

    fn append_from_memory(&mut self, name: &str, contents: &str) -> Result<(), ArchiveCreateError> {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mtime(self.unix_timestamp);
        header.set_mode(0o664);
        header.set_cksum();

        self.builder
            .append_data(&mut header, name, io::Cursor::new(contents))
            .map_err(ArchiveCreateError::OutputArchiveIo)?;
        // We always prioritize appending files from memory over files on disk, so don't check
        // membership in added_files before adding the file to the archive.
        self.added_files.insert(name.into());
        Ok(())
    }

    fn append_path_recursive<F>(
        &mut self,
        src_path: &Utf8Path,
        rel_path: &Utf8Path,
        limit: RecursionDepth,
        warn_on_exceed_depth: bool,
        callback: &mut F,
    ) -> Result<(), ArchiveCreateError>
    where
        F: for<'b> FnMut(ArchiveEvent<'b>) -> io::Result<()>,
    {
        // Within the loop, the metadata will be part of the directory entry.
        let metadata =
            fs::symlink_metadata(src_path).map_err(|error| ArchiveCreateError::InputFileRead {
                path: src_path.to_owned(),
                is_dir: None,
                error,
            })?;

        // Use an explicit stack to avoid the unlikely but possible situation of a stack overflow.
        let mut stack = vec![(limit, src_path.to_owned(), rel_path.to_owned(), metadata)];

        while let Some((depth, src_path, rel_path, metadata)) = stack.pop() {
            log::trace!(
                target: "nextest-runner",
                "processing `{src_path}` with metadata {metadata:?} \
                 (depth: {depth})",
            );

            if metadata.is_dir() {
                // Check the recursion limit.
                if depth.is_zero() {
                    callback(ArchiveEvent::RecursionDepthExceeded {
                        path: &src_path,
                        limit: limit.unwrap_finite(),
                        warn: warn_on_exceed_depth,
                    })
                    .map_err(ArchiveCreateError::ReporterIo)?;
                    continue;
                }

                // Iterate over this directory.
                log::debug!(
                    target: "nextest-runner",
                    "recursing into `{}`",
                    src_path
                );
                let entries = src_path.read_dir_utf8().map_err(|error| {
                    ArchiveCreateError::InputFileRead {
                        path: src_path.to_owned(),
                        is_dir: Some(true),
                        error,
                    }
                })?;
                for entry in entries {
                    let entry = entry.map_err(|error| ArchiveCreateError::DirEntryRead {
                        path: src_path.to_owned(),
                        error,
                    })?;
                    let metadata =
                        entry
                            .metadata()
                            .map_err(|error| ArchiveCreateError::InputFileRead {
                                path: entry.path().to_owned(),
                                is_dir: None,
                                error,
                            })?;
                    let entry_rel_path = rel_path_join(&rel_path, entry.file_name().as_ref());
                    stack.push((
                        depth.decrement(),
                        entry.into_path(),
                        entry_rel_path,
                        metadata,
                    ));
                }
            } else if metadata.is_file() || metadata.is_symlink() {
                self.append_file(&src_path, &rel_path)?;
            } else {
                // Don't archive other kinds of files.
                callback(ArchiveEvent::UnknownFileType { path: &src_path })
                    .map_err(ArchiveCreateError::ReporterIo)?;
            }
        }

        Ok(())
    }

    fn append_file(&mut self, src: &Utf8Path, dest: &Utf8Path) -> Result<(), ArchiveCreateError> {
        // Check added_files to ensure we aren't adding duplicate files.
        if !self.added_files.contains(dest) {
            log::debug!(
                target: "nextest-runner",
                "adding `{src}` to archive as `{dest}`",
            );
            self.builder
                .append_path_with_name(src, dest)
                .map_err(|error| ArchiveCreateError::InputFileRead {
                    path: src.to_owned(),
                    is_dir: Some(false),
                    error,
                })?;
            self.added_files.insert(dest.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_format_autodetect() {
        assert_eq!(
            ArchiveFormat::autodetect("foo.tar.zst".as_ref()).unwrap(),
            ArchiveFormat::TarZst,
        );
        assert_eq!(
            ArchiveFormat::autodetect("foo/bar.tar.zst".as_ref()).unwrap(),
            ArchiveFormat::TarZst,
        );
        ArchiveFormat::autodetect("foo".as_ref()).unwrap_err();
        ArchiveFormat::autodetect("/".as_ref()).unwrap_err();
    }
}
