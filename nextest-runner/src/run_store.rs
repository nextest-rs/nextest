// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storage management for nextest runs.

use crate::{
    config::ScriptId,
    errors::{RunStoreError, StoreWriterError},
    list::TestInstance,
    reporter::{CancelReason, TestEvent, TestEventKind, TestOutputDisplay},
    runner::{
        ExecuteStatus, ExecutionResult, ExecutionStatuses, RetryData, RunStats,
        SetupScriptExecuteStatus,
    },
    test_output::{TestOutput, TestSingleOutput},
};
use bytes::Bytes;
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, FixedOffset};
use debug_ignore::DebugIgnore;
use fs4::FileExt;
use nextest_metadata::{MismatchReason, RustBinaryId, RustTestCaseSummary, TestListSummary};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt,
    fs::File,
    io::{self, LineWriter, Write},
    time::Duration,
};
use uuid::Uuid;
use xxhash_rust::xxh3::Xxh3;
use zip::ZipWriter;

static RUNS_LOCK_FILE_NAME: &str = "runs.lock";
static RUNS_JSON_FILE_NAME: &str = "runs.json";
static STORE_ZIP_FILE_NAME: &str = "store.zip";
static CARGO_METADATA_JSON_FILE_NAME: &str = "cargo-metadata.json";
static TEST_LIST_JSON_FILE_NAME: &str = "test-list.json";
static RUN_LOG_FILE_NAME: &str = "run.log";

/// Manages the storage of runs.
#[derive(Debug)]
pub struct RunStore {
    runs_dir: Utf8PathBuf,
}

impl RunStore {
    /// Creates a new `RunStore`.
    pub fn new(store_dir: &Utf8Path) -> Result<Self, RunStoreError> {
        let runs_dir = store_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).map_err(|error| RunStoreError::RunDirCreate {
            run_dir: runs_dir.clone(),
            error,
        })?;

        Ok(Self { runs_dir })
    }

    /// Acquires an exclusive lock on the run store.
    ///
    /// This lock should only be held for a short duration.
    pub fn lock_exclusive(&self) -> Result<ExclusiveLockedRunStore<'_>, RunStoreError> {
        let lock_file_path = self.runs_dir.join(RUNS_LOCK_FILE_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_file_path)
            .map_err(|error| RunStoreError::FileLock {
                path: lock_file_path.clone(),
                error,
            })?;

        // These locks are held for a small amount of time (just enough to add a run to the list of
        // runs), so it's fine to block.
        file.lock_exclusive()
            .map_err(|error| RunStoreError::FileLock {
                path: lock_file_path,
                error,
            })?;

        // Now that the file is locked, read the list of runs from disk and add to it.
        let runs_json_path = self.runs_dir.join(RUNS_JSON_FILE_NAME);
        let recorded_runs: RecordedRunList = match std::fs::read_to_string(&runs_json_path) {
            Ok(runs_json) => serde_json::from_str(&runs_json).map_err(|error| {
                RunStoreError::RunListDeserialize {
                    path: runs_json_path,
                    error,
                }
            })?,
            Err(error) => {
                // If the file doesn't exist, that's fine. We'll create it later.
                if error.kind() == io::ErrorKind::NotFound {
                    RecordedRunList::default()
                } else {
                    // TODO: we may want to delete and recreate this file if it's invalid.
                    return Err(RunStoreError::RunListRead {
                        path: runs_json_path.clone(),
                        error,
                    });
                }
            }
        };

        Ok(ExclusiveLockedRunStore {
            runs_dir: &self.runs_dir,
            locked_file: DebugIgnore(file),
            recorded_runs,
        })
    }
}

/// Represents a run store which has been locked for exclusive access.
///
/// The lifetime parameter here is mostly to ensure that this isn't held for longer than the
/// corresponding [`RunStore`].
#[derive(Debug)]
pub struct ExclusiveLockedRunStore<'store> {
    runs_dir: &'store Utf8Path,
    locked_file: DebugIgnore<File>,
    recorded_runs: RecordedRunList,
}

impl<'store> ExclusiveLockedRunStore<'store> {
    // TODO: prune old runs

    /// Creates a run in the run directory, adding it to the list of runs.
    ///
    /// Consumes self, dropping the exclusive lock on the run directory.
    pub fn create_run_recorder(
        mut self,
        run_id: Uuid,
        nextest_version: Version,
        started_at: DateTime<FixedOffset>,
    ) -> Result<RunRecorder, RunStoreError> {
        // Add to the list of runs before creating the directory. This ensures that if creation
        // fails, an empty run directory isn't left behind. (It does mean that there may be spurious
        // entries in the list of runs, which will be dealt with while doing pruning).

        let run = RecordedRun {
            run_id,
            nextest_version,
            started_at,
        };
        self.recorded_runs.runs.push(run);

        // Write to runs.json.
        let runs_json_path = self.runs_dir.join(RUNS_JSON_FILE_NAME);
        let runs_json = serde_json::to_string_pretty(&self.recorded_runs).map_err(|error| {
            RunStoreError::RunListSerialize {
                path: runs_json_path.clone(),
                error,
            }
        })?;

        atomicwrites::AtomicFile::new(&runs_json_path, atomicwrites::AllowOverwrite)
            .write(|file| file.write_all(runs_json.as_bytes()))
            .map_err(|error| RunStoreError::RunListWrite {
                path: runs_json_path,
                error,
            })?;

        // Drop the lock since we're done writing to runs.json. Errors here aren't important because
        // the file will be closed soon anyway.
        _ = self.locked_file.unlock();

        // Now create the run directory.
        let run_dir = self.runs_dir.join(run_id.to_string());

        RunRecorder::new(run_dir)
    }
}

/// Manages the creation of a new run in the store.
#[derive(Debug)]
pub struct RunRecorder {
    store_path: Utf8PathBuf,
    store_writer: StoreWriter,
    log_path: Utf8PathBuf,
    log: DebugIgnore<LineWriter<File>>,
}

impl RunRecorder {
    fn new(run_dir: Utf8PathBuf) -> Result<Self, RunStoreError> {
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
            .write(true)
            .open(&log_path)
            .map_err(|error| RunStoreError::RunLogCreate {
                path: log_path.clone(),
                error,
            })?;

        let log = LineWriter::new(file);

        Ok(Self {
            store_path,
            store_writer,
            log_path,
            log: DebugIgnore(log),
        })
    }

    pub(crate) fn write_meta(
        &mut self,
        cargo_metadata_json: &str,
        test_list: &TestListSummary,
    ) -> Result<(), RunStoreError> {
        let test_list_json = serde_json::to_string(test_list)
            .map_err(|error| RunStoreError::TestListSerialize { error })?;

        self.write_meta_impl(TEST_LIST_JSON_FILE_NAME, test_list_json.as_bytes())?;

        self.write_meta_impl(
            CARGO_METADATA_JSON_FILE_NAME,
            cargo_metadata_json.as_bytes(),
        )?;

        Ok(())
    }

    fn write_meta_impl(&mut self, file_name: &str, bytes: &[u8]) -> Result<(), RunStoreError> {
        // Always use / while joining paths in the zip.
        let path = Utf8PathBuf::from(format!("meta/{file_name}"));
        self.store_writer
            .add_file(path, bytes)
            .map_err(|error| RunStoreError::StoreWrite {
                store_path: self.store_path.clone(),
                error,
            })
    }

    pub(crate) fn write_event(
        &mut self,
        event: TestEventSummary<InMemoryOutput>,
    ) -> Result<(), RunStoreError> {
        let mut cx = SerializeTestEventSummaryContext {
            store_writer: &mut self.store_writer,
        };

        let event = cx
            .handle_test_event(event)
            .map_err(|error| RunStoreError::StoreWrite {
                store_path: self.store_path.clone(),
                error,
            })?;

        let json = serde_json::to_string(&event)
            .map_err(|error| RunStoreError::TestEventSerialize { error })?;
        self.write_log_impl(json.as_bytes())?;
        self.write_log_impl(b"\n")?;

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

    pub(crate) fn finish(mut self) -> Result<(), RunStoreError> {
        // Close and flush the log file.
        self.log
            .flush()
            .map_err(|error| RunStoreError::RunLogFlush {
                path: self.log_path.clone(),
                error,
            })?;

        // Also finish the store file.
        self.store_writer
            .finish()
            .map_err(|error| RunStoreError::StoreWrite {
                store_path: self.store_path.clone(),
                error,
            })?;

        Ok(())
    }
}

#[derive(Debug)]
struct StoreWriter {
    writer: DebugIgnore<ZipWriter<File>>,
    added_files: HashSet<Utf8PathBuf>,
}

impl StoreWriter {
    fn new(store_path: &Utf8Path) -> Result<Self, StoreWriterError> {
        let zip_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(store_path)
            .map_err(|error| StoreWriterError::Create { error })?;
        let writer = ZipWriter::new(zip_file);

        Ok(Self {
            writer: DebugIgnore(writer),
            added_files: HashSet::new(),
        })
    }

    fn add_file(&mut self, path: Utf8PathBuf, contents: &[u8]) -> Result<(), StoreWriterError> {
        if self.added_files.contains(&path) {
            // The file has already been added to the store.
            return Ok(());
        }

        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Zstd);
        self.writer
            .start_file(path.clone(), options)
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

        self.added_files.insert(path);

        Ok(())
    }

    fn finish(mut self) -> Result<(), StoreWriterError> {
        let mut writer = self
            .writer
            .finish()
            .map_err(|error| StoreWriterError::Finish { error })?;

        writer
            .flush()
            .map_err(|error| StoreWriterError::Flush { error })?;

        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct RecordedRunList {
    #[serde(default)]
    pub(crate) runs: Vec<RecordedRun>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct RecordedRun {
    pub(crate) run_id: Uuid,
    pub(crate) nextest_version: Version,
    pub(crate) started_at: DateTime<FixedOffset>,
}

/// Context to create a [`TestEventSummary<ZipStoreOutput>`] out of a
/// [`TestEventSummary<InMemoryOutput>`].
#[derive(Debug)]
struct SerializeTestEventSummaryContext<'a> {
    store_writer: &'a mut StoreWriter,
}

impl<'a> SerializeTestEventSummaryContext<'a> {
    fn handle_test_event(
        &mut self,
        event: TestEventSummary<InMemoryOutput>,
    ) -> Result<TestEventSummary<ZipStoreOutput>, StoreWriterError> {
        Ok(TestEventSummary {
            timestamp: event.timestamp,
            elapsed: event.elapsed,
            kind: self.handle_test_event_kind(event.kind)?,
        })
    }

    fn handle_test_event_kind(
        &mut self,
        kind: TestEventKindSummary<InMemoryOutput>,
    ) -> Result<TestEventKindSummary<ZipStoreOutput>, StoreWriterError> {
        match kind {
            TestEventKindSummary::RunStarted {
                run_id,
                profile_name,
                cli_args,
            } => Ok(TestEventKindSummary::RunStarted {
                run_id,
                profile_name,
                cli_args,
            }),
            TestEventKindSummary::SetupScriptStarted {
                index,
                total,
                script_id,
                command,
                args,
                no_capture,
            } => Ok(TestEventKindSummary::SetupScriptStarted {
                index,
                total,
                script_id,
                command: command.to_string(),
                args: args.to_vec(),
                no_capture,
            }),
            TestEventKindSummary::SetupScriptSlow {
                script_id,
                command,
                args,
                elapsed,
                will_terminate,
            } => Ok(TestEventKindSummary::SetupScriptSlow {
                script_id,
                command: command.to_string(),
                args: args.to_vec(),
                elapsed,
                will_terminate,
            }),
            TestEventKindSummary::SetupScriptFinished {
                index,
                total,
                script_id,
                command,
                args,
                no_capture,
                run_status,
            } => {
                let run_status =
                    self.handle_setup_script_execute_status(&run_status, &script_id)?;
                Ok(TestEventKindSummary::SetupScriptFinished {
                    index,
                    total,
                    script_id,
                    command: command.to_string(),
                    args: args.to_vec(),
                    no_capture,
                    run_status,
                })
            }
            TestEventKindSummary::TestStarted {
                test_instance,
                current_stats,
                running,
                cancel_state,
            } => Ok(TestEventKindSummary::TestStarted {
                test_instance,
                current_stats,
                running,
                cancel_state,
            }),
            TestEventKindSummary::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => Ok(TestEventKindSummary::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            }),
            TestEventKindSummary::TestAttemptFailedWillRetry {
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
            } => {
                let run_status = self.handle_execute_status(run_status, &test_instance)?;
                Ok(TestEventKindSummary::TestAttemptFailedWillRetry {
                    test_instance,
                    run_status,
                    delay_before_next_attempt,
                    failure_output,
                })
            }
            TestEventKindSummary::TestRetryStarted {
                test_instance,
                retry_data,
            } => Ok(TestEventKindSummary::TestRetryStarted {
                test_instance,
                retry_data,
            }),
            TestEventKindSummary::TestFinished {
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
                cancel_state,
            } => {
                let run_statuses = self.handle_execution_statuses(run_statuses, &test_instance)?;
                Ok(TestEventKindSummary::TestFinished {
                    test_instance,
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats,
                    running,
                    cancel_state,
                })
            }
            TestEventKindSummary::TestSkipped {
                test_instance,
                reason,
            } => Ok(TestEventKindSummary::TestSkipped {
                test_instance,
                reason,
            }),
            TestEventKindSummary::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } => Ok(TestEventKindSummary::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            }),
            TestEventKindSummary::RunPaused {
                setup_scripts_running,
                running,
            } => Ok(TestEventKindSummary::RunPaused {
                setup_scripts_running,
                running,
            }),
            TestEventKindSummary::RunContinued {
                setup_scripts_running,
                running,
            } => Ok(TestEventKindSummary::RunContinued {
                setup_scripts_running,
                running,
            }),
            TestEventKindSummary::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
            } => Ok(TestEventKindSummary::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
            }),
        }
    }

    fn handle_setup_script_execute_status(
        &mut self,
        status: &SetupScriptExecuteStatusSummary<InMemoryOutput>,
        script_id: &ScriptId,
    ) -> Result<SetupScriptExecuteStatusSummary<ZipStoreOutput>, StoreWriterError> {
        let digest: String = script_id.to_hex_digest();
        let prefix = format!("script-{digest}");
        Ok(SetupScriptExecuteStatusSummary {
            stdout: self.handle_test_single_output(
                &status.stdout,
                &prefix,
                TestSingleOutputKind::Stdout,
            )?,
            stderr: self.handle_test_single_output(
                &status.stderr,
                &prefix,
                TestSingleOutputKind::Stderr,
            )?,
            result: status.result,
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            env_count: status.env_count,
        })
    }

    fn handle_execution_statuses(
        &mut self,
        statuses: ExecutionStatusesSummary<InMemoryOutput>,
        test_instance: &TestInstanceSummary,
    ) -> Result<ExecutionStatusesSummary<ZipStoreOutput>, StoreWriterError> {
        let statuses = statuses
            .statuses
            .into_iter()
            .map(|status| self.handle_execute_status(status, test_instance))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExecutionStatusesSummary { statuses })
    }

    fn handle_execute_status(
        &mut self,
        status: ExecuteStatusSummary<InMemoryOutput>,
        test_instance: &TestInstanceSummary,
    ) -> Result<ExecuteStatusSummary<ZipStoreOutput>, StoreWriterError> {
        let hex_digest = test_instance.to_hex_digest();
        let prefix = format!("test-{hex_digest}-{}", status.retry_data.attempt);

        // The output should only be persisted if the corresponding file doesn't actually exist
        // already.
        let output = status
            .output
            .map(|output| self.handle_test_output(output, &prefix))
            .transpose()?;

        Ok(ExecuteStatusSummary {
            retry_data: status.retry_data,
            output,
            result: status.result,
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            delay_before_start: status.delay_before_start,
        })
    }

    fn handle_test_output(
        &mut self,
        output: TestOutputSummary<InMemoryOutput>,
        prefix: &str,
    ) -> Result<TestOutputSummary<ZipStoreOutput>, StoreWriterError> {
        match output {
            TestOutputSummary::Split { stdout, stderr } => Ok(TestOutputSummary::Split {
                stdout: self.handle_test_single_output(
                    &stdout,
                    prefix,
                    TestSingleOutputKind::Stdout,
                )?,
                stderr: self.handle_test_single_output(
                    &stderr,
                    prefix,
                    TestSingleOutputKind::Stderr,
                )?,
            }),
            TestOutputSummary::Combined { output } => Ok(TestOutputSummary::Combined {
                output: self.handle_test_single_output(
                    &output,
                    prefix,
                    TestSingleOutputKind::Combined,
                )?,
            }),
            TestOutputSummary::ExecFail {
                message,
                description,
            } => Ok(TestOutputSummary::ExecFail {
                message,
                description,
            }),
        }
    }

    fn handle_test_single_output(
        &mut self,
        output: &InMemoryOutput,
        prefix: &str,
        kind: TestSingleOutputKind,
    ) -> Result<ZipStoreOutput, StoreWriterError> {
        // Write the output to a file, if it is non-empty.
        let file_name = if !output.buf.is_empty() {
            // Always use / while joining paths in the zip.
            let file_name = format!("{prefix}-{kind}");
            let file_path = Utf8PathBuf::from(format!("out/{file_name}"));
            self.store_writer.add_file(file_path, &output.buf)?;

            Some(file_name)
        } else {
            None
        };

        Ok(ZipStoreOutput { file_name })
    }
}

/// A serializable form of a test event.
///
/// Someday this will be stabilized and move to `nextest-metadata`.
///
/// The `O` parameter represents the way test outputs (stdout/stderr) have been stored.
///
/// * First, a `TestEvent` is transformed to one where the output is stored in-memory
///   (`InMemoryOutput`). (This is required because TestEvent isn't 'static and events must be sent
///   across threads.)
/// * Then, the output is stored in the store.zip file. This is represented by `ZipStoreOutput`.
#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct TestEventSummary<O> {
    /// The timestamp of the event.
    pub timestamp: DateTime<FixedOffset>,

    /// The time elapsed since the start of the test run.
    pub elapsed: Duration,

    /// The kind of test event this is.
    pub kind: TestEventKindSummary<O>,
}

impl TestEventSummary<InMemoryOutput> {
    pub(crate) fn from_test_event(event: TestEvent<'_>) -> Self {
        Self {
            timestamp: event.timestamp,
            elapsed: event.elapsed,
            kind: TestEventKindSummary::from_test_event_kind(event.kind),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum TestEventKindSummary<O> {
    #[serde(rename_all = "kebab-case")]
    RunStarted {
        run_id: Uuid,
        profile_name: String,
        cli_args: Vec<String>,
    },
    #[serde(rename_all = "kebab-case")]
    SetupScriptStarted {
        index: usize,
        total: usize,
        script_id: ScriptId,
        command: String,
        args: Vec<String>,
        no_capture: bool,
    },
    #[serde(rename_all = "kebab-case")]
    SetupScriptSlow {
        script_id: ScriptId,
        command: String,
        args: Vec<String>,
        elapsed: Duration,
        will_terminate: bool,
    },
    #[serde(rename_all = "kebab-case")]
    SetupScriptFinished {
        index: usize,
        total: usize,
        script_id: ScriptId,
        command: String,
        args: Vec<String>,
        no_capture: bool,
        run_status: SetupScriptExecuteStatusSummary<O>,
    },
    #[serde(rename_all = "kebab-case")]
    TestStarted {
        test_instance: TestInstanceSummary,
        current_stats: RunStats,
        running: usize,
        cancel_state: Option<CancelReason>,
    },
    #[serde(rename_all = "kebab-case")]
    TestSlow {
        test_instance: TestInstanceSummary,
        retry_data: RetryData,
        elapsed: Duration,
        will_terminate: bool,
    },
    #[serde(rename_all = "kebab-case")]
    TestAttemptFailedWillRetry {
        test_instance: TestInstanceSummary,
        run_status: ExecuteStatusSummary<O>,
        delay_before_next_attempt: Duration,
        failure_output: TestOutputDisplay,
    },
    #[serde(rename_all = "kebab-case")]
    TestRetryStarted {
        test_instance: TestInstanceSummary,
        retry_data: RetryData,
    },
    #[serde(rename_all = "kebab-case")]
    TestFinished {
        test_instance: TestInstanceSummary,
        success_output: TestOutputDisplay,
        failure_output: TestOutputDisplay,
        junit_store_success_output: bool,
        junit_store_failure_output: bool,
        run_statuses: ExecutionStatusesSummary<O>,
        current_stats: RunStats,
        running: usize,
        cancel_state: Option<CancelReason>,
    },
    #[serde(rename_all = "kebab-case")]
    TestSkipped {
        test_instance: TestInstanceSummary,
        reason: MismatchReason,
    },
    #[serde(rename_all = "kebab-case")]
    RunBeginCancel {
        setup_scripts_running: usize,
        running: usize,
        reason: CancelReason,
    },
    #[serde(rename_all = "kebab-case")]
    RunPaused {
        setup_scripts_running: usize,
        running: usize,
    },
    #[serde(rename_all = "kebab-case")]
    RunContinued {
        setup_scripts_running: usize,
        running: usize,
    },
    #[serde(rename_all = "kebab-case")]
    RunFinished {
        run_id: Uuid,
        start_time: DateTime<FixedOffset>,
        elapsed: Duration,
        run_stats: RunStats,
    },
}

impl TestEventKindSummary<InMemoryOutput> {
    fn from_test_event_kind(kind: TestEventKind<'_>) -> Self {
        match kind {
            TestEventKind::RunStarted {
                run_id,
                test_list: _,
                profile_name,
                cli_args,
            } => Self::RunStarted {
                run_id,
                profile_name,
                cli_args,
            },
            TestEventKind::SetupScriptStarted {
                index,
                total,
                script_id,
                command,
                args,
                no_capture,
            } => Self::SetupScriptStarted {
                index,
                total,
                script_id,
                command: command.to_string(),
                args: args.to_vec(),
                no_capture,
            },
            TestEventKind::SetupScriptSlow {
                script_id,
                command,
                args,
                elapsed,
                will_terminate,
            } => Self::SetupScriptSlow {
                script_id,
                command: command.to_string(),
                args: args.to_vec(),
                elapsed,
                will_terminate,
            },
            TestEventKind::SetupScriptFinished {
                index,
                total,
                script_id,
                command,
                args,
                no_capture,
                run_status,
            } => Self::SetupScriptFinished {
                index,
                total,
                script_id,
                command: command.to_string(),
                args: args.to_vec(),
                no_capture,
                run_status: SetupScriptExecuteStatusSummary::from_setup_script_execute_status(
                    run_status,
                ),
            },
            TestEventKind::TestStarted {
                test_instance,
                current_stats,
                running,
                cancel_state,
            } => Self::TestStarted {
                test_instance: TestInstanceSummary::from_test_instance(test_instance),
                current_stats,
                running,
                cancel_state,
            },
            TestEventKind::TestSlow {
                test_instance,
                retry_data,
                elapsed,
                will_terminate,
            } => Self::TestSlow {
                test_instance: TestInstanceSummary::from_test_instance(test_instance),
                retry_data,
                elapsed,
                will_terminate,
            },
            TestEventKind::TestAttemptFailedWillRetry {
                test_instance,
                run_status,
                delay_before_next_attempt,
                failure_output,
            } => Self::TestAttemptFailedWillRetry {
                test_instance: TestInstanceSummary::from_test_instance(test_instance),
                run_status: ExecuteStatusSummary::from_execute_status(run_status),
                delay_before_next_attempt,
                failure_output,
            },
            TestEventKind::TestRetryStarted {
                test_instance,
                retry_data,
            } => Self::TestRetryStarted {
                test_instance: TestInstanceSummary::from_test_instance(test_instance),
                retry_data,
            },
            TestEventKind::TestFinished {
                test_instance,
                success_output,
                failure_output,
                junit_store_success_output,
                junit_store_failure_output,
                run_statuses,
                current_stats,
                running,
                cancel_state,
            } => {
                let run_statuses = ExecutionStatusesSummary::from_execution_statuses(run_statuses);
                Self::TestFinished {
                    test_instance: TestInstanceSummary::from_test_instance(test_instance),
                    success_output,
                    failure_output,
                    junit_store_success_output,
                    junit_store_failure_output,
                    run_statuses,
                    current_stats,
                    running,
                    cancel_state,
                }
            }
            TestEventKind::TestSkipped {
                test_instance,
                reason,
            } => Self::TestSkipped {
                test_instance: TestInstanceSummary::from_test_instance(test_instance),
                reason,
            },
            TestEventKind::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            } => Self::RunBeginCancel {
                setup_scripts_running,
                running,
                reason,
            },
            TestEventKind::RunPaused {
                setup_scripts_running,
                running,
            } => Self::RunPaused {
                setup_scripts_running,
                running,
            },
            TestEventKind::RunContinued {
                setup_scripts_running,
                running,
            } => Self::RunContinued {
                setup_scripts_running,
                running,
            },
            TestEventKind::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
            } => Self::RunFinished {
                run_id,
                start_time,
                elapsed,
                run_stats,
            },
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct SetupScriptExecuteStatusSummary<O> {
    stdout: O,
    stderr: O,
    result: ExecutionResult,
    start_time: DateTime<FixedOffset>,
    time_taken: Duration,
    is_slow: bool,
    // TODO: store env vars here
    env_count: usize,
}

impl SetupScriptExecuteStatusSummary<InMemoryOutput> {
    fn from_setup_script_execute_status(status: SetupScriptExecuteStatus) -> Self {
        Self {
            stdout: InMemoryOutput::from_test_single_output(status.stdout),
            stderr: InMemoryOutput::from_test_single_output(status.stderr),
            result: status.result,
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            env_count: status.env_count,
        }
    }
}

#[derive(Serialize, Debug)]
pub(crate) struct ExecutionStatusesSummary<O> {
    /// This is guaranteed to be non-empty.
    statuses: Vec<ExecuteStatusSummary<O>>,
}

impl<'de, O: Deserialize<'de>> Deserialize<'de> for ExecutionStatusesSummary<O> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let statuses = Vec::<ExecuteStatusSummary<O>>::deserialize(deserializer)?;
        if statuses.is_empty() {
            return Err(serde::de::Error::custom("expected non-empty statuses"));
        }
        Ok(Self { statuses })
    }
}

impl ExecutionStatusesSummary<InMemoryOutput> {
    fn from_execution_statuses(statuses: ExecutionStatuses) -> Self {
        Self {
            statuses: statuses
                .statuses
                .into_iter()
                .map(ExecuteStatusSummary::from_execute_status)
                .collect(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ExecuteStatusSummary<O> {
    retry_data: RetryData,
    output: Option<TestOutputSummary<O>>,
    result: ExecutionResult,
    start_time: DateTime<FixedOffset>,
    time_taken: Duration,
    is_slow: bool,
    delay_before_start: Duration,
}

impl ExecuteStatusSummary<InMemoryOutput> {
    fn from_execute_status(status: ExecuteStatus) -> Self {
        let output = status.output.map(TestOutputSummary::from_test_output);
        Self {
            retry_data: status.retry_data,
            output,
            result: status.result,
            start_time: status.start_time,
            time_taken: status.time_taken,
            is_slow: status.is_slow,
            delay_before_start: status.delay_before_start,
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum TestOutputSummary<O> {
    #[serde(rename_all = "kebab-case")]
    Split { stdout: O, stderr: O },
    #[serde(rename_all = "kebab-case")]
    Combined { output: O },
    #[serde(rename_all = "kebab-case")]
    ExecFail {
        message: String,
        description: String,
    },
}

impl TestOutputSummary<InMemoryOutput> {
    fn from_test_output(output: TestOutput) -> Self {
        match output {
            TestOutput::Split { stdout, stderr } => Self::Split {
                stdout: InMemoryOutput::from_test_single_output(stdout),
                stderr: InMemoryOutput::from_test_single_output(stderr),
            },
            TestOutput::Combined { output } => Self::Combined {
                output: InMemoryOutput::from_test_single_output(output),
            },
            TestOutput::ExecFail {
                message,
                description,
            } => Self::ExecFail {
                message,
                description,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct InMemoryOutput {
    buf: Bytes,
}

impl InMemoryOutput {
    fn from_test_single_output(output: TestSingleOutput) -> Self {
        Self { buf: output.buf }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ZipStoreOutput {
    file_name: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum TestSingleOutputKind {
    Stdout,
    Stderr,
    Combined,
}

impl fmt::Display for TestSingleOutputKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => write!(f, "stdout"),
            Self::Stderr => write!(f, "stderr"),
            Self::Combined => write!(f, "combined"),
        }
    }
}

/// Information about a test instance.
#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct TestInstanceSummary {
    binary_id: RustBinaryId,
    name: String,
    info: RustTestCaseSummary,
}

impl TestInstanceSummary {
    fn from_test_instance(instance: TestInstance<'_>) -> Self {
        Self {
            binary_id: instance.suite_info.binary_id.clone(),
            name: instance.name.to_string(),
            info: instance.test_info.clone(),
        }
    }

    fn to_hex_digest(&self) -> String {
        // Hash the test instance name.
        let mut hasher = Xxh3::new();
        hasher.update(self.binary_id.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.name.as_bytes());
        let digest = hasher.digest();
        // Pad to 16 hex digits (64 bits).
        format!("{:016x}", digest)
    }
}
