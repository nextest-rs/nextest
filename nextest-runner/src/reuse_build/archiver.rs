// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ArchiveCounts, ArchiveEvent, BINARIES_METADATA_FILE_NAME, CARGO_METADATA_FILE_NAME};
use crate::{
    config::{
        core::{EvaluatableProfile, get_num_cpus},
        elements::{ArchiveConfig, ArchiveIncludeOnMissing, RecursionDepth},
    },
    errors::{ArchiveCreateError, FromMessagesError, UnknownArchiveFormat},
    helpers::{convert_rel_path_to_forward_slash, rel_path_join},
    list::{BinaryList, OutputFormat, RustBuildMeta, RustTestArtifact, SerializableFormat},
    redact::Redactor,
    reuse_build::{ArchiveFilterCounts, LIBDIRS_BASE_DIR, PathMapper},
    test_filter::{BinaryFilter, FilterBinaryMatch, FilterBound},
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use core::fmt;
use guppy::{PackageId, graph::PackageGraph};
use nextest_filtering::EvalContext;
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, BufWriter, Write},
    sync::Arc,
    time::{Instant, SystemTime},
};
use tracing::{debug, trace, warn};
use zstd::Encoder;

/// Applies archive filters to a [`BinaryList`].
pub fn apply_archive_filters(
    graph: &PackageGraph,
    binary_list: Arc<BinaryList>,
    filter: &BinaryFilter,
    ecx: &EvalContext<'_>,
    path_mapper: &PathMapper,
) -> Result<(BinaryList, ArchiveFilterCounts), FromMessagesError> {
    let rust_build_meta = binary_list.rust_build_meta.map_paths(path_mapper);
    let test_artifacts = RustTestArtifact::from_binary_list(
        graph,
        binary_list.clone(),
        &rust_build_meta,
        path_mapper,
        None,
    )?;

    // Apply filterset to `RustTestArtifact` list.
    let test_artifacts: BTreeSet<_> = test_artifacts
        .iter()
        .filter(|test_artifact| {
            // Don't obey the default filter here. The default filter will
            // be applied while running tests from the archive (the
            // configuration is expected to be present at that time).
            let filter_match = filter.check_match(test_artifact, ecx, FilterBound::All);

            debug_assert!(
                !matches!(filter_match, FilterBinaryMatch::Possible),
                "build_filtersets should have errored out on test filters, \
                 Possible should never be returned"
            );
            matches!(filter_match, FilterBinaryMatch::Definite)
        })
        .map(|test_artifact| &test_artifact.binary_id)
        .collect();

    let filtered_binaries: Vec<_> = binary_list
        .rust_binaries
        .iter()
        .filter(|binary| test_artifacts.contains(&binary.id))
        .cloned()
        .collect();

    // Build a map of package IDs included in the filtered set, then use that to
    // filter out non-test binaries not referred to by any package.
    let relevant_package_ids: HashSet<_> = filtered_binaries
        .iter()
        .map(|binary| &binary.package_id)
        .collect();
    let mut filtered_non_test_binaries = binary_list.rust_build_meta.non_test_binaries.clone();
    filtered_non_test_binaries.retain(|package_id, _| relevant_package_ids.contains(package_id));

    // Also filter out build script out directories.
    let mut filtered_build_script_out_dirs =
        binary_list.rust_build_meta.build_script_out_dirs.clone();
    filtered_build_script_out_dirs
        .retain(|package_id, _| relevant_package_ids.contains(package_id));

    let filtered_out_test_binary_count = binary_list
        .rust_binaries
        .len()
        .saturating_sub(filtered_binaries.len());
    let filtered_out_non_test_binary_count = binary_list
        .rust_build_meta
        .non_test_binaries
        .len()
        .saturating_sub(filtered_non_test_binaries.len());
    let filtered_out_build_script_out_dir_count = binary_list
        .rust_build_meta
        .build_script_out_dirs
        .len()
        .saturating_sub(filtered_build_script_out_dirs.len());

    let filtered_build_meta = RustBuildMeta {
        non_test_binaries: filtered_non_test_binaries,
        build_script_out_dirs: filtered_build_script_out_dirs,
        ..binary_list.rust_build_meta.clone()
    };

    Ok((
        BinaryList {
            rust_build_meta: filtered_build_meta,
            rust_binaries: filtered_binaries,
        },
        ArchiveFilterCounts {
            filtered_out_test_binary_count,
            filtered_out_non_test_binary_count,
            filtered_out_build_script_out_dir_count,
        },
    ))
}

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
#[expect(clippy::too_many_arguments)]
pub fn archive_to_file<'a, F>(
    profile: EvaluatableProfile<'a>,
    binary_list: &'a BinaryList,
    filter_counts: ArchiveFilterCounts,
    cargo_metadata: &'a str,
    graph: &'a PackageGraph,
    path_mapper: &'a PathMapper,
    format: ArchiveFormat,
    zstd_level: i32,
    output_file: &'a Utf8Path,
    mut callback: F,
    redactor: Redactor,
) -> Result<(), ArchiveCreateError>
where
    F: for<'b> FnMut(ArchiveEvent<'b>) -> io::Result<()>,
{
    let config = profile.archive_config();

    let start_time = Instant::now();

    let file = AtomicFile::new(output_file, OverwriteBehavior::AllowOverwrite);
    let file_count = file
        .write(|file| {
            // Tests require the standard library in two cases:
            // * proc-macro tests (host)
            // * tests compiled with -C prefer-dynamic (target)
            //
            // We only care about libstd -- empirically, other libraries in the path aren't
            // required.
            let (host_stdlib, host_stdlib_err) = if let Some(libdir) = binary_list
                .rust_build_meta
                .build_platforms
                .host
                .libdir
                .as_path()
            {
                split_result(find_std(libdir))
            } else {
                (None, None)
            };

            let (target_stdlib, target_stdlib_err) =
                if let Some(target) = &binary_list.rust_build_meta.build_platforms.target {
                    if let Some(libdir) = target.libdir.as_path() {
                        split_result(find_std(libdir))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

            let stdlib_count = host_stdlib.is_some() as usize + target_stdlib.is_some() as usize;

            let archiver = Archiver::new(
                config,
                binary_list,
                cargo_metadata,
                graph,
                path_mapper,
                host_stdlib,
                target_stdlib,
                format,
                zstd_level,
                file,
                redactor,
            )?;

            let test_binary_count = binary_list.rust_binaries.len();
            let non_test_binary_count = binary_list.rust_build_meta.non_test_binaries.len();
            let build_script_out_dir_count =
                binary_list.rust_build_meta.build_script_out_dirs.len();
            let linked_path_count = binary_list.rust_build_meta.linked_paths.len();
            let extra_path_count = config.include.len();

            let counts = ArchiveCounts {
                test_binary_count,
                filter_counts,
                non_test_binary_count,
                build_script_out_dir_count,
                linked_path_count,
                extra_path_count,
                stdlib_count,
            };

            callback(ArchiveEvent::ArchiveStarted {
                counts,
                output_file,
            })
            .map_err(ArchiveCreateError::ReporterIo)?;

            // Was there an error finding the standard library?
            if let Some(err) = host_stdlib_err {
                callback(ArchiveEvent::StdlibPathError {
                    error: &err.to_string(),
                })
                .map_err(ArchiveCreateError::ReporterIo)?;
            }
            if let Some(err) = target_stdlib_err {
                callback(ArchiveEvent::StdlibPathError {
                    error: &err.to_string(),
                })
                .map_err(ArchiveCreateError::ReporterIo)?;
            }

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
    host_stdlib: Option<Utf8PathBuf>,
    target_stdlib: Option<Utf8PathBuf>,
    builder: tar::Builder<Encoder<'static, BufWriter<W>>>,
    unix_timestamp: u64,
    added_files: HashSet<Utf8PathBuf>,
    config: &'a ArchiveConfig,
    redactor: Redactor,
}

impl<'a, W: Write> Archiver<'a, W> {
    #[expect(clippy::too_many_arguments)]
    fn new(
        config: &'a ArchiveConfig,
        binary_list: &'a BinaryList,
        cargo_metadata: &'a str,
        graph: &'a PackageGraph,
        path_mapper: &'a PathMapper,
        host_stdlib: Option<Utf8PathBuf>,
        target_stdlib: Option<Utf8PathBuf>,
        format: ArchiveFormat,
        compression_level: i32,
        writer: W,
        redactor: Redactor,
    ) -> Result<Self, ArchiveCreateError> {
        let buf_writer = BufWriter::new(writer);
        let builder = match format {
            ArchiveFormat::TarZst => {
                let mut encoder = zstd::Encoder::new(buf_writer, compression_level)
                    .map_err(ArchiveCreateError::OutputArchiveIo)?;
                encoder
                    .include_checksum(true)
                    .map_err(ArchiveCreateError::OutputArchiveIo)?;
                if let Err(err) = encoder.multithread(get_num_cpus() as u32) {
                    tracing::warn!(
                        ?err,
                        "libzstd compiled without multithreading, defaulting to single-thread"
                    );
                }
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
            host_stdlib,
            target_stdlib,
            builder,
            unix_timestamp,
            added_files: HashSet::new(),
            config,
            redactor,
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

        fn filter_map_err<T>(result: io::Result<()>) -> Option<Result<T, ArchiveCreateError>> {
            match result {
                Ok(()) => None,
                Err(err) => Some(Err(ArchiveCreateError::ReporterIo(err))),
            }
        }

        // Check that all archive.include paths exist.
        let archive_include_paths = self
            .config
            .include
            .iter()
            .filter_map(|include| {
                let src_path = include.join_path(target_dir);
                let src_path = self.path_mapper.map_binary(src_path);

                match src_path.symlink_metadata() {
                    Ok(metadata) => {
                        if metadata.is_dir() {
                            if include.depth().is_zero() {
                                // A directory with depth 0 will not be archived, so warn on that.
                                filter_map_err(callback(ArchiveEvent::DirectoryAtDepthZero {
                                    path: &src_path,
                                }))
                            } else {
                                Some(Ok((include, src_path)))
                            }
                        } else if metadata.is_file() || metadata.is_symlink() {
                            Some(Ok((include, src_path)))
                        } else {
                            filter_map_err(callback(ArchiveEvent::UnknownFileType {
                                step: ArchiveStep::ExtraPaths,
                                path: &src_path,
                            }))
                        }
                    }
                    Err(error) => {
                        if error.kind() == io::ErrorKind::NotFound {
                            match include.on_missing() {
                                ArchiveIncludeOnMissing::Error => {
                                    // TODO: accumulate errors rather than failing on the first one
                                    Some(Err(ArchiveCreateError::MissingExtraPath {
                                        path: src_path.to_owned(),
                                        redactor: self.redactor.clone(),
                                    }))
                                }
                                ArchiveIncludeOnMissing::Warn => {
                                    filter_map_err(callback(ArchiveEvent::ExtraPathMissing {
                                        path: &src_path,
                                        warn: true,
                                    }))
                                }
                                ArchiveIncludeOnMissing::Ignore => {
                                    filter_map_err(callback(ArchiveEvent::ExtraPathMissing {
                                        path: &src_path,
                                        warn: false,
                                    }))
                                }
                            }
                        } else {
                            Some(Err(ArchiveCreateError::InputFileRead {
                                step: ArchiveStep::ExtraPaths,
                                path: src_path.to_owned(),
                                is_dir: None,
                                error,
                            }))
                        }
                    }
                }
            })
            .collect::<Result<Vec<_>, ArchiveCreateError>>()?;

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

            self.append_file(ArchiveStep::TestBinaries, &binary.path, &rel_path)?;
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

            self.append_file(ArchiveStep::NonTestBinaries, &src_path, &rel_path)?;
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
                ArchiveStep::BuildScriptOutDirs,
                &src_path,
                &rel_path,
                RecursionDepth::Finite(1),
                false,
                callback,
            )?;

            // Archive build script output in order to set environment variables from there
            let Some(out_dir_parent) = build_script_out_dir.parent() else {
                warn!(
                    "could not determine parent directory of output directory {build_script_out_dir}"
                );
                continue;
            };
            let out_file_path = out_dir_parent.join("output");
            let src_path = self
                .binary_list
                .rust_build_meta
                .target_directory
                .join(&out_file_path);

            let rel_path = Utf8Path::new("target").join(out_file_path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            self.append_file(ArchiveStep::BuildScriptOutDirs, &src_path, &rel_path)?;
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
                ArchiveStep::LinkedPaths,
                &src_path,
                &rel_path,
                RecursionDepth::Finite(1),
                false,
                callback,
            )?;
        }

        // Also include extra paths.
        for (include, src_path) in archive_include_paths {
            let rel_path = include.join_path(Utf8Path::new("target"));
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            if src_path.exists() {
                self.append_path_recursive(
                    ArchiveStep::ExtraPaths,
                    &src_path,
                    &rel_path,
                    include.depth(),
                    // Warn if the implicit depth limit for these paths is in use.
                    true,
                    callback,
                )?;
            }
        }

        // Add the standard libraries to the archive if available.
        if let Some(host_stdlib) = self.host_stdlib.clone() {
            let rel_path = Utf8Path::new(LIBDIRS_BASE_DIR)
                .join("host")
                .join(host_stdlib.file_name().unwrap());
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            self.append_file(ArchiveStep::ExtraPaths, &host_stdlib, &rel_path)?;
        }
        if let Some(target_stdlib) = self.target_stdlib.clone() {
            // Use libdir/target/0 as the path to the target standard library, to support multiple
            // targets in the future.
            let rel_path = Utf8Path::new(LIBDIRS_BASE_DIR)
                .join("target/0")
                .join(target_stdlib.file_name().unwrap());
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);

            self.append_file(ArchiveStep::ExtraPaths, &target_stdlib, &rel_path)?;
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
        step: ArchiveStep,
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
                step,
                path: src_path.to_owned(),
                is_dir: None,
                error,
            })?;

        // Use an explicit stack to avoid the unlikely but possible situation of a stack overflow.
        let mut stack = vec![(limit, src_path.to_owned(), rel_path.to_owned(), metadata)];

        while let Some((depth, src_path, rel_path, metadata)) = stack.pop() {
            trace!(
                target: "nextest-runner",
                "processing `{src_path}` with metadata {metadata:?} \
                 (depth: {depth})",
            );

            if metadata.is_dir() {
                // Check the recursion limit.
                if depth.is_zero() {
                    callback(ArchiveEvent::RecursionDepthExceeded {
                        step,
                        path: &src_path,
                        limit: limit.unwrap_finite(),
                        warn: warn_on_exceed_depth,
                    })
                    .map_err(ArchiveCreateError::ReporterIo)?;
                    continue;
                }

                // Iterate over this directory.
                debug!(
                    target: "nextest-runner",
                    "recursing into `{}`",
                    src_path
                );
                let entries = src_path.read_dir_utf8().map_err(|error| {
                    ArchiveCreateError::InputFileRead {
                        step,
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
                                step,
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
                self.append_file(step, &src_path, &rel_path)?;
            } else {
                // Don't archive other kinds of files.
                callback(ArchiveEvent::UnknownFileType {
                    step,
                    path: &src_path,
                })
                .map_err(ArchiveCreateError::ReporterIo)?;
            }
        }

        Ok(())
    }

    fn append_file(
        &mut self,
        step: ArchiveStep,
        src: &Utf8Path,
        dest: &Utf8Path,
    ) -> Result<(), ArchiveCreateError> {
        // Check added_files to ensure we aren't adding duplicate files.
        if !self.added_files.contains(dest) {
            debug!(
                target: "nextest-runner",
                "adding `{src}` to archive as `{dest}`",
            );
            self.builder
                .append_path_with_name(src, dest)
                .map_err(|error| ArchiveCreateError::InputFileRead {
                    step,
                    path: src.to_owned(),
                    is_dir: Some(false),
                    error,
                })?;
            self.added_files.insert(dest.into());
        }
        Ok(())
    }
}

fn find_std(libdir: &Utf8Path) -> io::Result<Utf8PathBuf> {
    for path in libdir.read_dir_utf8()? {
        let path = path?;
        // As of Rust 1.78, std is of the form:
        //
        //   libstd-<hash>.so (non-macOS Unix)
        //   libstd-<hash>.dylib (macOS)
        //   std-<hash>.dll (Windows)
        let file_name = path.file_name();
        let is_unix = file_name.starts_with("libstd-")
            && (file_name.ends_with(".so") || file_name.ends_with(".dylib"));
        let is_windows = file_name.starts_with("std-") && file_name.ends_with(".dll");

        if is_unix || is_windows {
            return Ok(path.into_path());
        }
    }

    Err(io::Error::other(
        "could not find the Rust standard library in the libdir",
    ))
}

fn split_result<T, E>(result: Result<T, E>) -> (Option<T>, Option<E>) {
    match result {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    }
}

/// The part of the archive process that is currently in progress.
///
/// This is used for better warnings and errors.
#[derive(Clone, Copy, Debug)]
pub enum ArchiveStep {
    /// Test binaries are being archived.
    TestBinaries,

    /// Non-test binaries are being archived.
    NonTestBinaries,

    /// Build script output directories are being archived.
    BuildScriptOutDirs,

    /// Linked paths are being archived.
    LinkedPaths,

    /// Extra paths are being archived.
    ExtraPaths,

    /// The standard library is being archived.
    Stdlib,
}

impl fmt::Display for ArchiveStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TestBinaries => write!(f, "test binaries"),
            Self::NonTestBinaries => write!(f, "non-test binaries"),
            Self::BuildScriptOutDirs => write!(f, "build script output directories"),
            Self::LinkedPaths => write!(f, "linked paths"),
            Self::ExtraPaths => write!(f, "extra paths"),
            Self::Stdlib => write!(f, "standard library"),
        }
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
