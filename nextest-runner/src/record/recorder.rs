// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recording logic for individual test runs.
//!
//! The [`RunRecorder`] handles writing a single test run to disk, including:
//!
//! - A zstd-compressed zip archive (`store.zip`) containing metadata and outputs.
//! - A zstd-compressed JSON Lines log file (`run.log.zst`) containing test events.

use super::{
    dicts,
    format::{
        CARGO_METADATA_JSON_PATH, OutputDict, RECORD_OPTS_JSON_PATH, RUN_LOG_FILE_NAME,
        STDERR_DICT_PATH, STDOUT_DICT_PATH, STORE_ZIP_FILE_NAME, TEST_LIST_JSON_PATH,
    },
    summary::{
        OutputEventKind, OutputFileName, OutputKind, RecordOpts, TestEventKindSummary,
        TestEventSummary, ZipStoreOutput,
    },
};
use crate::{
    errors::{RunStoreError, StoreWriterError},
    record::format::{RERUN_INFO_JSON_PATH, RerunInfo},
    reporter::events::{
        ChildExecutionOutputDescription, ChildOutputDescription, ExecuteStatus, ExecutionStatuses,
        SetupScriptExecuteStatus,
    },
    test_output::ChildSingleOutput,
};
use camino::{Utf8Path, Utf8PathBuf};
use countio::Counter;
use debug_ignore::DebugIgnore;
use nextest_metadata::TestListSummary;
use std::{
    borrow::Cow,
    collections::HashSet,
    fs::File,
    io::{self, Write},
};
use zip::{CompressionMethod, ZipWriter};

/// Zstd encoder that auto-finishes on drop but also supports explicit finish.
///
/// Unlike `zstd::stream::AutoFinishEncoder`, this wrapper allows calling
/// `finish()` explicitly to get error handling and the underlying writer back.
/// If dropped without calling `finish()`, the stream is finalized and errors
/// are ignored.
///
/// The encoder is wrapped in `Counter<Encoder<Counter<File>>>`:
/// - Outer Counter tracks uncompressed bytes written to the encoder.
/// - Inner Counter tracks compressed bytes written to the file.
struct LogEncoder {
    /// The inner encoder, wrapped in Option so we can take it in finish().
    /// Counter<Encoder<Counter<File>>> tracks both uncompressed and compressed sizes.
    inner: Option<Counter<zstd::stream::Encoder<'static, Counter<File>>>>,
}

impl std::fmt::Debug for LogEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogEncoder").finish_non_exhaustive()
    }
}

impl LogEncoder {
    fn new(encoder: zstd::stream::Encoder<'static, Counter<File>>) -> Self {
        Self {
            inner: Some(Counter::new(encoder)),
        }
    }

    /// Finishes the encoder and returns the compressed and uncompressed sizes.
    ///
    /// The `entries` parameter is the number of log entries written.
    fn finish(mut self, entries: u64) -> io::Result<ComponentSizes> {
        let counter = self.inner.take().expect("encoder already finished");
        let uncompressed = counter.writer_bytes() as u64;
        let file_counter = counter.into_inner().finish()?;
        let compressed = file_counter.writer_bytes() as u64;
        Ok(ComponentSizes {
            compressed,
            uncompressed,
            entries,
        })
    }
}

impl Write for LogEncoder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .as_mut()
            .expect("encoder already finished")
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner
            .as_mut()
            .expect("encoder already finished")
            .flush()
    }
}

impl Drop for LogEncoder {
    fn drop(&mut self) {
        if let Some(counter) = self.inner.take() {
            // Intentionally ignore errors here. This Drop impl only runs if
            // finish() wasn't called, which only happens during a panic. In
            // that situation, logging or other side effects could make things
            // worse.
            let _ = counter.into_inner().finish();
        }
    }
}

/// Records a single test run to disk.
///
/// Created by `ExclusiveLockedRunStore::create_run_recorder`. Writes both a zip
/// archive with metadata and outputs, and a zstd-compressed JSON Lines log.
#[derive(Debug)]
pub struct RunRecorder {
    store_path: Utf8PathBuf,
    store_writer: StoreWriter,
    log_path: Utf8PathBuf,
    log: DebugIgnore<LogEncoder>,
    /// Number of log entries (records) written.
    log_entries: u64,
    max_output_size: usize,
}

impl RunRecorder {
    /// Creates a new `RunRecorder` in the given directory.
    ///
    /// `max_output_size` specifies the maximum size of a single output (stdout/stderr)
    /// before truncation. Outputs exceeding this size will have the middle portion removed.
    pub(super) fn new(
        run_dir: Utf8PathBuf,
        max_output_size: bytesize::ByteSize,
    ) -> Result<Self, RunStoreError> {
        std::fs::create_dir_all(&run_dir).map_err(|error| RunStoreError::RunDirCreate {
            run_dir: run_dir.clone(),
            error,
        })?;

        let store_path = run_dir.join(STORE_ZIP_FILE_NAME);
        let store_writer =
            StoreWriter::new(&store_path).map_err(|error| RunStoreError::StoreWrite {
                store_path: store_path.clone(),
                error,
            })?;

        let log_path = run_dir.join(RUN_LOG_FILE_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&log_path)
            .map_err(|error| RunStoreError::RunLogCreate {
                path: log_path.clone(),
                error,
            })?;

        // Compression level 3 is a good balance of speed and ratio. The zstd
        // library has its own internal buffer (~128KB), so no additional
        // buffering is needed.
        let encoder = zstd::stream::Encoder::new(Counter::new(file), 3).map_err(|error| {
            RunStoreError::RunLogCreate {
                path: log_path.clone(),
                error,
            }
        })?;
        let log = LogEncoder::new(encoder);

        Ok(Self {
            store_path,
            store_writer,
            log_path,
            log: DebugIgnore(log),
            log_entries: 0,
            // Saturate to usize::MAX on 32-bit platforms. This is fine because
            // you can't allocate more than usize::MAX bytes anyway.
            max_output_size: usize::try_from(max_output_size.as_u64()).unwrap_or(usize::MAX),
        })
    }

    /// Writes metadata (cargo metadata, test list, options, and dictionaries) to the archive.
    ///
    /// This should be called once at the beginning of a test run.
    ///
    /// Note: The store format version is stored in runs.json.zst, not in the archive itself.
    /// This allows checking replayability without opening the archive.
    pub(crate) fn write_meta(
        &mut self,
        cargo_metadata_json: &str,
        test_list: &TestListSummary,
        opts: &RecordOpts,
    ) -> Result<(), RunStoreError> {
        let test_list_json = serde_json::to_string(test_list)
            .map_err(|error| RunStoreError::TestListSerialize { error })?;

        let opts_json = serde_json::to_string(opts)
            .map_err(|error| RunStoreError::RecordOptionsSerialize { error })?;

        self.write_archive_file(TEST_LIST_JSON_PATH, test_list_json.as_bytes())?;
        self.write_archive_file(CARGO_METADATA_JSON_PATH, cargo_metadata_json.as_bytes())?;
        self.write_archive_file(RECORD_OPTS_JSON_PATH, opts_json.as_bytes())?;

        // Write dictionaries to make the archive self-contained.
        self.write_archive_file(STDOUT_DICT_PATH, dicts::STDOUT)?;
        self.write_archive_file(STDERR_DICT_PATH, dicts::STDERR)?;

        Ok(())
    }

    /// Writes rerun-specific metadata to the archive.
    ///
    /// This should be called once at the beginning of a rerun (after setup).
    pub(crate) fn write_rerun_info(&mut self, rerun_info: &RerunInfo) -> Result<(), RunStoreError> {
        let rerun_info_json = serde_json::to_string(rerun_info)
            .map_err(|error| RunStoreError::RerunInfoSerialize { error })?;

        self.write_archive_file(RERUN_INFO_JSON_PATH, rerun_info_json.as_bytes())?;

        Ok(())
    }

    fn write_archive_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), RunStoreError> {
        self.store_writer
            .add_file(Utf8PathBuf::from(path), bytes)
            .map_err(|error| RunStoreError::StoreWrite {
                store_path: self.store_path.clone(),
                error,
            })
    }

    /// Writes a test event to the archive and log.
    ///
    /// The event's outputs are written to the zip archive, and the event
    /// (with file references) is written to the JSON Lines log.
    pub(crate) fn write_event(
        &mut self,
        event: TestEventSummary<ChildSingleOutput>,
    ) -> Result<(), RunStoreError> {
        let mut cx = SerializeTestEventContext {
            store_writer: &mut self.store_writer,
            max_output_size: self.max_output_size,
        };

        let event = cx
            .convert_event(event)
            .map_err(|error| RunStoreError::StoreWrite {
                store_path: self.store_path.clone(),
                error,
            })?;

        let json = serde_json::to_string(&event)
            .map_err(|error| RunStoreError::TestEventSerialize { error })?;
        self.write_log_impl(json.as_bytes())?;
        self.write_log_impl(b"\n")?;

        self.log_entries += 1;

        Ok(())
    }

    fn write_log_impl(&mut self, bytes: &[u8]) -> Result<(), RunStoreError> {
        self.log
            .write_all(bytes)
            .map_err(|error| RunStoreError::RunLogWrite {
                path: self.log_path.clone(),
                error,
            })
    }

    /// Finishes writing and closes all files.
    ///
    /// This must be called to ensure all data is flushed to disk.
    /// Returns the compressed and uncompressed sizes for both log and store.
    pub(crate) fn finish(self) -> Result<StoreSizes, RunStoreError> {
        let log_sizes =
            self.log
                .0
                .finish(self.log_entries)
                .map_err(|error| RunStoreError::RunLogFlush {
                    path: self.log_path.clone(),
                    error,
                })?;

        let store_sizes =
            self.store_writer
                .finish()
                .map_err(|error| RunStoreError::StoreWrite {
                    store_path: self.store_path.clone(),
                    error,
                })?;

        Ok(StoreSizes {
            log: log_sizes,
            store: store_sizes,
        })
    }
}

/// Writes files to a zstd-compressed zip archive.
#[derive(Debug)]
pub(crate) struct StoreWriter {
    writer: DebugIgnore<ZipWriter<Counter<File>>>,
    added_files: HashSet<Utf8PathBuf>,
    /// Total uncompressed size of all files added to the archive.
    uncompressed_size: u64,
}

impl StoreWriter {
    /// Creates a new `StoreWriter` at the given path.
    fn new(store_path: &Utf8Path) -> Result<Self, StoreWriterError> {
        let zip_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(store_path)
            .map_err(|error| StoreWriterError::Create { error })?;
        let writer = ZipWriter::new(Counter::new(zip_file));

        Ok(Self {
            writer: DebugIgnore(writer),
            added_files: HashSet::new(),
            uncompressed_size: 0,
        })
    }

    /// Adds a file to the archive.
    ///
    /// Output files (in `out/`) are pre-compressed with zstd dictionaries for
    /// better compression. Metadata files use standard zstd compression.
    ///
    /// If a file with the same path has already been added, this is a no-op.
    fn add_file(&mut self, path: Utf8PathBuf, contents: &[u8]) -> Result<(), StoreWriterError> {
        if self.added_files.contains(&path) {
            return Ok(());
        }

        // Track the uncompressed size of the file.
        self.uncompressed_size += contents.len() as u64;

        let dict = OutputDict::for_path(&path);
        match dict.dict_bytes() {
            Some(dict_bytes) => {
                let compressed = compress_with_dict(contents, dict_bytes)
                    .map_err(|error| StoreWriterError::Compress { error })?;

                let options = zip::write::FileOptions::<'_, ()>::default()
                    .compression_method(CompressionMethod::Stored);
                self.writer
                    .start_file(path.as_str(), options)
                    .map_err(|error| StoreWriterError::StartFile {
                        path: path.clone(),
                        error,
                    })?;
                self.writer
                    .write_all(&compressed)
                    .map_err(|error| StoreWriterError::Write {
                        path: path.clone(),
                        error,
                    })?;
            }
            None => {
                let options = zip::write::FileOptions::<'_, ()>::default()
                    .compression_method(CompressionMethod::Zstd);
                self.writer
                    .start_file(path.as_str(), options)
                    .map_err(|error| StoreWriterError::StartFile {
                        path: path.clone(),
                        error,
                    })?;
                self.writer
                    .write_all(contents)
                    .map_err(|error| StoreWriterError::Write {
                        path: path.clone(),
                        error,
                    })?;
            }
        }

        self.added_files.insert(path);

        Ok(())
    }

    /// Finishes writing and closes the archive.
    ///
    /// Returns the compressed and uncompressed sizes and entry count.
    fn finish(self) -> Result<ComponentSizes, StoreWriterError> {
        let entries = self.added_files.len() as u64;
        let mut counter = self
            .writer
            .0
            .finish()
            .map_err(|error| StoreWriterError::Finish { error })?;

        counter
            .flush()
            .map_err(|error| StoreWriterError::Flush { error })?;

        Ok(ComponentSizes {
            compressed: counter.writer_bytes() as u64,
            uncompressed: self.uncompressed_size,
            entries,
        })
    }
}

/// Compressed and uncompressed sizes for a single component (log or store).
#[derive(Clone, Copy, Debug, Default)]
pub struct ComponentSizes {
    /// Compressed size in bytes.
    pub compressed: u64,
    /// Uncompressed size in bytes.
    pub uncompressed: u64,
    /// Number of entries (records for log, files for store).
    pub entries: u64,
}

/// Compressed and uncompressed sizes for storage, broken down by component.
#[derive(Clone, Copy, Debug, Default)]
pub struct StoreSizes {
    /// Sizes for the run log (run.log.zst).
    pub log: ComponentSizes,
    /// Sizes for the store archive (store.zip).
    pub store: ComponentSizes,
}

impl StoreSizes {
    /// Returns the total compressed size (log + store).
    pub fn total_compressed(&self) -> u64 {
        self.log.compressed + self.store.compressed
    }

    /// Returns the total uncompressed size (log + store).
    pub fn total_uncompressed(&self) -> u64 {
        self.log.uncompressed + self.store.uncompressed
    }
}

/// Compresses data using a pre-trained zstd dictionary.
fn compress_with_dict(data: &[u8], dict_bytes: &[u8]) -> io::Result<Vec<u8>> {
    // Compression level 3 is a good balance of speed and ratio for
    // dictionaries.
    let dict = zstd::dict::EncoderDictionary::copy(dict_bytes, 3);
    let mut encoder = zstd::stream::Encoder::with_prepared_dictionary(Vec::new(), &dict)?;
    encoder.write_all(data)?;
    encoder.finish()
}

/// Context for serializing test events to the zip store.
///
/// Handles writing output buffers to the zip and converting in-memory
/// references to file references.
struct SerializeTestEventContext<'a> {
    store_writer: &'a mut StoreWriter,
    max_output_size: usize,
}

impl SerializeTestEventContext<'_> {
    /// Converts an in-memory event to a zip store event.
    fn convert_event(
        &mut self,
        event: TestEventSummary<ChildSingleOutput>,
    ) -> Result<TestEventSummary<ZipStoreOutput>, StoreWriterError> {
        Ok(TestEventSummary {
            timestamp: event.timestamp,
            elapsed: event.elapsed,
            kind: self.convert_event_kind(event.kind)?,
        })
    }

    fn convert_event_kind(
        &mut self,
        kind: TestEventKindSummary<ChildSingleOutput>,
    ) -> Result<TestEventKindSummary<ZipStoreOutput>, StoreWriterError> {
        match kind {
            TestEventKindSummary::Core(core) => Ok(TestEventKindSummary::Core(core)),
            TestEventKindSummary::Output(output) => Ok(TestEventKindSummary::Output(
                self.convert_output_event(output)?,
            )),
        }
    }

    fn convert_output_event(
        &mut self,
        event: OutputEventKind<ChildSingleOutput>,
    ) -> Result<OutputEventKind<ZipStoreOutput>, StoreWriterError> {
        match event {
            OutputEventKind::SetupScriptFinished {
                stress_index,
                index,
                total,
                script_id,
                program,
                args,
                no_capture,
                run_status,
            } => {
                let run_status = self.convert_setup_script_status(&run_status)?;
                Ok(OutputEventKind::SetupScriptFinished {
                    stress_index,
                    index,
                    total,
                    script_id,
                    program,
                    args,
                    no_capture,
                    run_status,
                })
            }
            OutputEventKind::TestAttemptFailedWillRetry {
                stress_index,
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
                running,
            } => {
                let run_status = self.convert_execute_status(run_status)?;
                Ok(OutputEventKind::TestAttemptFailedWillRetry {
                    stress_index,
                    test_instance,
                    run_status,
                    delay_before_next_attempt,
                    failure_output,
                    running,
                })
            }
            OutputEventKind::TestFinished {
                stress_index,
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
            } => {
                let run_statuses = self.convert_execution_statuses(run_statuses)?;
                Ok(OutputEventKind::TestFinished {
                    stress_index,
                    test_instance,
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats,
                    running,
                })
            }
        }
    }

    fn convert_setup_script_status(
        &mut self,
        status: &SetupScriptExecuteStatus<ChildSingleOutput>,
    ) -> Result<SetupScriptExecuteStatus<ZipStoreOutput>, StoreWriterError> {
        Ok(SetupScriptExecuteStatus {
            output: self.convert_child_execution_output(&status.output)?,
            result: status.result.clone(),
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            env_map: status.env_map.clone(),
            error_summary: status.error_summary.clone(),
        })
    }

    fn convert_execution_statuses(
        &mut self,
        statuses: ExecutionStatuses<ChildSingleOutput>,
    ) -> Result<ExecutionStatuses<ZipStoreOutput>, StoreWriterError> {
        let statuses = statuses
            .into_iter()
            .map(|status| self.convert_execute_status(status))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExecutionStatuses::new(statuses))
    }

    fn convert_execute_status(
        &mut self,
        status: ExecuteStatus<ChildSingleOutput>,
    ) -> Result<ExecuteStatus<ZipStoreOutput>, StoreWriterError> {
        let output = self.convert_child_execution_output(&status.output)?;

        Ok(ExecuteStatus {
            retry_data: status.retry_data,
            output,
            result: status.result,
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            delay_before_start: status.delay_before_start,
            error_summary: status.error_summary,
            output_error_slice: status.output_error_slice,
        })
    }

    fn convert_child_execution_output(
        &mut self,
        output: &ChildExecutionOutputDescription<ChildSingleOutput>,
    ) -> Result<ChildExecutionOutputDescription<ZipStoreOutput>, StoreWriterError> {
        match output {
            ChildExecutionOutputDescription::Output {
                result,
                output,
                errors,
            } => {
                let output = self.convert_child_output(output)?;
                Ok(ChildExecutionOutputDescription::Output {
                    result: result.clone(),
                    output,
                    errors: errors.clone(),
                })
            }
            ChildExecutionOutputDescription::StartError(err) => {
                Ok(ChildExecutionOutputDescription::StartError(err.clone()))
            }
        }
    }

    fn convert_child_output(
        &mut self,
        output: &ChildOutputDescription<ChildSingleOutput>,
    ) -> Result<ChildOutputDescription<ZipStoreOutput>, StoreWriterError> {
        match output {
            ChildOutputDescription::Split { stdout, stderr } => Ok(ChildOutputDescription::Split {
                stdout: Some(self.write_single_output(stdout.as_ref(), OutputKind::Stdout)?),
                stderr: Some(self.write_single_output(stderr.as_ref(), OutputKind::Stderr)?),
            }),
            ChildOutputDescription::Combined { output } => Ok(ChildOutputDescription::Combined {
                output: self.write_single_output(Some(output), OutputKind::Combined)?,
            }),
        }
    }

    /// Writes a single output to the archive using content-addressed naming.
    ///
    /// The file name is a hash of the content, enabling deduplication of
    /// identical outputs across stress iterations, retries, and tests.
    fn write_single_output(
        &mut self,
        output: Option<&ChildSingleOutput>,
        kind: OutputKind,
    ) -> Result<ZipStoreOutput, StoreWriterError> {
        let Some(output) = output else {
            return Ok(ZipStoreOutput::Empty);
        };

        if output.buf.is_empty() {
            return Ok(ZipStoreOutput::Empty);
        }

        let original_len = output.buf.len();
        let (data, truncated): (Cow<'_, [u8]>, bool) = if original_len <= self.max_output_size {
            (Cow::Borrowed(&output.buf), false)
        } else {
            (truncate_output(&output.buf, self.max_output_size), true)
        };

        let file_name = OutputFileName::from_content(&data, kind);
        let file_path = Utf8PathBuf::from(format!("out/{file_name}"));

        self.store_writer.add_file(file_path, &data)?;

        if truncated {
            Ok(ZipStoreOutput::Truncated {
                file_name,
                original_size: original_len as u64,
            })
        } else {
            Ok(ZipStoreOutput::Full { file_name })
        }
    }
}

/// Truncates output to fit within `max_size` by keeping the start and end.
///
/// If `buf` is already within `max_size`, returns a borrowed reference.
/// Otherwise, returns an owned buffer with approximately equal portions from
/// the start and end, with a marker in the middle indicating how many bytes
/// were removed.
fn truncate_output(buf: &[u8], max_size: usize) -> Cow<'_, [u8]> {
    if buf.len() <= max_size {
        return Cow::Borrowed(buf);
    }

    let truncated_bytes = buf.len() - max_size;
    let marker = format!("\n\n... [truncated {truncated_bytes} bytes] ...\n\n");
    let marker_bytes = marker.as_bytes();

    let content_space = max_size.saturating_sub(marker_bytes.len());
    let head_size = content_space / 2;
    let tail_size = content_space - head_size;

    let mut result = Vec::with_capacity(max_size);
    result.extend_from_slice(&buf[..head_size]);
    result.extend_from_slice(marker_bytes);
    result.extend_from_slice(&buf[buf.len() - tail_size..]);

    Cow::Owned(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::dicts;

    #[test]
    fn test_truncate_output_no_truncation_needed() {
        let input = b"hello world";
        let result = truncate_output(input, 100);
        assert_eq!(&*result, input);
        assert!(matches!(result, Cow::Borrowed(_)), "should be borrowed");
    }

    #[test]
    fn test_truncate_output_exact_size() {
        let input = b"exactly100bytes";
        let result = truncate_output(input, input.len());
        assert_eq!(&*result, input);
        assert!(matches!(result, Cow::Borrowed(_)), "should be borrowed");
    }

    #[test]
    fn test_truncate_output_basic() {
        // Create input that exceeds max_size.
        let input: Vec<u8> = (0..200).collect();
        let max_size = 100;

        let result = truncate_output(&input, max_size);

        // Should be owned since truncation occurred.
        assert!(matches!(result, Cow::Owned(_)), "should be owned");

        // Result should be at or under max_size.
        assert!(
            result.len() <= max_size,
            "result len {} should be <= max_size {}",
            result.len(),
            max_size
        );

        // Should contain the truncation marker.
        let result_str = String::from_utf8_lossy(&result);
        assert!(
            result_str.contains("[truncated"),
            "should contain truncation marker: {result_str:?}"
        );
        assert!(
            result_str.contains("bytes]"),
            "should contain 'bytes]': {result_str:?}"
        );

        // Should start with beginning of original input.
        assert!(
            result.starts_with(&[0, 1, 2]),
            "should start with beginning of input"
        );

        // Should end with end of original input.
        assert!(
            result.ends_with(&[197, 198, 199]),
            "should end with end of input"
        );
    }

    #[test]
    fn test_truncate_output_preserves_head_and_tail() {
        let head = b"HEAD_CONTENT_";
        let middle = vec![b'x'; 1000];
        let tail = b"_TAIL_CONTENT";

        let mut input = Vec::new();
        input.extend_from_slice(head);
        input.extend_from_slice(&middle);
        input.extend_from_slice(tail);

        let max_size = 200;
        let result = truncate_output(&input, max_size);

        assert!(result.len() <= max_size);

        // Head should be preserved.
        assert!(
            result.starts_with(b"HEAD"),
            "should preserve head: {:?}",
            String::from_utf8_lossy(&result[..20])
        );

        // Tail should be preserved.
        assert!(
            result.ends_with(b"CONTENT"),
            "should preserve tail: {:?}",
            String::from_utf8_lossy(&result[result.len() - 20..])
        );
    }

    #[test]
    fn test_truncate_output_marker_shows_correct_count() {
        let input: Vec<u8> = vec![b'a'; 1000];
        let max_size = 100;

        let result = truncate_output(&input, max_size);
        let result_str = String::from_utf8_lossy(&result);

        // Should show 900 bytes truncated (1000 - 100 = 900).
        assert!(
            result_str.contains("[truncated 900 bytes]"),
            "should show correct truncation count: {result_str:?}"
        );
    }

    #[test]
    fn test_truncate_output_large_input() {
        // Simulate a more realistic scenario with larger input.
        let input: Vec<u8> = vec![b'x'; 20 * 1024 * 1024]; // 20 MB
        let max_size = 10 * 1024 * 1024; // 10 MB

        let result = truncate_output(&input, max_size);

        assert!(
            result.len() <= max_size,
            "result {} should be <= max_size {}",
            result.len(),
            max_size
        );

        let result_str = String::from_utf8_lossy(&result);
        assert!(
            result_str.contains("[truncated"),
            "should contain truncation marker"
        );
    }

    #[test]
    fn test_truncate_output_max_size_smaller_than_marker() {
        // When max_size is smaller than the marker itself, the function should
        // still produce a valid result. The marker is approximately 35+ bytes:
        // "\n\n... [truncated N bytes] ...\n\n".
        let input: Vec<u8> = vec![b'x'; 100];
        let max_size = 10; // Much smaller than the marker.

        let result = truncate_output(&input, max_size);

        // The result will be just the marker since there's no room for content.
        // This means result.len() > max_size, which is acceptable because the
        // marker is the minimum output when truncation occurs.
        let result_str = String::from_utf8_lossy(&result);
        assert!(
            result_str.contains("[truncated"),
            "should still contain truncation marker: {result_str:?}"
        );

        // The result should be the marker with no content bytes.
        assert!(
            result_str.starts_with("\n\n..."),
            "should start with marker prefix"
        );
        assert!(
            result_str.ends_with("...\n\n"),
            "should end with marker suffix"
        );
    }

    #[test]
    fn test_truncate_output_max_size_zero() {
        // Edge case: max_size of 0 should still produce the marker.
        let input: Vec<u8> = vec![b'x'; 50];
        let max_size = 0;

        let result = truncate_output(&input, max_size);

        // With max_size = 0, content_space = 0, so result is just the marker.
        let result_str = String::from_utf8_lossy(&result);
        assert!(
            result_str.contains("[truncated 50 bytes]"),
            "should show correct truncation count: {result_str:?}"
        );
    }

    #[test]
    fn test_compress_with_dict_stdout() {
        // Test data that looks like typical test output.
        let test_output = b"running 1 test\ntest tests::my_test ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored\n";

        // Compress with stdout dictionary.
        let compressed =
            compress_with_dict(test_output, dicts::STDOUT).expect("compression failed");

        // Decompress with the same dictionary.
        let dict = zstd::dict::DecoderDictionary::copy(dicts::STDOUT);
        let mut decoder = zstd::stream::Decoder::with_prepared_dictionary(&compressed[..], &dict)
            .expect("decoder creation failed");
        let mut decompressed = Vec::new();
        io::Read::read_to_end(&mut decoder, &mut decompressed).expect("decompression failed");

        assert_eq!(decompressed, test_output, "round-trip should preserve data");
    }
}
