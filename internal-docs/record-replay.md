# Record and replay feature for nextest

## Overview

Implement a record and replay feature that captures test run events and outputs into an archive, enabling later inspection and replay of test results.

## Design decisions

1. **Activation mechanism**: Experimental feature + config flag
   - The `record` experimental feature must be enabled via:
     - Environment variable: `NEXTEST_EXPERIMENTAL_RECORD=1`
     - User config: `experimental = ["record"]` in `~/.config/nextest/config.toml`
     - Project config: `experimental = ["record"]` in `.config/nextest.toml`
   - Additionally, `[record] enabled = true` must be set in the config
   - Recording only occurs when BOTH conditions are met
   - No CLI flag initially (keeps it experimental)

2. **Archive location**: Platform-specific cache directory, where `<record-dir>` is `projects/<encoded-workspace>/records/`. The workspace root is used as an absolute, canonical UTF-8 path.

   **Workspace path encoding**: The encoding uses underscore as an escape character to produce a bijective, directory-safe representation:
   - `_` → `__` (escape underscore first)
   - `/` → `_s` (Unix path separator)
   - `\` → `_b` (Windows path separator)
   - `:` → `_c` (Windows drive letter separator)
   - `*` → `_a` (asterisk, invalid on Windows)
   - `"` → `_q` (double quote, invalid on Windows)
   - `<` → `_l` (less than, invalid on Windows)
   - `>` → `_g` (greater than, invalid on Windows)
   - `|` → `_p` (pipe, invalid on Windows)
   - `?` → `_m` (question mark, invalid on Windows)

   Examples:
   - `/home/rain/dev/nextest` → `_shome_srain_sdev_snextest`
   - `C:\Users\rain\dev` → `C_c_bUsers_brain_bdev`
   - `/path_with_underscore` → `_spath__with__underscore`
   - `/weird*path?` → `_sweird_apath_m`

   The encoding is bijective: different workspace paths always produce different encoded strings, and the original path can be recovered from the encoded form.

   **Truncation**: If the encoded path exceeds 96 bytes, it is truncated at a valid UTF-8 boundary and a 6-character hex hash suffix (derived from the full encoded string) is appended to maintain uniqueness.

   Platform-specific cache directories:
   - Linux: `$XDG_CACHE_HOME/nextest/<record-dir>/` or `~/.cache/nextest/<record-dir>/`
   - macOS: `~/Library/Caches/nextest/<record-dir>/`
   - Windows: `%LOCALAPPDATA%\nextest\cache\<record-dir>\`

3. **Retention policy**: Caps on all three dimensions
   - Maximum number of archives (default: 100)
   - Maximum total size (default: 1GB)
   - Maximum age (default: 30 days)
   - LRU eviction when any limit is exceeded

4. **Replay semantics**: Re-view terminal output
   - Primary focus: replay test output as if run just happened
   - Use existing reporter infrastructure for display
   - Machine-readable export is future work

5. **Archive format versioning**: Version from day one
   - Include format version in archive metadata
   - Allows smooth upgrades and forward compatibility

## Existing implementation analysis (nextest2)

The nextest2 branch has a partial implementation in `run_store.rs`:

### Architecture
```
RunStore                     # Manages the runs directory
  └── ExclusiveLockedRunStore  # Holds lock, manages runs.json
       └── RunRecorder         # Writes a single run
            ├── StoreWriter    # Writes to store.zip (zstd-compressed)
            └── LogEncoder     # Writes to run.log.zst (zstd-compressed JSON lines)
```

### Archive structure
```
<runs_dir>/<run_id>/
├── store.zip           # Zstd-compressed zip containing:
│   ├── meta/
│   │   ├── cargo-metadata.json
│   │   ├── test-list.json
│   │   ├── stdout.dict              # Zstd dictionary for stdout decompression
│   │   └── stderr.dict              # Zstd dictionary for stderr decompression
│   └── out/
│       ├── <content_hash>-stdout    # Content-addressed output files
│       ├── <content_hash>-stderr    # (16 hex chars + kind suffix)
│       └── <content_hash>-combined  # Identical outputs are deduplicated
└── run.log.zst         # Zstd-compressed JSON lines of TestEventSummary
```

**Content-addressed output storage**: Output files are named by their content hash (XXH3-64), not by test identity. This enables deduplication: if 1000 stress iterations produce identical output, only one file is stored. Events in `run.log.zst` reference files by their content-addressed names.

### Event flow
```
TestEvent<'a>  →  TestEventSummary<ChildSingleOutput>  →  TestEventSummary<ZipStoreOutput>
     ↓                       ↓                                      ↓
(borrowed)           (uses existing output)              (writes to zip, stores filename)
```

### Key types
- `TestEventSummary<O>`: Serializable form of events, parameterized by output storage
- `ChildSingleOutput`: Output stored in memory with lazy string caching (from `test_output` module)
- `ZipStoreOutput`: Reference to a file in the zip archive
- `ChildExecutionOutputDescription<O>`: Generic output description used in both `ExecuteStatus` and summary types
- `ErrorSummary`: Pre-computed error message for display (serialized to archive)
- `OutputErrorSlice`: Heuristically extracted error slice from output (serialized to archive)

## Proposed implementation plan

### Phase 1: Core recording infrastructure

**New files:**
- `nextest-runner/src/record/mod.rs` - Module root
- `nextest-runner/src/record/store.rs` - Record store management
- `nextest-runner/src/record/recorder.rs` - Recording logic
- `nextest-runner/src/record/summary.rs` - Serializable event types
- `nextest-runner/src/record/cache_dir.rs` - Platform cache directory discovery

**Key changes:**
1. Add `record` module to `nextest-runner/src/lib.rs`
2. Add error types to `nextest-runner/src/errors.rs`
3. Integrate with `StructuredReporter` in `nextest-runner/src/reporter/structured/imp.rs`
4. Add CLI options in `cargo-nextest/src/dispatch.rs`

### Phase 2: Large output handling

**Problem**: Test output can be gigantic (gigabytes of debug output).

**Solutions:**
1. **Per-output size limit**: Truncate outputs exceeding a threshold (e.g., 10MB default)
   - Store truncation marker in the output file
   - Configurable via config file

2. **Streaming writes**: Write outputs to disk as they arrive rather than buffering
   - Requires changes to output capture in executor
   - More complex but handles arbitrary sizes

3. **Reference external files**: For outputs exceeding threshold, store path to temp file
   - Requires coordination with cleanup

**Recommended approach**: Per-output size limit with configurable threshold. Simpler to implement and covers 99% of use cases.

### Phase 3: Storage management

**Record store structure:**
```
<cache_dir>/nextest/records/
├── records.json        # Index of all records with metadata
├── records.lock        # File lock for concurrent access
└── <run_id>/
    ├── store.zip
    └── run.log.zst
```

**Retention implementation:**
```rust
pub struct RecordRetentionPolicy {
    /// Maximum number of records to keep.
    pub max_count: Option<usize>,
    /// Maximum total size of all records in bytes.
    pub max_total_size: Option<u64>,
    /// Delete records older than this duration.
    pub max_age: Option<Duration>,
}
```

**Pruning strategy:**
- Prune at start of new recording (not during)
- Acquire exclusive lock on records.json
- Sort by timestamp, delete oldest until within limits
- Handle concurrent nextest instances gracefully

### Phase 4: Replay and inspection

**CLI interface:**

Store management and inspection:
```
cargo nextest store list             # List all recorded runs
cargo nextest store info             # Show store location and size
cargo nextest store prune            # Prune old runs per retention policy
```

Full replay (separate top-level command):
```
cargo nextest replay [--run-id <id>] # Replay recorded run with full reporter output
```

**Reporter options for `replay`** (same as `cargo nextest run`):
```
--color <WHEN>                       # always, auto, never
--status-level <LEVEL>               # none, fail, retry, slow, leak, pass, skip, all
--final-status-level <LEVEL>         # Same options as --status-level
--failure-output <WHEN>              # immediate, final, immediate-final, never
--success-output <WHEN>              # Same options as --failure-output
--no-capture                         # Simulate no-capture mode (see below)
--no-output-indent                   # Disable output indentation
```

These options control how the recorded events are displayed during replay, allowing users to filter or expand the output differently than the original run. For example, a run recorded with `--failure-output immediate` could be replayed with `--failure-output final` to see failures grouped at the end.

**Simulated no-capture mode**: The `--no-capture` flag (alias: `--nocapture`) is a convenience option that simulates what `--no-capture` does during live test runs. Since recorded output is already captured, true no-capture isn't possible during replay. Instead, `--no-capture` sets:
- `--success-output immediate`
- `--failure-output immediate`
- `--no-output-indent`

This produces output similar to a live no-capture run. Explicit `--success-output`, `--failure-output`, or `--no-output-indent` flags take precedence over the `--no-capture` defaults.

Note: Progress bars are not shown during replay since events are processed instantaneously.

**Design rationale:**
- `replay` is top-level because it's the primary user-facing feature for re-experiencing a run
- `store` subcommands handle inspection and management tasks

**Implementation:**
- `store list`, `store info`, `store prune`: Already implemented
- `replay`: Already implemented with full reporter infrastructure
- Share reporter option parsing via `ReporterCommonOpts`

### Phase 5: Configuration

**Config file options** (`.config/nextest.toml` or `~/.config/nextest/config.toml`):
```toml
experimental = ["record"]

[record]
# Enable recording (required in addition to the experimental feature)
# Default: false
enabled = true

# Where to store records: "cache" (platform cache dir) or custom path
# Default: "cache"
store = "cache"

# Maximum size per output file before truncation
# Default: "10MB"
max-output-size = "10MB"

# Retention policy - all three are enforced
# Default: 100 records
max-records = 100
# Default: "1GB"
max-total-size = "1GB"
# Default: "30d"
max-age = "30d"
```

**Environment variable**:
```bash
# Enable the experimental feature for a single run
# (still requires [record] enabled = true in config)
NEXTEST_EXPERIMENTAL_RECORD=1 cargo nextest run
```

## Edge cases and considerations

### Concurrent runs
- Multiple nextest processes may run simultaneously
- Use file locking for records.json modifications
- Each run gets a unique UUID, no conflicts for run directories

### Interrupted recordings
- If recording is interrupted, the run directory may be incomplete
- On startup, detect incomplete runs and either:
  - Delete them (simpler)
  - Mark them as incomplete in the index

### Cross-platform compatibility
- Archive format should be portable across platforms
- Use forward slashes in zip paths
- UTF-8 for all text content

### Disk full handling
- Gracefully handle ENOSPC errors
- Log warning but don't fail the test run
- Recording is best-effort, not critical path

### Stress tests
- For stress test runs (`--stress`), should each sub-run be recorded separately?
- Proposal: Record as single archive with stress index in event metadata

## Files to modify

### New files
- `nextest-runner/src/record/mod.rs`
- `nextest-runner/src/record/store.rs`
- `nextest-runner/src/record/recorder.rs`
- `nextest-runner/src/record/summary.rs`
- `nextest-runner/src/record/cache_dir.rs`
- `nextest-runner/src/record/reader.rs` (for replay)
- `nextest-runner/src/record/session.rs` (for orchestration)

### Modified files
- `nextest-runner/src/lib.rs` - Add record module
- `nextest-runner/src/errors.rs` - Add record error types
- `nextest-runner/src/reporter/structured/imp.rs` - Integrate RecordReporter
- `nextest-runner/src/reporter/structured/mod.rs` - Export RecordReporter
- `nextest-runner/Cargo.toml` - Add zip dependency if not present (it is: for reuse_build)
- `cargo-nextest/src/dispatch/execution.rs` - Add CLI options and recording activation
- `cargo-nextest/src/lib.rs` - Add show subcommand
- `.config/nextest.toml` defaults - Document new options

## Dictionary compression

Test output (stdout/stderr) compresses poorly with standard zstd because individual outputs are small and lack internal redundancy. Pre-trained zstd dictionaries provide a "virtual prefix" of common patterns, dramatically improving compression.

### Experimental results

Trained on test output from 28 diverse Rust projects (tokio, serde, clap, ripgrep, etc.):

| Category | Without Dict | With Dict | Improvement |
|----------|-------------|-----------|-------------|
| Stdout   | 9.4x        | 15.4x     | **39%**     |
| Stderr   | 7.0x        | 10.8x     | **35%**     |
| Meta     | 11.0x       | 11.1x     | 0.8%        |

For large repos (300+ tests), the improvement averages **48.6%** for test output. Property-based testing (proptest) repos see less benefit (~5-20%) due to random output.

### Dictionary files

Source dictionaries are located in `nextest-runner/src/record/dicts/`:
- `stdout.dict` (8,192 bytes): For test stdout
- `stderr.dict` (4,809 bytes): For test stderr

Meta files (test-list.json, cargo-metadata.json) don't benefit from dictionaries—they're large enough to compress well on their own.

### Storage in archives

Dictionaries are stored directly in each archive under `meta/`:
```
store.zip
├── meta/
│   ├── cargo-metadata.json
│   ├── test-list.json
│   ├── stdout.dict          # Dictionary used for stdout compression
│   └── stderr.dict          # Dictionary used for stderr compression
└── out/
    └── ...
```

This makes archives fully self-contained—no need to track dictionary versions or worry about compatibility when dictionaries are updated. The ~13KB overhead is negligible compared to the compression benefits.

### Usage

```rust
use nextest_runner::record::dicts;

let stdout_dict = dicts::STDOUT;
let stderr_dict = dicts::STDERR;
```

### Training tool

The `zstd-dict` internal tool can retrain dictionaries on new samples:

```bash
# Collect samples by running nextest against various repos
cargo run -p zstd-dict --release -- train --dict-size 8192 --output-dir /tmp/dicts

# Analyze compression improvement
cargo run -p zstd-dict --release -- per-project --dict-dir /tmp/dicts
```

Training data is in `~/dev/nextest-rs/dict-training-repos/`.

## Dependencies

Already available in `nextest-runner`:
- `zip` (with zstd feature)
- `zstd`
- `serde`, `serde_json`
- `chrono`
- `uuid`
- `fs4` (file locking) - need to add
- `atomicwrites`
- `etcetera` (platform directories)

Need to add:
- `fs4` for cross-platform file locking

## Verification

1. **Unit tests**: Test serialization/deserialization of event types
2. **Integration tests**: Record a test run, verify archive contents
3. **Manual testing**:
   - Run `NEXTEST_EXPERIMENTAL_RECORD=1 cargo nextest run` in a test project
   - Verify archive created in cache directory
   - Run `cargo nextest show <run_id>` to replay
   - Verify large outputs are truncated correctly
   - Verify pruning works with multiple runs

## Implementation order

1. **Core infrastructure** (Phase 1): Get basic recording working with env var activation
2. **Config parsing** (Phase 5): Add config file support for recording settings
3. **Large output handling** (Phase 2): Add truncation to prevent disk bloat
4. **Storage management** (Phase 3): Implement retention policies and pruning
5. **Replay** (Phase 4): Add `show` subcommand for viewing recorded runs

The phases can be PRed incrementally, with each phase building on the previous.
