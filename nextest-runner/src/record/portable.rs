// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Portable archive creation and reading for recorded runs.
//!
//! A portable recording packages a single recorded run into a self-contained zip
//! file that can be shared and imported elsewhere.
//!
//! # Reading portable recordings
//!
//! Use [`PortableRecording::open`] to open a portable recording for reading. The
//! archive contains:
//!
//! - A manifest (`manifest.json`) with run metadata.
//! - A run log (`run.log.zst`) with test events.
//! - An inner store (`store.zip`) with metadata and test output.
//!
//! To read from the inner store, call [`PortableRecording::open_store`] to get a
//! [`PortableStoreReader`] that implements [`StoreReader`](super::reader::StoreReader).

use super::{
    format::{
        CARGO_METADATA_JSON_PATH, OutputDict, PORTABLE_MANIFEST_FILE_NAME,
        PORTABLE_RECORDING_FORMAT_VERSION, PortableManifest, RECORD_OPTS_JSON_PATH,
        RERUN_INFO_JSON_PATH, RUN_LOG_FILE_NAME, RerunInfo, STDERR_DICT_PATH, STDOUT_DICT_PATH,
        STORE_FORMAT_VERSION, STORE_ZIP_FILE_NAME, TEST_LIST_JSON_PATH, has_zip_extension,
        stored_file_options,
    },
    reader::{StoreReader, decompress_with_dict},
    store::{RecordedRunInfo, RunFilesExist, StoreRunsDir},
    summary::{RecordOpts, TestEventSummary},
};
use crate::{
    errors::{PortableRecordingError, PortableRecordingReadError, RecordReadError},
    output_spec::RecordingSpec,
    user_config::elements::MAX_MAX_OUTPUT_SIZE,
};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use bytesize::ByteSize;
use camino::{Utf8Path, Utf8PathBuf};
use countio::Counter;
use debug_ignore::DebugIgnore;
use eazip::{Archive, ArchiveWriter, CompressionMethod};
use itertools::Either;
use nextest_metadata::TestListSummary;
use std::{
    borrow::Cow,
    fs::File,
    io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write},
};

/// Result of writing a portable recording.
#[derive(Debug)]
pub struct PortableRecordingResult {
    /// The path to the written archive.
    pub path: Utf8PathBuf,
    /// The total size of the archive in bytes.
    pub size: u64,
}

/// Result of extracting a file from a portable recording.
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

/// Writer to create a portable recording from a recorded run.
#[derive(Debug)]
pub struct PortableRecordingWriter<'a> {
    run_info: &'a RecordedRunInfo,
    run_dir: Utf8PathBuf,
}

impl<'a> PortableRecordingWriter<'a> {
    /// Creates a new writer for the given run.
    ///
    /// Validates that the run directory exists and contains the required files.
    pub fn new(
        run_info: &'a RecordedRunInfo,
        runs_dir: StoreRunsDir<'_>,
    ) -> Result<Self, PortableRecordingError> {
        let run_dir = runs_dir.run_dir(run_info.run_id);

        if !run_dir.exists() {
            return Err(PortableRecordingError::RunDirNotFound { path: run_dir });
        }

        let store_zip_path = run_dir.join(STORE_ZIP_FILE_NAME);
        if !store_zip_path.exists() {
            return Err(PortableRecordingError::RequiredFileMissing {
                run_dir,
                file_name: STORE_ZIP_FILE_NAME,
            });
        }

        let run_log_path = run_dir.join(RUN_LOG_FILE_NAME);
        if !run_log_path.exists() {
            return Err(PortableRecordingError::RequiredFileMissing {
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

    /// Writes the portable recording to the given directory.
    ///
    /// The archive is written atomically using a temporary file and rename.
    /// The filename will be the default filename (`nextest-run-{run_id}.zip`).
    pub fn write_to_dir(
        &self,
        output_dir: &Utf8Path,
    ) -> Result<PortableRecordingResult, PortableRecordingError> {
        let output_path = output_dir.join(self.default_filename());
        self.write_to_path(&output_path)
    }

    /// Writes the portable recording to the given path.
    ///
    /// The archive is written atomically using a temporary file and rename.
    pub fn write_to_path(
        &self,
        output_path: &Utf8Path,
    ) -> Result<PortableRecordingResult, PortableRecordingError> {
        let atomic_file = AtomicFile::new(output_path, OverwriteBehavior::AllowOverwrite);

        let final_size = atomic_file
            .write(|temp_file| {
                let counter = Counter::new(temp_file);
                let mut zip_writer = ArchiveWriter::new(counter);

                self.write_manifest(&mut zip_writer)?;
                self.copy_file(&mut zip_writer, RUN_LOG_FILE_NAME)?;
                self.copy_file(&mut zip_writer, STORE_ZIP_FILE_NAME)?;

                let counter = zip_writer
                    .finish()
                    .map_err(PortableRecordingError::ZipFinalize)?;

                // Prefer the actual file size from metadata since ArchiveWriter
                // writes data descriptors after entries, causing the counter to
                // slightly overcount. Fall back to the counter value if metadata
                // is unavailable.
                let counter_bytes = counter.writer_bytes() as u64;
                let file = counter.into_inner();
                let size = file.metadata().map(|m| m.len()).unwrap_or(counter_bytes);

                Ok(size)
            })
            .map_err(|err| match err {
                atomicwrites::Error::Internal(source) => PortableRecordingError::AtomicWrite {
                    path: output_path.to_owned(),
                    source,
                },
                atomicwrites::Error::User(e) => e,
            })?;

        Ok(PortableRecordingResult {
            path: output_path.to_owned(),
            size: final_size,
        })
    }

    /// Writes the manifest to the archive.
    fn write_manifest<W: Write>(
        &self,
        zip_writer: &mut ArchiveWriter<W>,
    ) -> Result<(), PortableRecordingError> {
        let manifest = PortableManifest::new(self.run_info);
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(PortableRecordingError::SerializeManifest)?;

        let options = stored_file_options();

        zip_writer
            .add_file(PORTABLE_MANIFEST_FILE_NAME, &manifest_json[..], &options)
            .map_err(|source| PortableRecordingError::ZipWrite {
                file_name: PORTABLE_MANIFEST_FILE_NAME,
                source,
            })?;

        Ok(())
    }

    /// Copies a file from the run directory to the archive.
    ///
    /// The file is stored without additional compression since `run.log.zst`
    /// and `store.zip` are already compressed.
    fn copy_file<W: Write>(
        &self,
        zip_writer: &mut ArchiveWriter<W>,
        file_name: &'static str,
    ) -> Result<(), PortableRecordingError> {
        let source_path = self.run_dir.join(file_name);
        let mut file = File::open(&source_path)
            .map_err(|source| PortableRecordingError::ReadFile { file_name, source })?;

        let options = stored_file_options();

        let mut streamer = zip_writer
            .stream_file(file_name, &options)
            .map_err(|source| PortableRecordingError::ZipStartFile { file_name, source })?;

        io::copy(&mut file, &mut streamer)
            .map_err(|source| PortableRecordingError::ZipWrite { file_name, source })?;

        streamer
            .finish()
            .map_err(|source| PortableRecordingError::ZipWrite { file_name, source })?;

        Ok(())
    }
}

// ---
// Portable recording reading
// ---

/// Maximum size for spooling a non-seekable input to a temporary file (4 GiB).
///
/// This is a safety limit to avoid filling up disk when reading from a pipe.
/// Portable recordings are typically small (a few hundred MB at most), so this
/// is generous.
const SPOOL_SIZE_LIMIT: ByteSize = ByteSize(4 * 1024 * 1024 * 1024);

/// Classifies a Windows file handle for seekability.
///
/// On Windows, `SetFilePointerEx` can spuriously succeed on named pipe handles
/// (returning meaningless position values), so seek-based probing is
/// unreliable. We use `GetFileType` instead, which definitively classifies the
/// handle.
#[cfg(windows)]
enum WindowsFileKind {
    /// A regular disk file (seekable).
    Disk,
    /// A pipe, FIFO, or socket (not seekable, must be spooled).
    Pipe,
    /// A character device or unknown handle type (not expected for recording
    /// files).
    Other(u32),
}

/// Classifies a Windows file handle using `GetFileType`.
#[cfg(windows)]
fn classify_windows_handle(file: &File) -> WindowsFileKind {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};

    // SAFETY: the handle is valid because `file` is a live `File`.
    let file_type = unsafe { GetFileType(file.as_raw_handle()) };
    match file_type {
        FILE_TYPE_DISK => WindowsFileKind::Disk,
        FILE_TYPE_PIPE => WindowsFileKind::Pipe,
        other => WindowsFileKind::Other(other),
    }
}

/// Returns true if the I/O error indicates that the file descriptor does not
/// support seeking (i.e. it is a pipe, FIFO, or socket).
#[cfg(unix)]
fn is_not_seekable_error(e: &io::Error) -> bool {
    // Pipes/FIFOs/sockets fail lseek with ESPIPE.
    e.raw_os_error() == Some(libc::ESPIPE)
}

/// Ensures that a file is seekable, spooling to a temp file if necessary.
///
/// Process substitution paths (e.g. `/proc/self/fd/11` from `<(curl url)`)
/// produce pipe fds that are not seekable. ZIP reading requires seeking, so we
/// spool the pipe contents to an anonymous temporary file first.
///
/// Returns the original file if it's already seekable, or a new temp file
/// containing the spooled data.
fn ensure_seekable(file: File, path: &Utf8Path) -> Result<File, PortableRecordingReadError> {
    ensure_seekable_impl(file, path, SPOOL_SIZE_LIMIT)
}

/// Inner implementation of [`ensure_seekable`] with a configurable size limit.
///
/// Separated so tests can exercise the limit enforcement without writing 4 GiB.
fn ensure_seekable_impl(
    file: File,
    path: &Utf8Path,
    spool_limit: ByteSize,
) -> Result<File, PortableRecordingReadError> {
    // On Unix, lseek reliably fails with ESPIPE on pipes/FIFOs/sockets, so
    // a seek probe is sufficient.
    #[cfg(unix)]
    {
        let mut file = file;
        match file.stream_position() {
            Ok(_) => Ok(file),
            Err(e) if is_not_seekable_error(&e) => spool_to_temp(file, path, spool_limit),
            Err(e) => {
                // Unexpected seek error (e.g. EBADF, EIO): propagate rather than
                // silently falling into the spool path.
                Err(PortableRecordingReadError::SeekProbe {
                    path: path.to_owned(),
                    error: e,
                })
            }
        }
    }

    // On Windows, SetFilePointerEx can spuriously succeed on named pipe
    // handles, so seek-based probing is unreliable. Use GetFileType to
    // definitively classify the handle.
    #[cfg(windows)]
    match classify_windows_handle(&file) {
        WindowsFileKind::Disk => Ok(file),
        WindowsFileKind::Pipe => spool_to_temp(file, path, spool_limit),
        WindowsFileKind::Other(file_type) => Err(PortableRecordingReadError::SeekProbe {
            path: path.to_owned(),
            error: io::Error::other(format!(
                "unexpected file handle type {file_type:#x} (expected disk or pipe)"
            )),
        }),
    }
}

/// Spools the contents of a non-seekable file to an anonymous temporary file.
///
/// Returns the temp file, rewound to the beginning so callers can read it.
fn spool_to_temp(
    file: File,
    path: &Utf8Path,
    spool_limit: ByteSize,
) -> Result<File, PortableRecordingReadError> {
    let mut temp =
        camino_tempfile::tempfile().map_err(|error| PortableRecordingReadError::SpoolTempFile {
            path: path.to_owned(),
            error,
        })?;

    // Read up to spool_limit + 1 bytes. If we get more than the limit, the
    // input is too large. Use saturating_add to avoid wrapping if the limit
    // is u64::MAX (not an issue in practice since the limit is 4 GiB).
    let bytes_copied = io::copy(
        &mut (&file).take(spool_limit.0.saturating_add(1)),
        &mut temp,
    )
    .map_err(|error| PortableRecordingReadError::SpoolTempFile {
        path: path.to_owned(),
        error,
    })?;

    if bytes_copied > spool_limit.0 {
        return Err(PortableRecordingReadError::SpoolTooLarge {
            path: path.to_owned(),
            limit: spool_limit,
        });
    }

    // Rewind so the archive reader can read from the beginning.
    temp.seek(SeekFrom::Start(0))
        .map_err(|error| PortableRecordingReadError::SpoolTempFile {
            path: path.to_owned(),
            error,
        })?;

    Ok(temp)
}

/// Backing storage for an archive.
///
/// - `Left(File)`: Direct file-backed archive (normal case).
/// - `Right(Cursor<Vec<u8>>)`: Memory-backed archive (unwrapped from a wrapper zip).
type ArchiveReadStorage = Either<File, Cursor<Vec<u8>>>;

/// A portable recording opened for reading.
pub struct PortableRecording {
    archive_path: Utf8PathBuf,
    manifest: PortableManifest,
    outer_archive: Archive<BufReader<ArchiveReadStorage>>,
}

impl std::fmt::Debug for PortableRecording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortableRecording")
            .field("archive_path", &self.archive_path)
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl RunFilesExist for PortableRecording {
    fn store_zip_exists(&self) -> bool {
        self.outer_archive.index_of(STORE_ZIP_FILE_NAME).is_some()
    }

    fn run_log_exists(&self) -> bool {
        self.outer_archive.index_of(RUN_LOG_FILE_NAME).is_some()
    }
}

impl PortableRecording {
    /// Opens a portable recording from a file path.
    ///
    /// Validates the format and store versions on open to fail fast if the
    /// archive cannot be read by this version of nextest.
    ///
    /// This method also handles "wrapper" archives: if the archive does not
    /// contain `manifest.json` but contains exactly one `.zip` file, that inner
    /// file is treated as the nextest portable recording. This supports GitHub
    /// Actions artifact downloads, which wrap archives in an outer zip.
    pub fn open(path: &Utf8Path) -> Result<Self, PortableRecordingReadError> {
        let file = File::open(path).map_err(|error| PortableRecordingReadError::OpenArchive {
            path: path.to_owned(),
            error,
        })?;

        // Ensure the file is seekable. Process substitution paths (e.g.
        // `/proc/self/fd/11`) produce pipe fds; spool to a temp file if needed.
        let file = ensure_seekable(file, path)?;

        let mut outer_archive =
            Archive::new(BufReader::new(Either::Left(file))).map_err(|error| {
                PortableRecordingReadError::ReadArchive {
                    path: path.to_owned(),
                    error,
                }
            })?;

        // Check if this is a direct nextest archive (has manifest.json).
        if outer_archive
            .index_of(PORTABLE_MANIFEST_FILE_NAME)
            .is_some()
        {
            return Self::open_validated(path, outer_archive);
        }

        // No manifest.json found. Check if this is a wrapper archive containing
        // exactly one .zip file. Filter out directory entries (names ending with
        // '/' or '\').
        let mut file_count = 0;
        let mut zip_count = 0;
        let mut zip_file: Option<String> = None;
        for metadata in outer_archive.entries() {
            let name = metadata.name();
            if name.ends_with('/') || name.ends_with('\\') {
                // This is a directory entry, skip it.
                continue;
            }
            file_count += 1;
            if has_zip_extension(Utf8Path::new(name)) {
                zip_count += 1;
                if zip_count == 1 {
                    zip_file = Some(name.to_owned());
                }
            }
        }

        if let Some(inner_name) = zip_file.filter(|_| file_count == 1 && zip_count == 1) {
            // We only support reading up to the MAX_MAX_OUTPUT_SIZE cap. We'll
            // see if anyone complains -- they have to have both a wrapper zip
            // and to exceed the cap. (Probably worth extracting to a file on
            // disk or something at that point.)
            let inner_bytes = read_outer_file(&mut outer_archive, inner_name.into(), path)?;
            let inner_archive = Archive::new(BufReader::new(Either::Right(Cursor::new(
                inner_bytes,
            ))))
            .map_err(|error| PortableRecordingReadError::ReadArchive {
                path: path.to_owned(),
                error,
            })?;
            Self::open_validated(path, inner_archive)
        } else {
            Err(PortableRecordingReadError::NotAWrapperArchive {
                path: path.to_owned(),
                file_count,
                zip_count,
            })
        }
    }

    /// Opens and validates an archive that is known to contain `manifest.json`.
    fn open_validated(
        path: &Utf8Path,
        mut outer_archive: Archive<BufReader<ArchiveReadStorage>>,
    ) -> Result<Self, PortableRecordingReadError> {
        // Read and parse the manifest.
        let manifest_bytes =
            read_outer_file(&mut outer_archive, PORTABLE_MANIFEST_FILE_NAME.into(), path)?;
        let manifest: PortableManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                PortableRecordingReadError::ParseManifest {
                    path: path.to_owned(),
                    error,
                }
            })?;

        // Validate format version.
        if let Err(incompatibility) = manifest
            .format_version
            .check_readable_by(PORTABLE_RECORDING_FORMAT_VERSION)
        {
            return Err(PortableRecordingReadError::UnsupportedFormatVersion {
                path: path.to_owned(),
                found: manifest.format_version,
                supported: PORTABLE_RECORDING_FORMAT_VERSION,
                incompatibility,
            });
        }

        // Validate store format version.
        let store_version = manifest.store_format_version();
        if let Err(incompatibility) = store_version.check_readable_by(STORE_FORMAT_VERSION) {
            return Err(PortableRecordingReadError::UnsupportedStoreFormatVersion {
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
    /// The returned [`PortableRecordingRunLog`] can be used to iterate over events
    /// independently of this archive, avoiding borrow conflicts with
    /// [`open_store`](Self::open_store).
    pub fn read_run_log(&mut self) -> Result<PortableRecordingRunLog, PortableRecordingReadError> {
        let run_log_bytes = read_outer_file(
            &mut self.outer_archive,
            RUN_LOG_FILE_NAME.into(),
            &self.archive_path,
        )?;
        Ok(PortableRecordingRunLog {
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
    ) -> Result<ExtractOuterFileResult, PortableRecordingReadError> {
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
    /// The returned reader borrows from this archive via a zero-copy
    /// `Take` window into the outer archive's reader. CRC verification of
    /// the outer store.zip entry is skipped because each inner entry has its
    /// own CRC check.
    pub fn open_store(&mut self) -> Result<PortableStoreReader<'_>, PortableRecordingReadError> {
        let file = self
            .outer_archive
            .get_by_name(STORE_ZIP_FILE_NAME)
            .ok_or_else(|| PortableRecordingReadError::MissingFile {
                path: self.archive_path.clone(),
                file_name: Cow::Borrowed(STORE_ZIP_FILE_NAME),
            })?;

        let metadata = file.metadata();
        if metadata.compression_method != CompressionMethod::STORE {
            return Err(PortableRecordingReadError::CompressedInnerArchive {
                archive_path: self.archive_path.clone(),
                compression: metadata.compression_method,
            });
        }

        let reader = file.into_reader();
        let raw =
            metadata
                .read_raw(reader)
                .map_err(|error| PortableRecordingReadError::ReadArchive {
                    path: self.archive_path.clone(),
                    error,
                })?;

        let store_archive =
            Archive::new(raw).map_err(|error| PortableRecordingReadError::ReadArchive {
                path: self.archive_path.clone(),
                error,
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
    archive: &mut Archive<BufReader<ArchiveReadStorage>>,
    file_name: Cow<'static, str>,
    archive_path: &Utf8Path,
) -> Result<Vec<u8>, PortableRecordingReadError> {
    let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
    let mut file =
        archive
            .get_by_name(&file_name)
            .ok_or_else(|| PortableRecordingReadError::MissingFile {
                path: archive_path.to_owned(),
                file_name: file_name.clone(),
            })?;

    let claimed_size = file.metadata().uncompressed_size;
    if claimed_size > limit {
        return Err(PortableRecordingReadError::FileTooLarge {
            path: archive_path.to_owned(),
            file_name,
            size: claimed_size,
            limit,
        });
    }

    let capacity = usize::try_from(claimed_size).unwrap_or(usize::MAX);
    let mut contents = Vec::with_capacity(capacity);

    file.read()
        .and_then(|reader| reader.take(limit).read_to_end(&mut contents))
        .map_err(|error| PortableRecordingReadError::ReadArchive {
            path: archive_path.to_owned(),
            error,
        })?;

    Ok(contents)
}

/// Extracts a file from the outer archive to a path, streaming directly.
fn extract_outer_file_to_path(
    archive: &mut Archive<BufReader<ArchiveReadStorage>>,
    file_name: &'static str,
    archive_path: &Utf8Path,
    output_path: &Utf8Path,
    check_limit: bool,
) -> Result<ExtractOuterFileResult, PortableRecordingReadError> {
    let limit = MAX_MAX_OUTPUT_SIZE.as_u64();
    let mut file =
        archive
            .get_by_name(file_name)
            .ok_or_else(|| PortableRecordingReadError::MissingFile {
                path: archive_path.to_owned(),
                file_name: Cow::Borrowed(file_name),
            })?;

    let claimed_size = file.metadata().uncompressed_size;
    let exceeded_limit = if check_limit && claimed_size > limit {
        Some(claimed_size)
    } else {
        None
    };

    let mut output_file =
        File::create(output_path).map_err(|error| PortableRecordingReadError::ExtractFile {
            archive_path: archive_path.to_owned(),
            file_name,
            output_path: output_path.to_owned(),
            error,
        })?;

    let mut reader = file
        .read()
        .map_err(|error| PortableRecordingReadError::ReadArchive {
            path: archive_path.to_owned(),
            error,
        })?;

    let bytes_written = io::copy(&mut reader, &mut output_file).map_err(|error| {
        PortableRecordingReadError::ExtractFile {
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

/// The run log from a portable recording, read into memory.
///
/// This struct owns the run log bytes and can create event iterators
/// independently of the [`PortableRecording`] it came from.
#[derive(Debug)]
pub struct PortableRecordingRunLog {
    archive_path: Utf8PathBuf,
    run_log_bytes: Vec<u8>,
}

impl PortableRecordingRunLog {
    /// Returns an iterator over events from the run log.
    pub fn events(&self) -> Result<PortableRecordingEventIter<'_>, RecordReadError> {
        // The run log is zstd-compressed JSON Lines. Use with_buffer since the
        // data is already in memory (no need for Decoder's internal BufReader).
        let decoder =
            zstd::stream::Decoder::with_buffer(&self.run_log_bytes[..]).map_err(|error| {
                RecordReadError::OpenRunLog {
                    path: self.archive_path.join(RUN_LOG_FILE_NAME),
                    error,
                }
            })?;
        Ok(PortableRecordingEventIter {
            // BufReader is still needed for read_line().
            reader: DebugIgnore(BufReader::new(decoder)),
            line_buf: String::new(),
            line_number: 0,
        })
    }
}

/// Iterator over events from a portable recording's run log.
#[derive(Debug)]
pub struct PortableRecordingEventIter<'a> {
    reader: DebugIgnore<BufReader<zstd::stream::Decoder<'static, &'a [u8]>>>,
    line_buf: String,
    line_number: usize,
}

impl Iterator for PortableRecordingEventIter<'_> {
    type Item = Result<TestEventSummary<RecordingSpec>, RecordReadError>;

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

/// Reader for the inner store.zip within a portable recording.
///
/// Borrows from [`PortableRecording`] via a zero-copy `Take` window into the
/// outer archive's reader. Implements [`StoreReader`].
pub struct PortableStoreReader<'a> {
    archive_path: &'a Utf8Path,
    store_archive: Archive<io::Take<&'a mut BufReader<ArchiveReadStorage>>>,
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
        let mut file = self.store_archive.get_by_name(file_name).ok_or_else(|| {
            RecordReadError::FileNotFound {
                file_name: file_name.to_string(),
            }
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
            Err(RecordReadError::FileNotFound { .. }) => {
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
        let mut file = self.store_archive.get_by_name(store_path).ok_or_else(|| {
            RecordReadError::FileNotFound {
                file_name: store_path.to_owned(),
            }
        })?;

        let mut output_file =
            File::create(output_path).map_err(|error| RecordReadError::ExtractFile {
                store_path: store_path.to_owned(),
                output_path: output_path.to_owned(),
                error,
            })?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        format::{PORTABLE_RECORDING_FORMAT_VERSION, STORE_FORMAT_VERSION},
        store::{CompletedRunStats, RecordedRunStatus, RecordedSizes},
    };
    use camino_tempfile::{NamedUtf8TempFile, Utf8TempDir};
    use chrono::Local;
    use eazip::write::FileOptions;
    use quick_junit::ReportUuid;
    use semver::Version;
    use std::{collections::BTreeMap, io::Read};

    fn create_test_run_dir(run_id: ReportUuid) -> (Utf8TempDir, Utf8PathBuf) {
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_dir = runs_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let store_path = run_dir.join(STORE_ZIP_FILE_NAME);
        let store_file = File::create(&store_path).expect("create store.zip");
        let mut zip_writer = ArchiveWriter::new(store_file);
        let options = FileOptions::default();
        zip_writer
            .add_file("test.txt", &b"test content"[..], &options)
            .expect("add file");
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

        let writer = PortableRecordingWriter::new(&run_info, StoreRunsDir::new(&runs_dir))
            .expect("create writer");

        assert_eq!(
            writer.default_filename(),
            "nextest-run-550e8400-e29b-41d4-a716-446655440000.zip"
        );
    }

    #[test]
    fn test_write_portable_recording() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let (_temp_dir, runs_dir) = create_test_run_dir(run_id);
        let run_info = create_test_run_info(run_id);

        let writer = PortableRecordingWriter::new(&run_info, StoreRunsDir::new(&runs_dir))
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
        let mut archive = Archive::new(BufReader::new(archive_file)).expect("read archive");

        assert_eq!(archive.entries().len(), 3);

        {
            let mut manifest_file = archive
                .get_by_name(PORTABLE_MANIFEST_FILE_NAME)
                .expect("manifest");
            let mut manifest_content = String::new();
            manifest_file
                .read()
                .expect("get reader")
                .read_to_string(&mut manifest_content)
                .expect("read manifest");
            let manifest: PortableManifest =
                serde_json::from_str(&manifest_content).expect("parse manifest");
            assert_eq!(manifest.format_version, PORTABLE_RECORDING_FORMAT_VERSION);
            assert_eq!(manifest.run.run_id, run_id);
        }

        {
            let store_file = archive.get_by_name(STORE_ZIP_FILE_NAME).expect("store.zip");
            assert!(store_file.metadata().uncompressed_size > 0);
        }

        {
            let log_file = archive.get_by_name(RUN_LOG_FILE_NAME).expect("run.log.zst");
            assert!(log_file.metadata().uncompressed_size > 0);
        }
    }

    #[test]
    fn test_missing_run_dir() {
        let run_id = ReportUuid::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
        let temp_dir = camino_tempfile::tempdir().expect("create temp dir");
        let runs_dir = temp_dir.path().to_owned();
        let run_info = create_test_run_info(run_id);

        let result = PortableRecordingWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(matches!(
            result,
            Err(PortableRecordingError::RunDirNotFound { .. })
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
        let result = PortableRecordingWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(
            matches!(
                &result,
                Err(PortableRecordingError::RequiredFileMissing { file_name, .. })
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
        let mut zip_writer = ArchiveWriter::new(store_file);
        let options = FileOptions::default();
        zip_writer
            .add_file("test.txt", &b"test"[..], &options)
            .expect("add file");
        zip_writer.finish().expect("finish");

        let run_info = create_test_run_info(run_id);
        let result = PortableRecordingWriter::new(&run_info, StoreRunsDir::new(&runs_dir));

        assert!(
            matches!(
                &result,
                Err(PortableRecordingError::RequiredFileMissing { file_name, .. })
                if *file_name == RUN_LOG_FILE_NAME
            ),
            "expected RequiredFileMissing for run.log.zst, got {result:?}"
        );
    }

    #[test]
    fn test_ensure_seekable_regular_file() {
        // A regular file is already seekable and should be returned as-is.
        let temp = NamedUtf8TempFile::new().expect("created temp file");
        let path = temp.path().to_owned();

        std::fs::write(&path, b"hello world").expect("wrote to temp file");
        let file = File::open(&path).expect("opened temp file");

        // Get the file's OS-level fd/handle for identity comparison.
        #[cfg(unix)]
        let original_fd = {
            use std::os::unix::io::AsRawFd;
            file.as_raw_fd()
        };

        let result = ensure_seekable(file, &path).expect("ensure_seekable succeeded");

        // The returned file should be the same fd (no spooling occurred).
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            assert_eq!(
                result.as_raw_fd(),
                original_fd,
                "seekable file should be returned as-is"
            );
        }

        // Verify the content is still readable.
        let mut contents = String::new();
        let mut reader = io::BufReader::new(result);
        reader
            .read_to_string(&mut contents)
            .expect("read file contents");
        assert_eq!(contents, "hello world");
    }

    /// Converts a `PipeReader` into a `File` using platform-specific owned
    /// I/O types.
    #[cfg(unix)]
    fn pipe_reader_to_file(reader: std::io::PipeReader) -> File {
        use std::os::fd::OwnedFd;
        File::from(OwnedFd::from(reader))
    }

    /// Converts a `PipeReader` into a `File` using platform-specific owned
    /// I/O types.
    #[cfg(windows)]
    fn pipe_reader_to_file(reader: std::io::PipeReader) -> File {
        use std::os::windows::io::OwnedHandle;
        File::from(OwnedHandle::from(reader))
    }

    /// Tests that non-seekable inputs (pipes) are spooled to a temp file.
    ///
    /// This test uses `std::io::pipe()` to create a real pipe, which is the
    /// same mechanism the OS uses for process substitution (`<(command)`).
    #[test]
    fn test_ensure_seekable_pipe() {
        let (pipe_reader, mut pipe_writer) = std::io::pipe().expect("created pipe");
        let test_data = b"zip-like test content for pipe spooling";

        // Write data and close the write end so the read end reaches EOF.
        pipe_writer.write_all(test_data).expect("wrote to pipe");
        drop(pipe_writer);

        let pipe_file = pipe_reader_to_file(pipe_reader);

        let path = Utf8Path::new("/dev/fd/99");
        let result = ensure_seekable(pipe_file, path).expect("ensure_seekable succeeded");

        // The result should be a seekable temp file containing the pipe data.
        let mut contents = Vec::new();
        let mut reader = io::BufReader::new(result);
        reader
            .read_to_end(&mut contents)
            .expect("read spooled contents");
        assert_eq!(contents, test_data);
    }

    /// Tests that an empty pipe (zero bytes) is handled correctly.
    ///
    /// This simulates a download failure where the source produces no data.
    /// `ensure_seekable` should succeed (the temp file is created and rewound),
    /// and the downstream ZIP reader will report a proper error.
    #[test]
    fn test_ensure_seekable_empty_pipe() {
        let (pipe_reader, pipe_writer) = std::io::pipe().expect("created pipe");
        // Close writer immediately to produce an empty pipe.
        drop(pipe_writer);

        let pipe_file = pipe_reader_to_file(pipe_reader);
        let path = Utf8Path::new("/dev/fd/42");
        let mut result = ensure_seekable(pipe_file, path).expect("empty pipe should succeed");

        let mut contents = Vec::new();
        result.read_to_end(&mut contents).expect("read contents");
        assert!(contents.is_empty());
    }

    /// Tests that the spool size limit is enforced for pipes.
    ///
    /// Uses `ensure_seekable_impl` with a small limit so we can trigger the
    /// `SpoolTooLarge` error without writing gigabytes.
    #[test]
    fn test_ensure_seekable_spool_too_large() {
        let (pipe_reader, mut pipe_writer) = std::io::pipe().expect("created pipe");

        // Write 20 bytes, then set a limit of 10.
        pipe_writer
            .write_all(b"01234567890123456789")
            .expect("wrote to pipe");
        drop(pipe_writer);

        let pipe_file = pipe_reader_to_file(pipe_reader);

        let path = Utf8Path::new("/dev/fd/42");
        let result = ensure_seekable_impl(pipe_file, path, ByteSize(10));
        assert!(
            matches!(
                &result,
                Err(PortableRecordingReadError::SpoolTooLarge {
                    limit: ByteSize(10),
                    ..
                })
            ),
            "expected SpoolTooLarge, got {result:?}"
        );
    }

    /// Tests that data exactly one byte over the spool limit fails.
    ///
    /// This is the precise boundary: `take(limit + 1)` reads exactly
    /// `limit + 1` bytes, and `bytes_copied > limit` triggers the error.
    #[test]
    fn test_ensure_seekable_spool_one_over_limit() {
        let (pipe_reader, mut pipe_writer) = std::io::pipe().expect("created pipe");

        // Write exactly limit + 1 = 11 bytes with a limit of 10.
        pipe_writer
            .write_all(b"01234567890")
            .expect("wrote to pipe");
        drop(pipe_writer);

        let pipe_file = pipe_reader_to_file(pipe_reader);

        let path = Utf8Path::new("/dev/fd/42");
        let result = ensure_seekable_impl(pipe_file, path, ByteSize(10));
        assert!(
            matches!(
                &result,
                Err(PortableRecordingReadError::SpoolTooLarge {
                    limit: ByteSize(10),
                    ..
                })
            ),
            "expected SpoolTooLarge at limit+1 bytes, got {result:?}"
        );
    }

    /// Tests that data exactly at the spool limit succeeds.
    #[test]
    fn test_ensure_seekable_spool_exact_limit() {
        let (pipe_reader, mut pipe_writer) = std::io::pipe().expect("created pipe");

        // Write exactly 10 bytes with a limit of 10.
        pipe_writer.write_all(b"0123456789").expect("wrote to pipe");
        drop(pipe_writer);

        let pipe_file = pipe_reader_to_file(pipe_reader);

        let path = Utf8Path::new("/dev/fd/42");
        let mut result = ensure_seekable_impl(pipe_file, path, ByteSize(10))
            .expect("exact limit should succeed");

        // Verify the spooled content is correct.
        let mut contents = Vec::new();
        result.read_to_end(&mut contents).expect("read contents");
        assert_eq!(contents, b"0123456789");
    }
}
