// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading logic for recorded test runs.
//!
//! The [`RecordReader`] reads a recorded test run from disk, providing access
//! to metadata and events stored during the run.

use super::{
    format::{
        CARGO_METADATA_JSON_PATH, OutputDict, RECORD_OPTS_JSON_PATH, RUN_LOG_FILE_NAME,
        STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_ZIP_FILE_NAME, TEST_LIST_JSON_PATH,
    },
    summary::{RecordOpts, TestEventSummary, ZipStoreOutput},
};
use crate::{
    errors::RecordReadError,
    record::format::{RERUN_INFO_JSON_PATH, RerunInfo},
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
use camino::{Utf8Path, Utf8PathBuf};
use debug_ignore::DebugIgnore;
use nextest_metadata::TestListSummary;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
};
use zip::{ZipArchive, result::ZipError};

/// Reader for a recorded test run.
///
/// Provides access to the metadata and events stored during a test run.
/// The archive is opened lazily when methods are called.
#[derive(Debug)]
pub struct RecordReader {
    run_dir: Utf8PathBuf,
    archive: Option<ZipArchive<File>>,
    /// Cached stdout dictionary loaded from the archive.
    stdout_dict: Option<Vec<u8>>,
    /// Cached stderr dictionary loaded from the archive.
    stderr_dict: Option<Vec<u8>>,
}

impl RecordReader {
    /// Opens a recorded run from its directory.
    ///
    /// The directory should contain `store.zip` and `run.log.zst`.
    pub fn open(run_dir: &Utf8Path) -> Result<Self, RecordReadError> {
        if !run_dir.exists() {
            return Err(RecordReadError::RunNotFound {
                path: run_dir.to_owned(),
            });
        }

        Ok(Self {
            run_dir: run_dir.to_owned(),
            archive: None,
            stdout_dict: None,
            stderr_dict: None,
        })
    }

    /// Returns the path to the run directory.
    pub fn run_dir(&self) -> &Utf8Path {
        &self.run_dir
    }

    /// Opens the zip archive if not already open.
    fn ensure_archive(&mut self) -> Result<&mut ZipArchive<File>, RecordReadError> {
        if self.archive.is_none() {
            let store_path = self.run_dir.join(STORE_ZIP_FILE_NAME);
            let file = File::open(&store_path).map_err(|error| RecordReadError::OpenArchive {
                path: store_path,
                error,
            })?;
            let archive =
                ZipArchive::new(file).map_err(|error| RecordReadError::ReadArchiveFile {
                    file_name: STORE_ZIP_FILE_NAME.to_string(),
                    error,
                })?;
            self.archive = Some(archive);
        }
        Ok(self.archive.as_mut().expect("archive was just set"))
    }

    /// Reads a file from the archive as bytes, with size limit.
    ///
    /// The size limit prevents malicious archives from causing OOM by
    /// specifying a huge decompressed size. The limit is checked against the
    /// claimed size in the ZIP header, and `take()` is used during decompression
    /// to guard against spoofed headers.
    ///
    /// Since nextest controls archive creation, any mismatch between the header
    /// size and actual size indicates corruption or tampering.
    fn read_archive_file(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError> {
        let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
        let archive = self.ensure_archive()?;
        let file =
            archive
                .by_name(file_name)
                .map_err(|error| RecordReadError::ReadArchiveFile {
                    file_name: file_name.to_string(),
                    error,
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

    /// Returns the cargo metadata JSON from the archive.
    pub fn read_cargo_metadata(&mut self) -> Result<String, RecordReadError> {
        let bytes = self.read_archive_file(CARGO_METADATA_JSON_PATH)?;
        String::from_utf8(bytes).map_err(|e| RecordReadError::Decompress {
            file_name: CARGO_METADATA_JSON_PATH.to_string(),
            error: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })
    }

    /// Returns the test list from the archive.
    pub fn read_test_list(&mut self) -> Result<TestListSummary, RecordReadError> {
        let bytes = self.read_archive_file(TEST_LIST_JSON_PATH)?;
        serde_json::from_slice(&bytes).map_err(|error| RecordReadError::DeserializeMetadata {
            file_name: TEST_LIST_JSON_PATH.to_string(),
            error,
        })
    }

    /// Returns the record options from the archive.
    pub fn read_record_opts(&mut self) -> Result<RecordOpts, RecordReadError> {
        let bytes = self.read_archive_file(RECORD_OPTS_JSON_PATH)?;
        serde_json::from_slice(&bytes).map_err(|error| RecordReadError::DeserializeMetadata {
            file_name: RECORD_OPTS_JSON_PATH.to_string(),
            error,
        })
    }

    /// Returns the rerun info from the archive, if this is a rerun.
    ///
    /// Returns `Ok(None)` if this run is not a rerun (the file doesn't exist).
    /// Returns `Err` if the file exists but cannot be read or parsed.
    pub fn read_rerun_info(&mut self) -> Result<Option<RerunInfo>, RecordReadError> {
        match self.read_archive_file(RERUN_INFO_JSON_PATH) {
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

    /// Loads the dictionaries from the archive.
    ///
    /// This must be called before reading output files. The dictionaries are
    /// used for decompressing test output.
    ///
    /// Note: The store format version is checked before opening the archive,
    /// using the `store_format_version` field in runs.json.zst. This method
    /// assumes the version has already been validated.
    pub fn load_dictionaries(&mut self) -> Result<(), RecordReadError> {
        self.stdout_dict = Some(self.read_archive_file(STDOUT_DICT_PATH)?);
        self.stderr_dict = Some(self.read_archive_file(STDERR_DICT_PATH)?);
        Ok(())
    }

    /// Returns an iterator over events in the run log.
    ///
    /// Events are read one at a time from the zstd-compressed JSON Lines file.
    pub fn events(&self) -> Result<RecordEventIter, RecordReadError> {
        let log_path = self.run_dir.join(RUN_LOG_FILE_NAME);
        let file = File::open(&log_path).map_err(|error| RecordReadError::OpenRunLog {
            path: log_path.clone(),
            error,
        })?;
        let decoder =
            zstd::stream::Decoder::new(file).map_err(|error| RecordReadError::OpenRunLog {
                path: log_path,
                error,
            })?;
        Ok(RecordEventIter {
            reader: DebugIgnore(BufReader::new(decoder)),
            line_buf: String::new(),
            line_number: 0,
        })
    }

    /// Reads output for a specific file from the archive.
    ///
    /// The `file_name` should be the value from `ZipStoreOutput::file_name`,
    /// e.g., "test-abc123-1-stdout".
    ///
    /// The [`OutputFileName`](crate::record::OutputFileName) type ensures that
    /// file names are validated during deserialization, preventing path traversal.
    ///
    /// # Panics
    ///
    /// Panics if [`load_dictionaries`](Self::load_dictionaries) has not been called first.
    pub fn read_output(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError> {
        let path = format!("out/{file_name}");
        let compressed = self.read_archive_file(&path)?;
        let limit = MAX_MAX_OUTPUT_SIZE.as_u64();

        // Output files are stored pre-compressed with zstd dictionaries.
        // Unknown file types indicate a format revision that should have been
        // rejected during version validation.
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

    /// Returns the dictionary bytes for the given output file name, if known.
    ///
    /// Returns `None` for unknown file types, which indicates a format revision
    /// that should have been rejected during version validation.
    ///
    /// # Panics
    ///
    /// Panics if [`load_dictionaries`](Self::load_dictionaries) has not been called first.
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

/// Decompresses data using a pre-trained zstd dictionary, with a size limit.
///
/// The limit prevents compression bombs where a small compressed payload
/// expands to an extremely large decompressed output.
fn decompress_with_dict(
    compressed: &[u8],
    dict_bytes: &[u8],
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let dict = zstd::dict::DecoderDictionary::copy(dict_bytes);
    let decoder = zstd::stream::Decoder::with_prepared_dictionary(compressed, &dict)?;
    let mut decompressed = Vec::new();
    decoder.take(limit).read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

/// Zstd decoder reading from a file.
type LogDecoder = zstd::stream::Decoder<'static, BufReader<File>>;

/// Iterator over recorded events.
///
/// Reads events one at a time from the zstd-compressed JSON Lines run log.
#[derive(Debug)]
pub struct RecordEventIter {
    reader: DebugIgnore<BufReader<LogDecoder>>,
    line_buf: String,
    line_number: usize,
}

impl Iterator for RecordEventIter {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_reader_nonexistent_dir() {
        let result = RecordReader::open(Utf8Path::new("/nonexistent/path"));
        assert!(matches!(result, Err(RecordReadError::RunNotFound { .. })));
    }
}
