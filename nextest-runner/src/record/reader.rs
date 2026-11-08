// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading logic for recorded test runs.
//!
//! The [`RecordReader`] reads a recorded test run from disk, providing access
//! to metadata and events stored during the run.
//!
//! The [`StoreReader`] trait provides a unified interface for reading from
//! either on-disk stores or portable archives.

use super::{
    format::{
        BUILD_SCOPE_JSON_PATH, CARGO_METADATA_JSON_PATH, OutputDict, RECORD_OPTS_JSON_PATH,
        RUN_LOG_FILE_NAME, STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_ZIP_FILE_NAME,
        TEST_LIST_JSON_PATH,
    },
    summary::{RecordOpts, TestEventSummary},
};
use crate::{
    errors::RecordReadError,
    output_spec::RecordingSpec,
    record::format::{RERUN_INFO_JSON_PATH, RecordedBuildScope, RerunInfo},
    redact::SizeDisplay,
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
use bytesize::ByteSize;
use camino::{Utf8Path, Utf8PathBuf};
use debug_ignore::DebugIgnore;
use eazip::Archive;
use nextest_metadata::TestListSummary;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
};

/// Trait for reading from a recorded run's store.
///
/// This trait abstracts over reading from either an on-disk store directory
/// (via [`RecordReader`]) or from an inner store.zip within a portable archive
/// (via [`PortableStoreReader`](super::portable::PortableStoreReader)).
pub trait StoreReader {
    /// Returns the cargo metadata JSON from the store.
    fn read_cargo_metadata(&mut self) -> Result<String, RecordReadError>;

    /// Returns the test list summary from the store.
    fn read_test_list(&mut self) -> Result<TestListSummary, RecordReadError>;

    /// Returns the record options from the store.
    fn read_record_opts(&mut self) -> Result<RecordOpts, RecordReadError>;

    /// Returns the build scope for this run from the store.
    fn read_build_scope(&mut self) -> Result<RecordedBuildScope, RecordReadError>;

    /// Returns the rerun info from the store, if this is a rerun.
    ///
    /// Returns `Ok(None)` if this run is not a rerun (the file doesn't exist).
    fn read_rerun_info(&mut self) -> Result<Option<RerunInfo>, RecordReadError>;

    /// Loads the dictionaries from the store.
    ///
    /// This must be called before reading output files.
    fn load_dictionaries(&mut self) -> Result<(), RecordReadError>;

    /// Reads output for a specific file from the store.
    ///
    /// The `file_name` should be the value from `ZipStoreOutput::file_name`,
    /// e.g., "test-abc123-1-stdout".
    ///
    /// # Panics
    ///
    /// Panics if [`load_dictionaries`](Self::load_dictionaries) has not been called first.
    fn read_output(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError>;

    /// Extracts a file from the store to a path, streaming directly.
    ///
    /// The `store_path` is relative to the store root (e.g., `meta/test-list.json`).
    /// Returns the number of bytes written.
    fn extract_file_to_path(
        &mut self,
        store_path: &str,
        output_path: &Utf8Path,
    ) -> Result<u64, RecordReadError>;
}

/// Reader for a recorded test run.
///
/// Provides access to the metadata and events stored during a test run.
/// The archive is opened lazily when methods are called.
#[derive(Debug)]
pub struct RecordReader {
    run_dir: Utf8PathBuf,
    archive: Option<DebugIgnore<Archive<BufReader<File>>>>,
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
    fn ensure_archive(&mut self) -> Result<&mut Archive<BufReader<File>>, RecordReadError> {
        if self.archive.is_none() {
            let store_path = self.run_dir.join(STORE_ZIP_FILE_NAME);
            let file = File::open(&store_path).map_err(|error| RecordReadError::OpenArchive {
                path: store_path.clone(),
                error,
            })?;
            let archive = Archive::new(BufReader::new(file)).map_err(|error| {
                RecordReadError::ParseArchive {
                    path: store_path,
                    error,
                }
            })?;
            self.archive = Some(DebugIgnore(archive));
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
        let mut file =
            archive
                .get_by_name(file_name)
                .ok_or_else(|| RecordReadError::FileNotFound {
                    file_name: file_name.to_string(),
                })?;

        let claimed_size = file.metadata().uncompressed_size;
        if claimed_size > limit {
            return Err(RecordReadError::FileTooLarge {
                file_name: file_name.to_string(),
                size: claimed_size,
                limit,
            });
        }

        let capacity = usize::try_from(claimed_size).unwrap_or(usize::MAX);
        let mut contents = Vec::with_capacity(capacity);

        // file.read() returns a reader that decompresses and verifies CRC32 +
        // size. The take(limit) is a safety belt against spoofed headers.
        file.read()
            .and_then(|reader| reader.take(limit).read_to_end(&mut contents))
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

    /// Returns the build scope from the archive.
    pub fn read_build_scope(&mut self) -> Result<RecordedBuildScope, RecordReadError> {
        let bytes = self.read_archive_file(BUILD_SCOPE_JSON_PATH)?;
        serde_json::from_slice(&bytes).map_err(|error| RecordReadError::DeserializeMetadata {
            file_name: BUILD_SCOPE_JSON_PATH.to_string(),
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
            Err(RecordReadError::FileNotFound { .. }) => {
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
    pub fn events(&self) -> Result<RunLogEventIter<BufReader<LogDecoder>>, RecordReadError> {
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
        Ok(RunLogEventIter::new(BufReader::new(decoder)))
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

impl StoreReader for RecordReader {
    fn read_cargo_metadata(&mut self) -> Result<String, RecordReadError> {
        RecordReader::read_cargo_metadata(self)
    }

    fn read_test_list(&mut self) -> Result<TestListSummary, RecordReadError> {
        RecordReader::read_test_list(self)
    }

    fn read_record_opts(&mut self) -> Result<RecordOpts, RecordReadError> {
        RecordReader::read_record_opts(self)
    }

    fn read_build_scope(&mut self) -> Result<RecordedBuildScope, RecordReadError> {
        RecordReader::read_build_scope(self)
    }

    fn read_rerun_info(&mut self) -> Result<Option<RerunInfo>, RecordReadError> {
        RecordReader::read_rerun_info(self)
    }

    fn load_dictionaries(&mut self) -> Result<(), RecordReadError> {
        RecordReader::load_dictionaries(self)
    }

    fn read_output(&mut self, file_name: &str) -> Result<Vec<u8>, RecordReadError> {
        RecordReader::read_output(self, file_name)
    }

    fn extract_file_to_path(
        &mut self,
        store_path: &str,
        output_path: &Utf8Path,
    ) -> Result<u64, RecordReadError> {
        let archive = self.ensure_archive()?;
        let mut file =
            archive
                .get_by_name(store_path)
                .ok_or_else(|| RecordReadError::FileNotFound {
                    file_name: store_path.to_owned(),
                })?;

        let mut output_file =
            File::create(output_path).map_err(|error| RecordReadError::ExtractFile {
                store_path: store_path.to_owned(),
                output_path: output_path.to_owned(),
                error,
            })?;

        // file.read() decompresses and verifies CRC32 + size.
        let mut reader = file
            .read()
            .map_err(|error| RecordReadError::ReadArchiveFile {
                file_name: store_path.to_owned(),
                error,
            })?;

        io::copy(&mut reader, &mut output_file).map_err(|error| RecordReadError::ExtractFile {
            store_path: store_path.to_owned(),
            output_path: output_path.to_owned(),
            error,
        })
    }
}

/// Decompresses data using a pre-trained zstd dictionary, with a size limit.
///
/// The limit prevents compression bombs where a small compressed payload
/// expands to an extremely large decompressed output.
pub(super) fn decompress_with_dict(
    compressed: &[u8],
    dict_bytes: &[u8],
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let dict = zstd::dict::DecoderDictionary::copy(dict_bytes);
    let decoder = zstd::stream::Decoder::with_prepared_dictionary(compressed, &dict)?;
    let mut decompressed = Vec::new();
    decoder
        .take(limit.saturating_add(1))
        .read_to_end(&mut decompressed)?;
    if decompressed.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "decompressed size exceeds the limit of {}",
                SizeDisplay(limit)
            ),
        ));
    }
    Ok(decompressed)
}

/// Zstd decoder reading from a file.
type LogDecoder = zstd::stream::Decoder<'static, BufReader<File>>;

/// The maximum length of a run log line.
///
/// This is at least as large as any lin nextest writes today. In particular,
/// event lines embed failure text, so this matches the archive size limit.
const MAX_RUN_LOG_LINE_SIZE: ByteSize = MAX_MAX_OUTPUT_SIZE;

/// An iterator over events in a zstd-compressed JSON Lines run log.
///
/// This is shared by the store and portable readers.
#[derive(Debug)]
pub struct RunLogEventIter<R> {
    reader: DebugIgnore<R>,
    line_buf: Vec<u8>,
    line_number: usize,
    limit: ByteSize,
}

impl<R: BufRead> RunLogEventIter<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader: DebugIgnore(reader),
            line_buf: Vec::new(),
            line_number: 0,
            limit: MAX_RUN_LOG_LINE_SIZE,
        }
    }
}

impl<R: BufRead> Iterator for RunLogEventIter<R> {
    type Item = Result<TestEventSummary<RecordingSpec>, RecordReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        let limit = self.limit.as_u64();
        loop {
            self.line_buf.clear();
            self.line_number += 1;

            // Reading at most limit + 1 bytes bounds memory per line.
            match self
                .reader
                .by_ref()
                .take(limit.saturating_add(1))
                .read_until(b'\n', &mut self.line_buf)
            {
                Ok(0) => return None,
                Ok(_) => {
                    let line = self.line_buf.strip_suffix(b"\n").unwrap_or(&self.line_buf);
                    if line.len() as u64 > limit {
                        return Some(Err(RecordReadError::RunLogLineTooLong {
                            line_number: self.line_number,
                            limit: self.limit,
                        }));
                    }
                    let line = line.trim_ascii();
                    if line.is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_slice(line).map_err(|error| {
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
    use crate::record::recorder::compress_with_dict;

    #[test]
    fn test_run_log_line_limit() {
        #[derive(Debug, PartialEq)]
        enum LineResult {
            End,
            TooLong { line_number: usize },
            ParseError { line_number: usize },
        }

        let limit = ByteSize::b(8);
        let cases: [(&str, &[u8], LineResult); 8] = [
            ("empty input", b"", LineResult::End),
            ("only blank lines", b"\n\n  \n", LineResult::End),
            (
                "line at limit",
                b"12345678\n",
                LineResult::ParseError { line_number: 1 },
            ),
            (
                "line at limit without newline",
                b"12345678",
                LineResult::ParseError { line_number: 1 },
            ),
            (
                "line over limit",
                b"123456789\n",
                LineResult::TooLong { line_number: 1 },
            ),
            (
                "line over limit without newline",
                b"123456789",
                LineResult::TooLong { line_number: 1 },
            ),
            (
                "line over limit after a blank line",
                b"\n123456789\n",
                LineResult::TooLong { line_number: 2 },
            ),
            (
                "multibyte character crossing the limit",
                "1234567\u{e9}\n".as_bytes(),
                LineResult::TooLong { line_number: 1 },
            ),
        ];

        for (description, input, expected) in cases {
            let mut iter = RunLogEventIter {
                reader: DebugIgnore(io::Cursor::new(input)),
                line_buf: Vec::new(),
                line_number: 0,
                limit,
            };
            let actual = match iter.next() {
                None => LineResult::End,
                Some(Err(RecordReadError::RunLogLineTooLong {
                    line_number,
                    limit: error_limit,
                })) => {
                    assert_eq!(error_limit, limit, "{description}: error limit");
                    LineResult::TooLong { line_number }
                }
                Some(Err(RecordReadError::ParseEvent { line_number, .. })) => {
                    LineResult::ParseError { line_number }
                }
                Some(other) => panic!("{description}: unexpected result {other:?}"),
            };
            assert_eq!(actual, expected, "{description}");
            assert!(
                iter.line_buf.len() as u64 <= limit.as_u64() + 1,
                "{description}: buffered {} bytes, more than limit + 1",
                iter.line_buf.len()
            );
        }
    }

    #[test]
    fn test_decompress_with_dict_limit() {
        let limit = 64;
        let dict = OutputDict::Stdout
            .dict_bytes()
            .expect("stdout dictionary exists");
        for (len, within_limit) in [
            (0, true),
            (63, true),
            (64, true),
            (65, false),
            (4096, false),
        ] {
            let data = vec![b'x'; len];
            let compressed = compress_with_dict(&data, dict).expect("compressed data");
            let result = decompress_with_dict(&compressed, dict, limit);
            if within_limit {
                assert_eq!(
                    result.expect("decompression within the limit succeeds"),
                    data,
                    "len {len}: round-trips"
                );
            } else {
                let error = result.expect_err("decompression over the limit fails");
                assert_eq!(
                    error.kind(),
                    io::ErrorKind::InvalidData,
                    "len {len}: error kind"
                );
            }
        }
    }

    #[test]
    fn test_record_reader_nonexistent_dir() {
        let result = RecordReader::open(Utf8Path::new("/nonexistent/path"));
        assert!(matches!(result, Err(RecordReadError::RunNotFound { .. })));
    }
}
