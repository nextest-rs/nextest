// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Portable archive creation and reading for recorded runs.
//!
//! A portable archive packages a single recorded run into a self-contained zip
//! file that can be shared and imported elsewhere.
//!
//! # Reading portable archives
//!
//! Use [`PortableArchive::open`] to open a portable archive for reading. The
//! archive contains:
//!
//! - A manifest (`manifest.json`) with run metadata.
//! - A run log (`run.log.zst`) with test events.
//! - An inner store (`store.zip`) with metadata and test output.
//!
//! To read from the inner store, call [`PortableArchive::open_store`] to get a
//! [`PortableStoreReader`] that implements [`StoreReader`](super::reader::StoreReader).

use super::{
    format::{
        CARGO_METADATA_JSON_PATH, OutputDict, PORTABLE_ARCHIVE_FORMAT_VERSION,
        PORTABLE_MANIFEST_FILE_NAME, PortableManifest, RECORD_OPTS_JSON_PATH, RERUN_INFO_JSON_PATH,
        RUN_LOG_FILE_NAME, RerunInfo, STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_FORMAT_VERSION,
        STORE_ZIP_FILE_NAME, TEST_LIST_JSON_PATH,
    },
    reader::{StoreReader, decompress_with_dict},
    store::{RecordedRunInfo, RunFilesExist, StoreRunsDir},
    summary::{RecordOpts, TestEventSummary, ZipStoreOutput},
};
use crate::{
    errors::{PortableArchiveError, PortableArchiveReadError, RecordReadError},
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use countio::Counter;
use debug_ignore::DebugIgnore;
use nextest_metadata::TestListSummary;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
};
use zip::{
    CompressionMethod, ZipArchive, ZipWriter, read::ZipFileSeek, result::ZipError,
    write::SimpleFileOptions,
};

/// Result of writing a portable archive.
#[derive(Debug)]
pub struct PortableArchiveResult {
    /// The path to the written archive.
    pub path: Utf8PathBuf,
    /// The total size of the archive in bytes.
    pub size: u64,
}

/// Result of extracting a file from a portable archive.
#[derive(Debug)]
pub struct ExtractOuterFileResult {
    /// The number of bytes written to the output file.
    pub bytes_written: u64,
    /// If the file size exceeded the limit threshold, contains the claimed size.
    ///
    /// This is informational only; the full file is always extracted regardless
    /// of whether this is `Some`.
    pub exceeded_limit: Option<u64>,
}

/// Writer to create a portable archive from a recorded run.
#[derive(Debug)]
pub struct PortableArchiveWriter<'a> {
    run_info: &'a RecordedRunInfo,
    run_dir: Utf8PathBuf,
}

impl<'a> PortableArchiveWriter<'a> {
    /// Creates a new writer for the given run.
    ///
    /// Validates that the run directory exists and contains the required files.
    pub fn new(
        run_info: &'a RecordedRunInfo,
        runs_dir: StoreRunsDir<'_>,
    ) -> Result<Self, PortableArchiveError> {
        let run_dir = runs_dir.run_dir(run_info.run_id);

        if !run_dir.exists() {
            return Err(PortableArchiveError::RunDirNotFound { path: run_dir });
        }

        let store_zip_path = run_dir.join(STORE_ZIP_FILE_NAME);
        if !store_zip_path.exists() {
            return Err(PortableArchiveError::RequiredFileMissing {
                run_dir,
                file_name: STORE_ZIP_FILE_NAME,
            });
        }

        let run_log_path = run_dir.join(RUN_LOG_FILE_NAME);
        if !run_log_path.exists() {
            return Err(PortableArchiveError::RequiredFileMissing {
                run_dir,
                file_name: RUN_LOG_FILE_NAME,
            });
        }

        Ok(Self { run_info, run_dir })
    }

    /// Returns the default filename for this archive.
    ///
    /// Format: `nextest-run-{run_id}.zip`
    pub fn default_filename(&self) -> String {
        format!("nextest-run-{}.zip", self.run_info.run_id)
    }

    /// Writes the portable archive to the given directory.
    ///
    /// The archive is written atomically using a temporary file and rename.
    /// The filename will be the default filename (`nextest-run-{run_id}.zip`).
    pub fn write_to_dir(
        &self,
        output_dir: &Utf8Path,
    ) -> Result<PortableArchiveResult, PortableArchiveError> {
        let output_path = output_dir.join(self.default_filename());
        self.write_to_path(&output_path)
    }

    /// Writes the portable archive to the given path.
    ///
    /// The archive is written atomically using a temporary file and rename.
    pub fn write_to_path(
        &self,
        output_path: &Utf8Path,
    ) -> Result<PortableArchiveResult, PortableArchiveError> {
        let atomic_file = AtomicFile::new(output_path, OverwriteBehavior::AllowOverwrite);

        let final_size = atomic_file
            .write(|temp_file| {
                let counter = Counter::new(temp_file);
                let mut zip_writer = ZipWriter::new(counter);

                self.write_manifest(&mut zip_writer)?;
                self.copy_file(&mut zip_writer, RUN_LOG_FILE_NAME)?;
                self.copy_file(&mut zip_writer, STORE_ZIP_FILE_NAME)?;

                let counter = zip_writer
                    .finish()
                    .map_err(PortableArchiveError::ZipFinalize)?;

                // Prefer the actual file size from metadata since ZipWriter
                // seeks and overwrites headers, causing the counter to
                // overcount. Fall back to the counter value if metadata is
                // unavailable.
                let counter_bytes = counter.writer_bytes() as u64;
                let file = counter.into_inner();
                let size = file.metadata().map(|m| m.len()).unwrap_or(counter_bytes);

                Ok(size)
            })
            .map_err(|err| match err {
                atomicwrites::Error::Internal(source) => PortableArchiveError::AtomicWrite {
                    path: output_path.to_owned(),
                    source,
                },
                atomicwrites::Error::User(e) => e,
            })?;

        Ok(PortableArchiveResult {
            path: output_path.to_owned(),
            size: final_size,
        })
    }

    /// Writes the manifest to the archive.
    fn write_manifest<W: Write + io::Seek>(
        &self,
        zip_writer: &mut ZipWriter<W>,
    ) -> Result<(), PortableArchiveError> {
        let manifest = PortableManifest::new(self.run_info);
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(PortableArchiveError::SerializeManifest)?;

        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        zip_writer
            .start_file(PORTABLE_MANIFEST_FILE_NAME, options)
            .map_err(|source| PortableArchiveError::ZipStartFile {
                file_name: PORTABLE_MANIFEST_FILE_NAME,
                source,
            })?;

        zip_writer
            .write_all(&manifest_json)
            .map_err(|source| PortableArchiveError::ZipWrite {
                file_name: PORTABLE_MANIFEST_FILE_NAME,
                source,
            })?;

        Ok(())
    }

    /// Copies a file from the run directory to the archive.
    ///
    /// The file is stored without additional compression since `run.log.zst`
    /// and `store.zip` are already compressed.
    fn copy_file<W: Write + io::Seek>(
        &self,
        zip_writer: &mut ZipWriter<W>,
        file_name: &'static str,
    ) -> Result<(), PortableArchiveError> {
        let source_path = self.run_dir.join(file_name);
        let mut file = File::open(&source_path)
            .map_err(|source| PortableArchiveError::ReadFile { file_name, source })?;

        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        zip_writer
            .start_file(file_name, options)
            .map_err(|source| PortableArchiveError::ZipStartFile { file_name, source })?;

        io::copy(&mut file, zip_writer)
            .map_err(|source| PortableArchiveError::ZipWrite { file_name, source })?;

        Ok(())
    }
}

// ---
// Portable archive reading
// ---

/// A portable archive opened for reading.
#[derive(Debug)]
pub struct PortableArchive {
    archive_path: Utf8PathBuf,
    manifest: PortableManifest,
    outer_archive: ZipArchive<File>,
}

impl RunFilesExist for PortableArchive {
    fn store_zip_exists(&self) -> bool {
        self.outer_archive
            .index_for_name(STORE_ZIP_FILE_NAME)
            .is_some()
    }

    fn run_log_exists(&self) -> bool {
        self.outer_archive
            .index_for_name(RUN_LOG_FILE_NAME)
            .is_some()
    }
}

impl PortableArchive {
    /// Opens a portable archive from a file path.
    ///
    /// Validates the format and store versions on open to fail fast if the
    /// archive cannot be read by this version of nextest.
    pub fn open(path: &Utf8Path) -> Result<Self, PortableArchiveReadError> {
        let file = File::open(path).map_err(|error| PortableArchiveReadError::OpenArchive {
            path: path.to_owned(),
            error,
        })?;

        let mut outer_archive =
            ZipArchive::new(file).map_err(|error| PortableArchiveReadError::ReadArchive {
                path: path.to_owned(),
                error,
            })?;

        // Read and parse the manifest.
        let manifest_bytes =
            read_outer_file(&mut outer_archive, PORTABLE_MANIFEST_FILE_NAME, path)?;
        let manifest: PortableManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                PortableArchiveReadError::ParseManifest {
                    path: path.to_owned(),
                    error,
                }
            })?;

        // Validate format version.
        if let Err(incompatibility) = manifest
            .format_version
            .check_readable_by(PORTABLE_ARCHIVE_FORMAT_VERSION)
        {
            return Err(PortableArchiveReadError::UnsupportedFormatVersion {
                path: path.to_owned(),
                found: manifest.format_version,
                supported: PORTABLE_ARCHIVE_FORMAT_VERSION,
                incompatibility,
            });
        }

        // Validate store format version.
        let store_version = manifest.store_format_version();
        if let Err(incompatibility) = store_version.check_readable_by(STORE_FORMAT_VERSION) {
            return Err(PortableArchiveReadError::UnsupportedStoreFormatVersion {
                path: path.to_owned(),
                found: store_version,
                supported: STORE_FORMAT_VERSION,
                incompatibility,
            });
        }

        Ok(Self {
            archive_path: path.to_owned(),
            manifest,
            outer_archive,
        })
    }

    /// Returns the path to the archive file.
    pub fn archive_path(&self) -> &Utf8Path {
        &self.archive_path
    }

    /// Returns run info extracted from the manifest.
    pub fn run_info(&self) -> RecordedRunInfo {
        self.manifest.run_info()
    }

    /// Reads the run log into memory and returns it as an owned struct.
    ///
    /// The returned [`PortableArchiveRunLog`] can be used to iterate over events
    /// independently of this archive, avoiding borrow conflicts with
    /// [`open_store`](Self::open_store).
    pub fn read_run_log(&mut self) -> Result<PortableArchiveRunLog, PortableArchiveReadError> {
        let run_log_bytes = read_outer_file(
            &mut self.outer_archive,
            RUN_LOG_FILE_NAME,
            &self.archive_path,
        )?;
        Ok(PortableArchiveRunLog {
            archive_path: self.archive_path.clone(),
            run_log_bytes,
        })
    }

    /// Extracts a file from the outer archive to a path, streaming directly.
    ///
    /// This avoids loading the entire file into memory. The full file is always
    /// extracted regardless of size.
    ///
    /// If `check_limit` is true, the result will indicate whether the file
    /// exceeded [`MAX_MAX_OUTPUT_SIZE`]. This is informational only and does
    /// not affect extraction.
    pub fn extract_outer_file_to_path(
        &mut self,
        file_name: &'static str,
        output_path: &Utf8Path,
        check_limit: bool,
    ) -> Result<ExtractOuterFileResult, PortableArchiveReadError> {
        extract_outer_file_to_path(
            &mut self.outer_archive,
            file_name,
            &self.archive_path,
            output_path,
            check_limit,
        )
    }

    /// Opens the inner store.zip for reading.
    ///
    /// The returned reader borrows from this archive and implements [`StoreReader`].
    pub fn open_store(&mut self) -> Result<PortableStoreReader<'_>, PortableArchiveReadError> {
        // Use by_name_seek to get a seekable handle to store.zip.
        let store_handle = self
            .outer_archive
            .by_name_seek(STORE_ZIP_FILE_NAME)
            .map_err(|error| match error {
                ZipError::FileNotFound => PortableArchiveReadError::MissingFile {
                    path: self.archive_path.clone(),
                    file_name: STORE_ZIP_FILE_NAME,
                },
                _ => PortableArchiveReadError::ReadArchive {
                    path: self.archive_path.clone(),
                    error,
                },
            })?;

        let store_archive = ZipArchive::new(store_handle).map_err(|error| {
            PortableArchiveReadError::ReadArchive {
                path: self.archive_path.clone(),
                error,
            }
        })?;

        Ok(PortableStoreReader {
            archive_path: &self.archive_path,
            store_archive,
            stdout_dict: None,
            stderr_dict: None,
        })
    }
}

/// Reads a file from the outer archive into memory, with size limits.
fn read_outer_file(
    archive: &mut ZipArchive<File>,
    file_name: &'static str,
    archive_path: &Utf8Path,
) -> Result<Vec<u8>, PortableArchiveReadError> {
    let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
    let file = archive.by_name(file_name).map_err(|error| match error {
        ZipError::FileNotFound => PortableArchiveReadError::MissingFile {
            path: archive_path.to_owned(),
            file_name,
        },
        _ => PortableArchiveReadError::ReadArchive {
            path: archive_path.to_owned(),
            error,
        },
    })?;

    let claimed_size = file.size();
    if claimed_size > limit {
        return Err(PortableArchiveReadError::FileTooLarge {
            path: archive_path.to_owned(),
            file_name,
            size: claimed_size,
            limit,
        });
    }

    let capacity = usize::try_from(claimed_size).unwrap_or(usize::MAX);
    let mut contents = Vec::with_capacity(capacity);

    file.take(limit)
        .read_to_end(&mut contents)
        .map_err(|error| PortableArchiveReadError::ReadArchive {
            path: archive_path.to_owned(),
            error: ZipError::Io(error),
        })?;

    Ok(contents)
}

/// Extracts a file from the outer archive to a path, streaming directly.
fn extract_outer_file_to_path(
    archive: &mut ZipArchive<File>,
    file_name: &'static str,
    archive_path: &Utf8Path,
    output_path: &Utf8Path,
    check_limit: bool,
) -> Result<ExtractOuterFileResult, PortableArchiveReadError> {
    let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
    let mut file = archive.by_name(file_name).map_err(|error| match error {
        ZipError::FileNotFound => PortableArchiveReadError::MissingFile {
            path: archive_path.to_owned(),
            file_name,
        },
        _ => PortableArchiveReadError::ReadArchive {
            path: archive_path.to_owned(),
            error,
        },
    })?;

    let claimed_size = file.size();
    let exceeded_limit = if check_limit && claimed_size > limit {
        Some(claimed_size)
    } else {
        None
    };

    let mut output_file =
        File::create(output_path).map_err(|error| PortableArchiveReadError::ExtractFile {
            archive_path: archive_path.to_owned(),
            file_name,
            output_path: output_path.to_owned(),
            error,
        })?;

    let bytes_written = io::copy(&mut file, &mut output_file).map_err(|error| {
        PortableArchiveReadError::ExtractFile {
            archive_path: archive_path.to_owned(),
            file_name,
            output_path: output_path.to_owned(),
            error,
        }
    })?;

    Ok(ExtractOuterFileResult {
        bytes_written,
        exceeded_limit,
    })
}

/// The run log from a portable archive, read into memory.
///
/// This struct owns the run log bytes and can create event iterators
/// independently of the [`PortableArchive`] it came from.
#[derive(Debug)]
pub struct PortableArchiveRunLog {
    archive_path: Utf8PathBuf,
    run_log_bytes: Vec<u8>,
}

impl PortableArchiveRunLog {
    /// Returns an iterator over events from the run log.
    pub fn events(&self) -> Result<PortableArchiveEventIter<'_>, RecordReadError> {
        // The run log is zstd-compressed JSON Lines. Use with_buffer since the
        // data is already in memory (no need for Decoder's internal BufReader).
        let decoder =
            zstd::stream::Decoder::with_buffer(&self.run_log_bytes[..]).map_err(|error| {
                RecordReadError::OpenRunLog {
                    path: self.archive_path.join(RUN_LOG_FILE_NAME),
                    error,
                }
            })?;
        Ok(PortableArchiveEventIter {
            // BufReader is still needed for read_line().
            reader: DebugIgnore(BufReader::new(decoder)),
            line_buf: String::new(),
            line_number: 0,
        })
    }
}

/// Iterator over events from a portable archive's run log.
#[derive(Debug)]
pub struct PortableArchiveEventIter<'a> {
    reader: DebugIgnore<BufReader<zstd::stream::Decoder<'static, &'a [u8]>>>,
    line_buf: String,
    line_number: usize,
}

impl Iterator for PortableArchiveEventIter<'_> {
    type Item = Result<TestEventSummary<ZipStoreOutput>, RecordReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.line_buf.clear();
            self.line_number += 1;

            match self.reader.read_line(&mut self.line_buf) {
                Ok(0) => return None,
                Ok(_) => {
                    let trimmed = self.line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_str(trimmed).map_err(|error| {
                        RecordReadError::ParseEvent {
                            line_number: self.line_number,
                            error,
                        }
                    }));
                }
                Err(error) => {
                    return Some(Err(RecordReadError::ReadRunLog {
                        line_number: self.line_number,
                        error,
                    }));
                }
            }
        }
    }
}

/// Reader for the inner store.zip within a portable archive.
///
/// Borrows from [`PortableArchive`] and implements [`StoreReader`].
pub struct PortableStoreReader<'a> {
    archive_path: &'a Utf8Path,
    store_archive: ZipArchive<ZipFileSeek<'a, File>>,
    /// Cached stdout dictionary loaded from the archive.
    stdout_dict: Option<Vec<u8>>,
    /// Cached stderr dictionary loaded from the archive.
    stderr_dict: Option<Vec<u8>>,
}

impl std::fmt::Debug for PortableStoreReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortableStoreReader")
            .field("archive_path", &self.archive_path)
            .field("stdout_dict", &self.stdout_dict.as_ref().map(|d| d.len()))
            .field("stderr_dict", &self.stderr_dict.as_ref().map(|d| d.len()))
            .finish_non_exhaustive()
    }
}

impl PortableStoreReader<'_> {
    /// Reads a file from the store archive as bytes, with size limit.
    fn read_store_file(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError> {
        let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
        let file = self.store_archive.by_name(file_name).map_err(|error| {
            RecordReadError::ReadArchiveFile {
                file_name: file_name.to_string(),
                error,
            }
        })?;

        let claimed_size = file.size();
        if claimed_size > limit {
            return Err(RecordReadError::FileTooLarge {
                file_name: file_name.to_string(),
                size: claimed_size,
                limit,
            });
        }

        let capacity = usize::try_from(claimed_size).unwrap_or(usize::MAX);
        let mut contents = Vec::with_capacity(capacity);

        file.take(limit)
            .read_to_end(&mut contents)
            .map_err(|error| RecordReadError::Decompress {
                file_name: file_name.to_string(),
                error,
            })?;

        let actual_size = contents.len() as u64;
        if actual_size != claimed_size {
            return Err(RecordReadError::SizeMismatch {
                file_name: file_name.to_string(),
                claimed_size,
                actual_size,
            });
        }

        Ok(contents)
    }

    /// Returns the dictionary bytes for the given output file name, if known.
    fn get_dict_for_output(&self, file_name: &str) -> Option<&[u8]> {
        match OutputDict::for_output_file_name(file_name) {
            OutputDict::Stdout => Some(
                self.stdout_dict
                    .as_ref()
                    .expect("load_dictionaries must be called first"),
            ),
            OutputDict::Stderr => Some(
                self.stderr_dict
                    .as_ref()
                    .expect("load_dictionaries must be called first"),
            ),
            OutputDict::None => None,
        }
    }
}

impl StoreReader for PortableStoreReader<'_> {
    fn read_cargo_metadata(&mut self) -> Result<String, RecordReadError> {
        let bytes = self.read_store_file(CARGO_METADATA_JSON_PATH)?;
        String::from_utf8(bytes).map_err(|e| RecordReadError::Decompress {
            file_name: CARGO_METADATA_JSON_PATH.to_string(),
            error: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })
    }

    fn read_test_list(&mut self) -> Result<TestListSummary, RecordReadError> {
        let bytes = self.read_store_file(TEST_LIST_JSON_PATH)?;
        serde_json::from_slice(&bytes).map_err(|error| RecordReadError::DeserializeMetadata {
            file_name: TEST_LIST_JSON_PATH.to_string(),
            error,
        })
    }

    fn read_record_opts(&mut self) -> Result<RecordOpts, RecordReadError> {
        let bytes = self.read_store_file(RECORD_OPTS_JSON_PATH)?;
        serde_json::from_slice(&bytes).map_err(|error| RecordReadError::DeserializeMetadata {
            file_name: RECORD_OPTS_JSON_PATH.to_string(),
            error,
        })
    }

    fn read_rerun_info(&mut self) -> Result<Option<RerunInfo>, RecordReadError> {
        match self.read_store_file(RERUN_INFO_JSON_PATH) {
            Ok(bytes) => {
                let info = serde_json::from_slice(&bytes).map_err(|error| {
                    RecordReadError::DeserializeMetadata {
                        file_name: RERUN_INFO_JSON_PATH.to_string(),
                        error,
                    }
                })?;
                Ok(Some(info))
            }
            Err(RecordReadError::ReadArchiveFile {
                error: ZipError::FileNotFound,
                ..
            }) => {
                // File doesn't exist; this is not a rerun.
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn load_dictionaries(&mut self) -> Result<(), RecordReadError> {
        self.stdout_dict = Some(self.read_store_file(STDOUT_DICT_PATH)?);
        self.stderr_dict = Some(self.read_store_file(STDERR_DICT_PATH)?);
        Ok(())
    }

    fn read_output(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError> {
        let path = format!("out/{file_name}");
        let compressed = self.read_store_file(&path)?;
        let limit = MAX_MAX_OUTPUT_SIZE.as_u64();

        let dict_bytes = self.get_dict_for_output(file_name).ok_or_else(|| {
            RecordReadError::UnknownOutputType {
                file_name: file_name.to_owned(),
            }
        })?;

        decompress_with_dict(&compressed, dict_bytes, limit).map_err(|error| {
            RecordReadError::Decompress {
                file_name: path,
                error,
            }
        })
    }

    fn extract_file_to_path(
        &mut self,
        store_path: &str,
        output_path: &Utf8Path,
    ) -> Result<u64, RecordReadError> {
        let mut file = self.store_archive.by_name(store_path).map_err(|error| {
            RecordReadError::ReadArchiveFile {
                file_name: store_path.to_owned(),
                error,
            }
        })?;

        let mut output_file =
            File::create(output_path).map_err(|error| RecordReadError::ExtractFile {
                store_path: store_path.to_owned(),
                output_path: output_path.to_owned(),
                error,
            })?;

        io::copy(&mut file, &mut output_file).map_err(|error| RecordReadError::ExtractFile {
            store_path: store_path.to_owned(),
            output_path: output_path.to_owned(),
            error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        format::{PORTABLE_ARCHIVE_FORMAT_VERSION, STORE_FORMAT_VERSION},
        store::{CompletedRunStats, RecordedRunStatus, RecordedSizes},
    };
    use camino_tempfile::Utf8TempDir;
    use chrono::Local;
    use quick_junit::ReportUuid;
    use semver::Version;
    use std::{collections::BTreeMap, io::Read};
    use zip::ZipArchive;

    fn create_test_run_dir(run_id: ReportUuid) -> (Utf8TempDir, Utf8PathBuf) {
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_dir = runs_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let store_path = run_dir.join(STORE_ZIP_FILE_NAME);
        let store_file = File::create(&store_path).expect("create store.zip");
        let mut zip_writer = ZipWriter::new(store_file);
        zip_writer
            .start_file("test.txt", SimpleFileOptions::default())
            .expect("start file");
        zip_writer
            .write_all(b"test content")
            .expect("write content");
        zip_writer.finish().expect("finish zip");

        let log_path = run_dir.join(RUN_LOG_FILE_NAME);
        let log_file = File::create(&log_path).expect("create run.log.zst");
        let mut encoder = zstd::stream::Encoder::new(log_file, 3).expect("create encoder");
        encoder.write_all(b"test log content").expect("write log");
        encoder.finish().expect("finish encoder");

        (temp_dir, runs_dir)
    }

    fn create_test_run_info(run_id: ReportUuid) -> RecordedRunInfo {
        let now = Local::now().fixed_offset();
        RecordedRunInfo {
            run_id,
            store_format_version: STORE_FORMAT_VERSION,
            nextest_version: Version::new(0, 9, 111),
            started_at: now,
            last_written_at: now,
            duration_secs: Some(12.345),
            cli_args: vec!["cargo".to_owned(), "nextest".to_owned(), "run".to_owned()],
            build_scope_args: vec!["--workspace".to_owned()],
            env_vars: BTreeMap::from([("CARGO_TERM_COLOR".to_owned(), "always".to_owned())]),
            parent_run_id: None,
            sizes: RecordedSizes::default(),
            status: RecordedRunStatus::Completed(CompletedRunStats {
                initial_run_count: 10,
                passed: 9,
                failed: 1,
                exit_code: 100,
            }),
        }
    }

    #[test]
    fn test_default_filename() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let (_temp_dir, runs_dir) = create_test_run_dir(run_id);
        let run_info = create_test_run_info(run_id);

        let writer = PortableArchiveWriter::new(&run_info, StoreRunsDir::new(&runs_dir))
            .expect("create writer");

        assert_eq!(
            writer.default_filename(),
            "nextest-run-550e8400-e29b-41d4-a716-446655440000.zip"
        );
    }

    #[test]
    fn test_write_portable_archive() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let (_temp_dir, runs_dir) = create_test_run_dir(run_id);
        let run_info = create_test_run_info(run_id);

        let writer = PortableArchiveWriter::new(&run_info, StoreRunsDir::new(&runs_dir))
            .expect("create writer");

        let output_dir = camino_tempfile::tempdir().expect("create output dir");

        let result = writer
            .write_to_dir(output_dir.path())
            .expect("write archive");

        assert!(result.path.exists());
        assert!(result.size > 0);

        // Verify that the reported size matches the actual file size on disk.
        let actual_size = std::fs::metadata(&result.path)
            .expect("get file metadata")
            .len();
        assert_eq!(
            result.size, actual_size,
            "reported size should match actual file size"
        );

        assert_eq!(
            result.path.file_name(),
            Some("nextest-run-550e8400-e29b-41d4-a716-446655440000.zip")
        );

        let archive_file = File::open(&result.path).expect("open archive");
        let mut archive = ZipArchive::new(archive_file).expect("read archive");

        assert_eq!(archive.len(), 3);

        {
            let mut manifest_file = archive
                .by_name(PORTABLE_MANIFEST_FILE_NAME)
                .expect("manifest");
            let mut manifest_content = String::new();
            manifest_file
                .read_to_string(&mut manifest_content)
                .expect("read manifest");
            let manifest: PortableManifest =
                serde_json::from_str(&manifest_content).expect("parse manifest");
            assert_eq!(manifest.format_version, PORTABLE_ARCHIVE_FORMAT_VERSION);
            assert_eq!(manifest.run.run_id, run_id);
        }

        {
            let store_file = archive.by_name(STORE_ZIP_FILE_NAME).expect("store.zip");
            assert!(store_file.size() > 0);
        }

        {
            let log_file = archive.by_name(RUN_LOG_FILE_NAME).expect("run.log.zst");
            assert!(log_file.size() > 0);
        }
    }

    #[test]
    fn test_missing_run_dir() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_info = create_test_run_info(run_id);

        let result = PortableArchiveWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(matches!(
            result,
            Err(PortableArchiveError::RunDirNotFound { .. })
        ));
    }

    #[test]
    fn test_missing_store_zip() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_dir = runs_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let log_path = run_dir.join(RUN_LOG_FILE_NAME);
        let log_file = File::create(&log_path).expect("create run.log.zst");
        let mut encoder = zstd::stream::Encoder::new(log_file, 3).expect("create encoder");
        encoder.write_all(b"test").expect("write");
        encoder.finish().expect("finish");

        let run_info = create_test_run_info(run_id);
        let result = PortableArchiveWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(
            matches!(
                &result,
                Err(PortableArchiveError::RequiredFileMissing { file_name, .. })
                if *file_name == STORE_ZIP_FILE_NAME
            ),
            "expected RequiredFileMissing for store.zip, got {result:?}"
        );
    }

    #[test]
    fn test_missing_run_log() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_dir = runs_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let store_path = run_dir.join(STORE_ZIP_FILE_NAME);
        let store_file = File::create(&store_path).expect("create store.zip");
        let mut zip_writer = ZipWriter::new(store_file);
        zip_writer
            .start_file("test.txt", SimpleFileOptions::default())
            .expect("start");
        zip_writer.write_all(b"test").expect("write");
        zip_writer.finish().expect("finish");

        let run_info = create_test_run_info(run_id);
        let result = PortableArchiveWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(
            matches!(
                &result,
                Err(PortableArchiveError::RequiredFileMissing { file_name, .. })
                if *file_name == RUN_LOG_FILE_NAME
            ),
            "expected RequiredFileMissing for run.log.zst, got {result:?}"
        );
    }
}
