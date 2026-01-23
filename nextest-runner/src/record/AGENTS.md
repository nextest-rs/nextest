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
- `run_id_index.rs`: Efficient prefix lookup for run IDs (jj-style shortest unique prefixes).
- `rerun.rs`: Computes outstanding tests from a recorded run for rerun workflows.
- `session.rs`: High-level session management (setup/finalize lifecycle).
- `dicts/`: Pre-trained zstd dictionaries for output compression.

## Archive format

Each run is stored in a directory named by its UUID (e.g., `runs/550e8400-e29b-41d4-a716-446655440000/`), containing:

### `store.zip`

A zip archive with two types of content:

**Metadata files** in `meta/`:
- `cargo-metadata.json`: Build graph information.
- `test-list.json`: The test list summary.
- `record-opts.json`: Options affecting replay (run mode, etc.).
- `rerun-info.json`: Rerun-specific metadata (only for reruns).
- `stdout.dict`, `stderr.dict`: Zstd dictionaries for self-contained archives.

**Output files** in `out/`:
- Content-addressed naming: `{xxh3_hash_16hex}-{stdout|stderr|combined}`.
- Pre-compressed with zstd dictionaries.
- Deduplication via content addressing (identical outputs share a file).

### `run.log.zst`

A zstd-compressed JSON Lines file containing test events. Each line is a `TestEventSummary<ZipStoreOutput>` that references output files in the zip by name.

## Format versions

There are **two separate format versions**:

1. **`RUNS_JSON_FORMAT_VERSION`** (in `format.rs`): Version of `runs.json.zst` format. Controls backward/forward compatibility of the run list itself.

2. **`RECORD_FORMAT_VERSION`** (in `format.rs`): Version of the archive format (`store.zip` + `run.log.zst`). Stored per-run in `runs.json.zst` to enable checking replayability without opening archives.

**Write permission model**: When reading `runs.json.zst`, if its format version is newer than the current nextest supports, writing is denied (`RunsJsonWritePermission::Denied`) to prevent data loss. Reading always proceeds.

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

## Rerun chain model

Reruns form a chain via `parent_run_id`:
```
initial_run -> rerun_1 -> rerun_2 -> ...
```

Each rerun stores `RerunInfo` containing:
- `parent_run_id`: Immediate parent.
- `root_info`: Information from the chain root (build scope args, original run ID).
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

Used to find the most recent replayable run for `latest` selection.
