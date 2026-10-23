# Record module

This module provides recording infrastructure for nextest runs: capturing test events and outputs to disk for later inspection, replay, and rerun workflows.

## Architecture overview

The recording system has three main components:

1. **Run store** (`store.rs`): Manages the directory containing all recorded runs. Handles locking, the master run list (`runs.json.zst`), and metadata about each run.

2. **Recorder** (`recorder.rs`): Writes a single run's data to disk during test execution. Creates the archive and event log.

3. **Reader** (`reader.rs`): Reads a recorded run from disk for replay or inspection.

Supporting modules:
- `format.rs`: Serialization types and constants shared between recorder and reader.
- `summary.rs`: Serializable event types that mirror runtime `TestEvent` types.
- `replay.rs`: Converts recorded events back to `TestEvent` for display.
- `retention.rs`: Retention policies and pruning logic.
- `run_id_index.rs`: Efficient prefix lookup for run IDs (jj-style shortest unique prefixes), plus `RunIdOrRecordingSelector` for CLI commands that accept either.
- `rerun.rs`: Computes outstanding tests from a recorded run for rerun workflows.
- `session.rs`: High-level session management (setup/finalize lifecycle).
- `portable.rs`: Portable archive creation and reading for sharing runs across machines.
- `chrome_trace.rs`: Converts recorded events to Chrome Trace Event Format JSON for visualization in Perfetto UI or `chrome://tracing`.
- `dicts/`: Pre-trained zstd dictionaries for output compression.

## Archive format

Each run is stored in a directory named by its UUID (e.g., `runs/550e8400-e29b-41d4-a716-446655440000/`), containing:

### `store.zip`

A zip archive with two types of content:

**Metadata files** in `meta/`:
- `cargo-metadata.json`: Build graph information.
- `test-list.json`: The test list summary.
- `record-opts.json`: Options affecting replay (run mode, etc.).
- `build-scope.json`: The build scope this run resolved (store format 3.0 and later). Used only by reruns; see [Build scope storage](#build-scope-storage).
- `rerun-info.json`: Rerun-specific metadata (only for reruns).
- `stdout.dict`, `stderr.dict`: Zstd dictionaries for self-contained archives.

**Output files** in `out/`:
- Content-addressed naming: `{xxh3_hash_16hex}-{stdout|stderr|combined}`.
- Pre-compressed with zstd dictionaries.
- Deduplication via content addressing (identical outputs share a file).

### `run.log.zst`

A zstd-compressed JSON Lines file containing test events. Each line is a `TestEventSummary<ZipStoreOutput>` that references output files in the zip by name.

## Portable recording format

Portable archives package a single recorded run into a self-contained zip file for sharing across machines. Created via `cargo nextest store export`, they can be read via `cargo nextest replay -R archive.zip`, `cargo nextest run --rerun archive.zip`, or `cargo nextest store info archive.zip`.

The outer zip contains:
- `manifest.json`: Run metadata and format versions (`PortableManifest`).
- `store.zip`: The inner store archive (same format as the on-disk `store.zip`).
- `run.log.zst`: The event log (same format as on-disk).

Key types:
- `PortableRecording`: Reader for portable recordings. Validates format versions on open.
- `PortableRecordingWriter`: Creates portable recordings from a recorded run.
- `PortableStoreReader`: Implements `StoreReader` for reading from archives.

Format versions:
- `PORTABLE_RECORDING_FORMAT_VERSION`: Version of the outer archive structure.
- The inner store uses `STORE_FORMAT_VERSION` (same as on-disk stores).

Both versions use major/minor semantics with `check_readable_by()` for compatibility checking.

## StoreReader trait

`StoreReader` abstracts over reading from either on-disk stores (`RecordReader`) or portable recordings (`PortableStoreReader`). This enables replay and rerun code to work with both sources transparently.

Key methods:
- `read_cargo_metadata()`, `read_test_list()`, `read_record_opts()`: Read metadata.
- `read_build_scope()`: Read `meta/build-scope.json` (required for store format 3.0 and later).
- `read_rerun_info()`: Read rerun chain info (returns `None` for non-reruns).
- `load_dictionaries()`: Must be called before `read_output()`.
- `read_output(file_name)`: Read decompressed test output.

## RunFilesExist trait

`RunFilesExist` abstracts checking for required run files (`store.zip`, `run.log.zst`). Implemented by both `StoreRunFiles` (on-disk) and `PortableRecording`. Used by `RecordedRunInfo::check_replayability()`.

## Format versions

There are **two separate format versions**:

1. **`RUNS_JSON_FORMAT_VERSION`** (in `format.rs`): Version of `runs.json.zst` format (newtype `RunsJsonFormatVersion`). Controls backward/forward compatibility of the run list itself.

2. **`STORE_FORMAT_VERSION`** (in `format.rs`): Version of the archive format (`store.zip` + `run.log.zst`). This is a `StoreFormatVersion` combining a major version (`StoreFormatMajorVersion`) and minor version (`StoreFormatMinorVersion`). Stored per-run in `runs.json.zst` to enable checking replayability without opening archives. Major versions must match exactly; minor versions allow reading older archives but not newer ones.

**Write permission model**: When reading `runs.json.zst`, if its format version is newer than the current nextest supports, writing is denied (`RunsJsonWritePermission::Denied`) to prevent data loss. Reading always proceeds.

`StoreFormatMajorVersion::too_old_guidance` maps each older major version to a `TooOldRecordingGuidance`:

- `UseOlderNextest`: the "use nextest <= X" message, naming the last nextest release that can read the recording.
- `Discard`: for majors before 3. The message recommends discarding the recording rather than switching to an older nextest. When a newer nextest rewrites `runs.json.zst`, it drops these runs' `build-scope-args`, so an older nextest could rerun chain-root runs with Cargo's default build scope. (Reruns of reruns are unaffected, since they carry the root's scope in `rerun-info.json`.)
- `Unknown`: no entry exists for this major version, so the message gives no advice.

Add an entry whenever the major version is bumped; `test_too_old_guidance_known_for_all_older_majors` fails if one is missing.

## Build scope storage

The build scope (package, target, and feature selection) is stored as a typed `BuildScope` (from `crate::build_scope`), in each run's `store.zip` only:

- `meta/build-scope.json` (`BUILD_SCOPE_JSON_PATH`): a serialized `RecordedBuildScope` (`{"build-kind": ..., "scope": {...}}`, both fields required), written by `RecordSession::setup` (via `RunRecorder::write_build_scope`) for every run, from `RecordSessionConfig::build_scope`. `scope` is the build scope this run resolved (cargo-nextest's `BaseApp::resolve_build_scope`). That's the scope from the run's command line, or, for a rerun without build scope arguments, the scope inherited from the chain root. Read via `StoreReader::read_build_scope`, but only for the chain root: a rerun of a rerun takes the root's scope from the parent's `rerun-info.json`.
- `RerunRootInfo::build_scope` (`meta/rerun-info.json`, as a `build-scope` object): required. A copy of the root run's `RecordedBuildScope` (same type and shape as its `build-scope.json`), so the root's build kind travels with the chain even if the root is pruned. Reruns inherit its `scope` when the user passes no build scope arguments.

`build-kind` (`BuildKind`) records how the run got its binaries: `cargo` when nextest built the binaries with Cargo (`cargo test --no-run`, or `cargo bench --no-run` for benchmarks) using the scope, including with `--cargo-metadata` alone; and `reused` with `--archive-file` or `--binaries-metadata`, where the scope wasn't used to build anything. cargo-nextest's `BaseApp::build_binary_list_with_scope` returns it alongside the binary list: `Reused` from its reuse early return, `Cargo` after building. Since build scope arguments can't be combined with `--archive-file` or `--binaries-metadata`, a chain root that reused a build always records an empty scope, so a rerun of it inherits `{}`. Nothing reads the build kind for behavior yet; it's recorded so that a future version can treat chains rooted at reused builds differently without a format change.

The scope is deliberately not part of `RecordOpts`: replay reads `record-opts.json` but never `build-scope.json`, so a build scope that a later nextest can't read only breaks rerunning that run, never replaying it.

`runs.json.zst` and the portable manifest don't hold a build scope. Entries written before store format 3.0 had an unstructured `build-scope-args` field; serde ignores it on read, and rewriting `runs.json.zst` drops it (no legacy support, by design). Those runs aren't replayable (major version 2), but they still load, list, and prune. Since nextest 0.9.149 can still read a newer `runs.json.zst` (only writes are denied), it may rerun its own 2.x runs with Cargo's default scope after a newer nextest has rewritten the index. This is an accepted risk.

When a rerun's parent is the chain root (it has no `rerun-info.json`), `ComputedRerunInfo::compute` and `compute_from_archive` build `RerunRootInfo` from the parent's `meta/build-scope.json`. If that file can't be read (missing or malformed: on a 3.x run, the metadata was edited or corrupted), they return a `RecordReadError`, the same as any other read failure. cargo-nextest reports every failure to read a rerun's parent run as `ExpectedError::RerunParentReadError`, which names the run (and the portable recording, if any), carries the `RecordReadError` as its cause, and suggests starting a new rerun chain.

`BuildScope` ignores unknown fields on read. Adding a field to it requires bumping the `STORE_FORMAT_VERSION` minor, so that older readers refuse to rerun with a wider scope than the one recorded. Adding a `BuildKind` variant also requires bumping the `STORE_FORMAT_VERSION` minor: older readers fail to deserialize an unknown `build-kind`, and the bump makes them report the run as too new instead.

## Locking model

The run store uses file locking on `runs.lock`:

- **Shared lock** (`lock_shared`): For read-only operations (listing runs, reading metadata). Multiple readers can hold simultaneously.

- **Exclusive lock** (`lock_exclusive`): For mutations (creating runs, completing runs, pruning). Exclusive with both shared and exclusive locks.

The lock is acquired with retries (100ms intervals, 5s timeout) to handle brief contention and NFS-like filesystems where locking may be unreliable.

**Critical**: The exclusive lock should be held only briefly—just long enough to add a run entry and create its directory. The recorder then writes independently without holding the lock.

## Content-addressed output storage

Output files use XXH3 hashing for content addressing:

```
OutputFileName::from_content(content, OutputKind::Stdout)
// -> "a1b2c3d4e5f6789a-stdout"
```

Benefits:
- **Deduplication**: Stress runs with identical outputs store only one copy.
- **Security**: `OutputFileName` validates format during deserialization to prevent path traversal.
- **Compression**: Dictionary selection based on suffix (`-stdout`, `-stderr`, `-combined`).

## Zstd dictionary compression

The `dicts/` module contains pre-trained dictionaries that provide ~40-60% compression improvement for typical test output:

- `STDOUT`: For stdout and combined output.
- `STDERR`: For stderr.

Dictionaries are embedded in each archive (`meta/stdout.dict`, `meta/stderr.dict`) to make archives self-contained. When reading, dictionaries are loaded from the archive (not the embedded constants) to ensure version compatibility.

## Retention and pruning

`RecordRetentionPolicy` enforces limits on:
- `max_count`: Maximum number of runs.
- `max_total_size`: Maximum total compressed size.
- `max_age`: Maximum age since last use.

Pruning is LRU-based using `last_written_at`, which is updated when:
- A run is created.
- A run completes.
- A rerun references a parent run.

**Implicit pruning**: During recording, pruning occurs automatically if:
- More than 1 day since last prune, OR
- Any limit exceeded by 1.5x.

**Orphan cleanup**: Directories that exist on disk but aren't in `runs.json.zst` are deleted during pruning. This handles crashes between directory creation and run completion.

## Run ID index

`RunIdIndex` enables jj-style shortest unique prefix display:

```rust
let index = RunIdIndex::new(&runs);
let prefix = index.shortest_unique_prefix(run_id);
// prefix.prefix = "5" (highlighted portion)
// prefix.rest = "50e8400-e29b-41d4-a716-446655440000"
```

Implementation uses sorted neighbor comparison rather than a trie—simpler and sufficient for expected run counts.

### RunIdOrRecordingSelector

CLI commands that can consume runs from either the store or a portable recording use `RunIdOrRecordingSelector`. Parsing logic:
- Strings ending in `.zip` → `RecordingPath(path)`
- Strings containing `/` or `\` → `RecordingPath(path)` (handles process substitution paths like `/proc/self/fd/11` or `/dev/fd/5`, and relative paths like `./recording`)
- Everything else → `RunId(RunIdSelector)` (parses as `latest` or hex prefix)

Bare filenames without separators or `.zip` are *not* treated as paths (by design), so typos like `latets` produce clear error messages rather than confusing "file not found" errors.

This enables commands like `cargo nextest replay -R path/to/archive.zip` and `cargo nextest replay -R <(curl url)` to work alongside `cargo nextest replay -R latest`.

### Non-seekable input handling

`PortableRecording::open` handles non-seekable inputs (pipes from process substitution) via `ensure_seekable`. Detection is platform-specific: on Windows, `GetFileType` is used because `SetFilePointerEx` spuriously succeeds on named pipe handles; on Unix, `lseek` reliably fails with `ESPIPE`. Non-seekable inputs are spooled to an anonymous temp file via `camino_tempfile::tempfile()`, with a 4 GiB safety limit. The temp file fits into the existing `ArchiveReadStorage = Either<File, Cursor<Vec<u8>>>` without type changes.

## Rerun chain model

Reruns form a chain via `parent_run_id`:
```
initial_run -> rerun_1 -> rerun_2 -> ...
```

Each rerun stores `RerunInfo` containing:
- `parent_run_id`: Immediate parent.
- `root_info`: Information from the chain root (build scope, original run ID).
- `test_suites`: Map of binary ID → passing/outstanding test sets.

**Outstanding test computation** (`rerun.rs`):
- Tests that failed or weren't seen → outstanding.
- Tests that passed or were skipped due to prior pass → passing.
- Explicitly skipped tests carry forward their previous status.

The `compute_outstanding_pure` function is designed for property-based testing via the `TestListInfo` trait.

## Replay

`ReplayContext` converts `TestEventSummary<ZipStoreOutput>` back to `TestEvent` for display:

1. Test instances must be registered first (`register_test`).
2. Output files are read from the archive on demand.
3. Events are passed through the normal `DisplayReporter`.

`ReplayReporter` wraps `DisplayReporter` with replay-specific header output.

## Chrome trace export

`chrome_trace.rs` converts recorded events to [Chrome Trace Event Format](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU) JSON, viewable in [Perfetto UI](https://ui.perfetto.dev) or Chrome's `chrome://tracing`. Invoked via `cargo nextest store export-chrome-trace`.

### Public API

- `convert_to_chrome_trace(nextest_version, events, group_by, message_format) -> Result<Vec<u8>, ChromeTraceError>`: Converts an iterator of `TestEventSummary<RecordingSpec>` to JSON bytes. Operates directly on the storage format; no replay infrastructure needed.
- `ChromeTraceGroupBy`: Controls how tests are grouped in the output (`Binary` or `Slot`).
- `ChromeTraceMessageFormat`: Controls JSON serialization (`Json` or `JsonPretty`).
- `ChromeTraceError`: Read errors, missing start events, or JSON serialization errors.

### Dimension mapping

- **pid**: Depends on the grouping mode. In `Binary` mode, a numeric ID per `RustBinaryId` (groups tests by binary); in `Slot` mode, all tests share a single pid (`ALL_TESTS_PID = 2`). In both modes, pid 0 is reserved for run lifecycle events and pid 1 for setup scripts. Test binary pids start at 2.
- **tid**: `global_slot` from `TestSlotAssignment` (shows parallelism). A `TID_OFFSET` of 10,000 is added to ensure test and script tids never collide with process pids. Setup scripts use their script index + offset. Stress sub-runs use a dedicated tid.
- **name**: In `Binary` mode, the test name alone (binary is encoded in the pid). In `Slot` mode, prefixed with the binary ID for disambiguation. Script IDs and `"test run"` / `"sub-run"` are used for other event types.
- **cat**: `"test"`, `"setup-script"`, `"stress"`, or `"run"`.

### B/E events instead of X events

The converter uses B (begin) / E (end) duration event pairs instead of X (complete) events. This design choice enables splitting spans around pause/resume boundaries: when `RunPaused` is seen, E events close every open span, and when `RunContinued` is seen, matching B events reopen them. The result is a visible gap in the timeline during pauses.

### State tracking

`ChromeTraceConverter` tracks open spans to support pause/resume:
- `slot_assignments: HashMap<OwnedTestInstanceId, TestSlotAssignment>`: An entry means the test has an open B event.
- `running_scripts: BTreeMap<usize, String>`: An entry means the script has an open B event. BTreeMap ensures deterministic iteration order.
- `stress_subrun_open: bool`: Whether a stress sub-run span is currently open.
- `run_bar_state: RunBarState`: `Open`, `Paused`, or `Closed` for the run lifecycle bar.

On pause, `close_all_open_spans` emits E events for the run bar, stress sub-run, all tests, and all scripts. On resume, `reopen_all_spans` emits corresponding B events.

### Missing start event handling

Events that reference a test, script, or stress sub-run without a prior start event produce a hard error:
- `TestSlow`, `TestAttemptFailedWillRetry`, or `TestFinished` without a prior `TestStarted` returns `ChromeTraceError::MissingTestStart`.
- `SetupScriptSlow` without a prior `SetupScriptStarted` returns `ChromeTraceError::MissingScriptStart`.
- `StressSubRunFinished` without a prior `StressSubRunStarted` returns `ChromeTraceError::MissingStressSubRunStart`.

This means corrupt logs where start events are missing in the middle will fail the entire conversion rather than producing a partial trace. (Truncated logs, where events at the end are missing, are handled gracefully.)

### Additional trace features

- **Counter events** (`"C"` phase): Track `running_tests` and `running_scripts` counts, emitted at test start/finish and script start/finish.
- **Flow events** (`"s"`/`"f"` phases): Connect failed test attempts to their retries with arrows.
- **Metadata events** (`"M"` phase): Set `process_name`, `process_sort_index`, `thread_name`, and `thread_sort_index` for deterministic ordering in Perfetto.
- **`otherData`**: Session-level metadata (nextest version, run ID, profile, CLI args, stress condition).

### Error types

- `ChromeTraceError::ReadError`: Wraps `RecordReadError` from the event iterator.
- `ChromeTraceError::MissingTestStart`: An event referenced a test with no prior `TestStarted` (corrupt or truncated log).
- `ChromeTraceError::MissingScriptStart`: A `SetupScriptSlow` referenced a script with no prior `SetupScriptStarted`.
- `ChromeTraceError::MissingStressSubRunStart`: A `StressSubRunFinished` arrived with no prior `StressSubRunStarted`.
- `ChromeTraceError::SerializeError`: Wraps `serde_json::Error` from final serialization.

### Serde note

`ChromeTraceArgs` uses `#[serde(untagged)]` for serialization only (the type is never deserialized), so the AGENTS.md guideline about untagged deserializers does not apply.

## Testing patterns

### Property-based tests for rerun logic

`rerun.rs` contains extensive proptest-based testing:

- **Model-based oracle**: `RerunModel` describes a sequence of runs with test lists and outcomes.
- **Decision table oracle**: Each test's fate is determined independently via `decide_test_outcome`.
- **SUT vs oracle**: The actual implementation is compared against the oracle.

Key properties tested:
- Passing and outstanding sets are always disjoint.
- Matching tests with definitive outcomes are always tracked.
- Stress run accumulation: any failure → overall failure.

### Snapshot tests for serialization

`format.rs` uses insta snapshots for `RecordedRun` serialization formats:
- `test_recorded_run_serialize_incomplete`
- `test_recorded_run_serialize_completed`
- etc.

## Error handling

Errors are split into:
- `RunStoreError`: Store-level errors (locking, I/O, format mismatch).
- `RecordReadError`: Reading errors (missing files, decompression, parsing).
- `RecordPruneError`: Pruning errors (collected but don't stop operation).
- `ReplayConversionError`: Replay errors (test not found, invalid data).
- `ChromeTraceError`: Chrome trace conversion errors (read errors or serialization).

Finalization errors (`RecordFinalizeWarning`) are non-fatal—the recording itself completed.

## Critical implementation notes

### Size limits for decompression

`RecordReader::read_archive_file` enforces `MAX_MAX_OUTPUT_SIZE` limits:
- Checks claimed size in ZIP header before allocation.
- Uses `take()` during decompression to guard against spoofed headers.

### Output truncation

Large outputs are truncated during recording (`truncate_output`):
- Keeps head and tail portions.
- Inserts marker: `\n\n... [truncated N bytes] ...\n\n`
- `ZipStoreOutput::Truncated` records original size.

### Stress run outcome accumulation

For stress runs, multiple `TestFinished` events occur for the same test. Outcome accumulation uses `entry().and_modify()` to only "upgrade" from Passed to Failed, never downgrade:

```rust
outcomes
    .entry(test_instance.clone())
    .and_modify(|existing| {
        if outcome == TestOutcome::Failed {
            *existing = TestOutcome::Failed;
        }
    })
    .or_insert(outcome);
```

### Replayability checking

`RecordedRunInfo::check_replayability` returns:
- `Replayable`: Safe to replay.
- `NotReplayable(reasons)`: Blocking issues (format too new, missing files, unknown status).
- `Incomplete`: Might be usable but needs verification.

Used to display replayability status and identify issues that would prevent replay.
