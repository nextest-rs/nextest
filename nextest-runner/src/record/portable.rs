// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Portable archive creation for recorded runs.
//!
//! A portable archive packages a single recorded run into a self-contained zip
//! file that can be shared and imported elsewhere.

use super::{
    format::{
        PORTABLE_MANIFEST_FILE_NAME, PortableManifest, RUN_LOG_FILE_NAME, STORE_ZIP_FILE_NAME,
    },
    store::{RecordedRunInfo, StoreRunsDir},
};
use crate::errors::PortableArchiveError;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::{Utf8Path, Utf8PathBuf};
use countio::Counter;
use std::{
    fs::File,
    io::{self, Write},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Result of writing a portable archive.
#[derive(Debug)]
pub struct PortableArchiveResult {
    /// The path to the written archive.
    pub path: Utf8PathBuf,
    /// The total size of the archive in bytes.
    pub size: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        format::{PORTABLE_ARCHIVE_FORMAT_VERSION, STORE_FORMAT_VERSION},
        store::{CompletedRunStats, RecordedRunStatus, RecordedSizes},
    };
    use chrono::Local;
    use quick_junit::ReportUuid;
    use semver::Version;
    use std::{collections::BTreeMap, io::Read};
    use zip::ZipArchive;

    fn create_test_run_dir(run_id: ReportUuid) -> (camino_tempfile::Utf8TempDir, Utf8PathBuf) {
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
