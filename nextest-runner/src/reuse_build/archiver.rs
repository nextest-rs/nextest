// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ArchiveEvent, Unarchiver, BINARIES_METADATA_FILE_NAME, CARGO_METADATA_FILE_NAME};
use crate::{
    errors::{ArchiveCreateError, ArchiveReadError},
    helpers::convert_rel_path_to_forward_slash,
    list::{BinaryList, OutputFormat, SerializableFormat},
    reuse_build::PathMapper,
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use std::{
    io::{self, BufWriter, Write},
    time::SystemTime,
};
use zstd::Encoder;

/// Archives test binaries along with metadata to the given file.
///
/// The output file is a Zstandard-compressed tarball (`.tar.zst`).
pub fn archive_to_file<'a, F>(
    binary_list: &'a BinaryList,
    cargo_metadata: &'a str,
    path_mapper: &'a PathMapper,
    compression_level: i32,
    output_file: &'a Utf8Path,
    mut callback: F,
) -> Result<(), ArchiveCreateError>
where
    F: FnMut(ArchiveEvent<'a>) -> io::Result<()>,
{
    let file = AtomicFile::new(output_file, OverwriteBehavior::AllowOverwrite);
    let test_binary_count = binary_list.rust_binaries.len();
    let non_test_binary_count = binary_list.rust_build_meta.non_test_binaries.len();
    let linked_path_count = binary_list.rust_build_meta.linked_paths.len();

    file.write(|file| {
        callback(ArchiveEvent::ArchiveStarted {
            test_binary_count,
            non_test_binary_count,
            linked_path_count,
            output_file,
        })
        .map_err(ArchiveCreateError::ReporterIo)?;
        // Write out the archive.
        let archiver = Archiver::new(
            binary_list,
            cargo_metadata,
            path_mapper,
            compression_level,
            file,
        )?;
        archiver.archive()?;

        Ok(())
    })
    .map_err(|err| match err {
        atomicwrites::Error::Internal(err) => ArchiveCreateError::OutputFileIo(err),
        atomicwrites::Error::User(err) => err,
    })?;

    // Read the archive to validate it and grab data. TODO: move this to the AtomicFile block once
    // https://github.com/untitaker/rust-atomicwrites/pull/53 is released.
    let mut file = std::fs::File::open(output_file)
        .map_err(|error| ArchiveCreateError::Validation(ArchiveReadError::Io(error)))?;
    let mut unarchiver = Unarchiver::new(&mut file);
    let info = unarchiver
        .get_info()
        .map_err(ArchiveCreateError::Validation)?;

    callback(ArchiveEvent::Archived {
        // TODO: obtain file count from archive
        file_count: info.file_count,
        output_file,
    })
    .map_err(ArchiveCreateError::ReporterIo)?;

    Ok(())
}

struct Archiver<'a, W: Write> {
    binary_list: &'a BinaryList,
    cargo_metadata: &'a str,
    path_mapper: &'a PathMapper,
    builder: tar::Builder<Encoder<'static, BufWriter<W>>>,
    unix_timestamp: u64,
}

impl<'a, W: Write> Archiver<'a, W> {
    fn new(
        binary_list: &'a BinaryList,
        cargo_metadata: &'a str,
        path_mapper: &'a PathMapper,
        compression_level: i32,
        writer: W,
    ) -> Result<Self, ArchiveCreateError> {
        let buf_writer = BufWriter::new(writer);
        let mut encoder = zstd::Encoder::new(buf_writer, compression_level)
            .map_err(ArchiveCreateError::OutputFileIo)?;
        encoder
            .include_checksum(true)
            .map_err(ArchiveCreateError::OutputFileIo)?;
        encoder
            .multithread(num_cpus::get() as u32)
            .map_err(ArchiveCreateError::OutputFileIo)?;
        let builder = tar::Builder::new(encoder);

        let unix_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("current time should be after 1970-01-01")
            .as_secs();

        Ok(Self {
            binary_list,
            cargo_metadata,
            path_mapper,
            builder,
            unix_timestamp,
        })
    }

    fn archive(mut self) -> Result<W, ArchiveCreateError> {
        self.add_file(CARGO_METADATA_FILE_NAME, self.cargo_metadata)?;

        let binaries_metadata = self
            .binary_list
            .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
            .map_err(ArchiveCreateError::CreateBinaryList)?;

        self.add_file(BINARIES_METADATA_FILE_NAME, &binaries_metadata)?;

        // Write all discovered binaries into the archive.
        let target_dir = &self.binary_list.rust_build_meta.target_directory;
        for binary in &self.binary_list.rust_binaries {
            let rel_path = binary
                .path
                .strip_prefix(target_dir.parent().expect("target dir cannot be the root"))
                .expect("binary paths must be within target directory");
            let rel_path = convert_rel_path_to_forward_slash(rel_path);
            self.builder
                .append_path_with_name(&binary.path, &rel_path)
                .map_err(ArchiveCreateError::OutputFileIo)?;
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

            self.builder
                .append_path_with_name(&src_path, &rel_path)
                .map_err(ArchiveCreateError::OutputFileIo)?;
        }

        // Write linked paths to the archive.
        for linked_path in &self.binary_list.rust_build_meta.linked_paths {
            // linked paths are e.g. debug/foo/bar. We need to prepend the target directory.
            let src_path = self
                .binary_list
                .rust_build_meta
                .target_directory
                .join(linked_path);
            let src_path = self.path_mapper.map_binary(src_path);

            let rel_path = Utf8Path::new("target").join(linked_path);
            let rel_path = convert_rel_path_to_forward_slash(&rel_path);
            self.builder
                .append_dir_all(&rel_path, &src_path)
                .map_err(ArchiveCreateError::OutputFileIo)?;
        }

        // TODO: add extra files.

        // Finish writing the archive.
        let encoder = self
            .builder
            .into_inner()
            .map_err(ArchiveCreateError::OutputFileIo)?;
        // Finish writing the zstd stream.
        let buf_writer = encoder.finish().map_err(ArchiveCreateError::OutputFileIo)?;
        let writer = buf_writer
            .into_inner()
            .map_err(|err| ArchiveCreateError::OutputFileIo(err.into_error()))?;

        Ok(writer)
    }

    // ---
    // Helper methods
    // ---

    fn add_file(&mut self, name: &str, contents: &str) -> Result<(), ArchiveCreateError> {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mtime(self.unix_timestamp);
        header.set_mode(0o664);
        header.set_cksum();

        self.builder
            .append_data(&mut header, name, io::Cursor::new(contents))
            .map_err(ArchiveCreateError::OutputFileIo)
    }
}
