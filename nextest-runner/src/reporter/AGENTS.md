# Reporter module

This document provides context for the `reporter` module in `nextest-runner`. It covers architecture, patterns, and conventions specific to this module that supplement the root `AGENTS.md`.

## Architecture overview

The reporter subsystem transforms test execution events into human-readable terminal output and machine-readable formats. It follows an event-driven architecture where the runner produces `TestEvent`s and the reporter consumes them.

### Core flow

```
TestRunner → TestEvent → Reporter → {DisplayReporter, EventAggregator, StructuredReporter}
                                           ↓               ↓                    ↓
                                    terminal/stderr    JUnit XML         libtest JSON,
                                                       to disk           recording archive
```

### Key types

- **`Reporter<'a>`** (`imp.rs`): Main entry point that orchestrates all reporting. Combines `DisplayReporter`, `EventAggregator`, and `StructuredReporter`.
- **`ReporterEvent<'a>`** (`events.rs`): Root event type with `Tick` (periodic refresh) and `Test(Box<TestEvent<'a>>)` variants.
- **`TestEvent<'a>`** (`events.rs`): Test event with timestamp, elapsed time, and `TestEventKind`.
- **`TestEventKind<'a>`** (`events.rs`): ~30 event variants covering the full test lifecycle.

## Module structure

```
reporter/
├── mod.rs                    # Public API, re-exports
├── imp.rs                    # Reporter, ReporterBuilder
├── events.rs                 # TestEvent, TestEventKind, RunStats, ExecutionStatuses (~2600 lines)
├── error_description.rs      # UnitErrorDescription, heuristic error extraction
├── helpers.rs                # Styles, print_lines_in_chunks, highlight_end
├── test_helpers.rs           # Proptest strategies for testing
│
├── displayer/                # Human-friendly terminal output
│   ├── mod.rs
│   ├── imp.rs                # DisplayReporter, DisplayReporterImpl (~1200 lines)
│   ├── status_level.rs       # StatusLevel, FinalStatusLevel, output decision logic
│   ├── unit_output.rs        # TestOutputDisplay, ChildOutputSpec, ANSI handling
│   ├── progress.rs           # ProgressBarState, OSC 9;4 terminal progress
│   └── formatters.rs         # Duration formatters, skip counts, final warnings
│
├── aggregator/               # Disk-based metadata output
│   ├── mod.rs
│   ├── imp.rs                # EventAggregator
│   └── junit.rs              # MetadataJunit (JUnit XML via quick_junit)
│
└── structured/               # Machine-readable formats
    ├── mod.rs
    ├── imp.rs                # StructuredReporter
    ├── libtest.rs            # LibtestReporter (line-by-line JSON, ~920 lines)
    └── recorder.rs           # RecordReporter (archive recording via background thread)
```

## Event system

### Event lifecycle

Events flow through these stages:

1. **Run-level**: `RunStarted` → (`StressSubRunStarted` → ... → `StressSubRunFinished`)* → `RunFinished`
2. **Setup scripts**: `SetupScriptStarted` → `SetupScriptSlow`? → `SetupScriptFinished`
3. **Tests**: `TestStarted` → `TestSlow`? → (`TestAttemptFailedWillRetry` → `TestRetryStarted`)* → `TestFinished`
4. **Control flow**: `RunBeginCancel`, `RunBeginKill`, `RunPaused`, `RunContinued`
5. **Interactive**: `InfoStarted`, `InfoResponse`, `InfoFinished`, `InputEnter`

### Key event conventions

- Events carry `stress_index: Option<StressIndex>` for stress test tracking.
- `TestInstanceId<'a>` borrows from the test list; `OwnedTestInstanceId` is used when ownership is needed.
- `current_stats: RunStats` is included in many events for incremental progress display.
- `running: usize` tracks concurrent test count for progress bar updates.

### Output type parameter

Many event types are generic over `O`, the output storage type:
- `ChildSingleOutput`: Runtime output with byte buffers.
- Used throughout `ExecuteStatus<O>`, `ExecutionStatuses<O>`, etc.

## Output display configuration

### TestOutputDisplay

`TestOutputDisplay` (`unit_output.rs`) controls when test output is shown:

```rust
pub enum TestOutputDisplay {
    Immediate,      // Show on completion (default for failures)
    ImmediateFinal, // Show immediately AND at end
    Final,          // Only show at run end
    Never,          // Don't show output
}
```

Key methods:
- `is_immediate()`: True for `Immediate` or `ImmediateFinal`.
- `is_final()`: True for `Final` or `ImmediateFinal`.

This enum is combined with status levels to determine actual output behavior.

## Status levels

Status levels control output verbosity, similar to log levels. They are **incremental**: higher levels include all lower levels.

### During-run levels (`StatusLevel`)

```
None < Fail < Retry < Slow < Leak < Pass < Skip < All
```

### Final output levels (`FinalStatusLevel`)

```
None < Fail < Flaky < Slow < Skip < Leak < Pass < All
```

Note the differences:
- `Flaky` replaces `Retry` for final output (different semantics).
- `Skip` is prioritized differently (before `Leak` in final, after `Pass` during run).

### Output decision logic

The complex `StatusLevels::compute_output_on_test_finished()` method (`status_level.rs:90-175`) handles:
- Whether to write the status line.
- Whether to show output immediately.
- Whether to store output for final display.

This logic accounts for cancellation scenarios (interrupt, signal, test failure immediate) and avoids duplicate output spam. The decision table is documented inline with extensive property-based tests.

## Display reporter

### Progress bar management

`ProgressBarState` (`progress.rs`) manages the indicatif progress bar with:
- **Stacked hide states**: `hidden_no_capture`, `hidden_run_paused`, `hidden_info_response`.
- **Running test tracking**: `Vec<RunningTest>` with status (Running, Slow, Delay, Retry).
- **Chunked output**: `print_lines_in_chunks()` prevents terminal overwhelm during large output bursts.
- **OSC 9;4 progress**: Terminal progress codes for supported terminals (Windows Terminal, iTerm, WezTerm, Ghostty).

The refresh rate is intentionally set to 1 Hz (`PROGRESS_REFRESH_RATE_HZ`) to batch updates efficiently.

### Output formatting

- **Indentation**: `ChildOutputSpec` defines headers and indent levels for stdout/stderr/combined output.
- **ANSI handling**: When colorized, output is shown with ANSI escapes intact plus a reset. When not colorized, ANSI escapes are stripped via `strip_ansi_escapes`.
- **Highlight extraction**: `TestOutputErrorSlice` heuristically extracts panic messages, error strings, and should-panic failures from output.
- **Per-line coloring**: For CI environments that reset colors per line, highlights are re-applied for each line.

### Styles

`Styles` (`helpers.rs`) centralizes color configuration:
- `pass`: green bold
- `fail`: red bold
- `retry`: magenta bold
- `skip`: yellow bold
- `script_id`: blue bold
- `run_id_prefix`/`run_id_rest`: For highlighting unique run ID prefixes

## Structured reporters

### Libtest reporter

`LibtestReporter` (`libtest.rs`) emits line-by-line JSON compatible with `rustc --format json`:
- Versioned format (major 0 = unstable, minor versions track libtest changes).
- Emits per-binary suite blocks to match cargo test's serial execution model.
- Optional `nextest` subobject with additional metadata.
- Handles `#[should_panic]` message mismatches.

### Record reporter

`RecordReporter` (`recorder.rs`) writes events to disk archives:
- Runs in a **separate thread** with bounded channel (128 events) for backpressure.
- Converts events to `TestEventSummary` (serializable form) before sending.
- Non-recordable events (interactive/informational) are silently skipped.
- Thread panics are caught and converted to `RecordReporterError`.

## Statistics and execution tracking

### RunStats

`RunStats` (`events.rs`) tracks comprehensive test run statistics:

```rust
pub struct RunStats {
    pub initial_run_count: usize,      // Total tests expected
    pub finished_count: usize,         // Tests completed
    pub setup_scripts_*: usize,        // Setup script counters
    pub passed: usize,                 // Includes slow, timed_out, flaky, leaky
    pub passed_slow: usize,            // Subset of passed
    pub flaky: usize,                  // Passed on retry
    pub failed: usize,                 // Includes leaky_failed
    pub failed_timed_out: usize,       // Timed out and failed
    pub leaky: usize,                  // Passed but leaked handles
    pub leaky_failed: usize,           // Failed due to leak
    pub exec_failed: usize,            // Failed to start
    pub skipped: usize,
    pub cancel_reason: Option<CancelReason>,
}
```

Key methods:
- `has_failures()`: Returns true if any failures occurred.
- `failed_count()`: Sum of `failed + exec_failed + failed_timed_out`.
- `summarize_final()`: Returns `FinalRunStats` enum for exit code determination.
- `on_test_finished()`: Updates stats based on final execution status.

### ExecutionStatuses

`ExecutionStatuses<O>` (`events.rs`) tracks all attempts for a single test:
- **Invariant**: Always non-empty (at least one attempt).
- `last_status()`: The final attempt's status (used for overall result).
- `describe()`: Returns `ExecutionDescription` enum (Success, Flaky, Failure).

The `ExecutionDescription` determines status levels:
- **Success**: Single passing run.
- **Flaky**: Multiple runs, final passed.
- **Failure**: All runs failed.

### CancelReason ordering

`CancelReason` has an intentional ordering for output suppression logic:

```rust
SetupScriptFailure < TestFailure < TestFailureImmediate < ReportError < GlobalTimeout < Signal < Interrupt < SecondSignal
```

Higher values indicate more urgent cancellation; interrupt and signal hide output to avoid spam.

## JUnit XML reporter

`MetadataJunit` (`aggregator/junit.rs`) generates JUnit XML via the `quick_junit` crate:

- Test suites map to test binaries.
- Setup scripts are included as test cases with `nextest-kind: setup-script` property.
- Reruns are tracked via `rerun` elements in test cases.
- Output storage is configurable per success/failure via `JunitConfig`.
- Timestamps use ISO 8601 format.

## Error description

`UnitErrorDescription` (`error_description.rs`) aggregates errors from test/script execution:
- `all_error_list()`: All errors.
- `exec_fail_error_list()`: Start and output errors only.
- `child_process_error_list()`: Abort and output errors (child-generated).

Heuristic extraction uses regex patterns:
- `PANICKED_AT_REGEX`: Matches `thread 'name' panicked at` (last occurrence for proptest compatibility).
- `ERROR_REGEX`: Matches `Error: ` for Result-based test failures.

## Testing patterns

### Proptest strategies

`test_helpers.rs` provides `Arbitrary` implementations for:
- `Duration` via `arb_duration()`.
- `DateTime<FixedOffset>` via `arb_datetime_fixed_offset()` (with minute-precision offsets for JSON round-tripping).
- `SmolStr` via `arb_smol_str()`.
- `ConfigIdentifier`, `ScriptId`.

### Snapshot testing

Extensive use of insta snapshots in `displayer/snapshots/` for:
- Progress bar messages.
- Running test display.
- Skip count formatting.
- Final warnings.

### Property tests for output decisions

The `status_level.rs` tests use proptest to exhaustively verify `compute_output_on_test_finished()`:
- ~10 property tests covering all combinations of display, cancel_status, status levels.
- Deterministic tests (no sleeps) per Oxide philosophy.

## Key conventions

### Lifetimes

- `Reporter<'a>`, `DisplayReporter<'a>`, etc. borrow from the test list.
- Events use `TestInstanceId<'a>` (borrowed) during the run.
- `OwnedTestInstanceId` is used for stored/replayed data.

### Boxing large events

`ReporterEvent::Test(Box<TestEvent<'a>>)` boxes the inner event to keep the enum size manageable.

### Output indentation

`ChildOutputSpec.output_indent` is a `&'static str` to avoid allocations:
```rust
pub(super) output_indent: &'static str,  // e.g., "    " or ""
```

### Environment variables

- `__NEXTEST_DISPLAY_EMPTY_OUTPUTS`: Force display of empty stdout/stderr (for testing).
- `__NEXTEST_PROGRESS_PRINTLN_CHUNK_SIZE`: Configure output chunking size (default 4096).

### CI detection

- `is_ci::uncached()` disables progress bar in CI environments that pretend to be terminals.
- Per-line ANSI reset handles CI color reset issues.

## Performance considerations

- `DebugIgnore` wrapper avoids expensive debug formatting for `final_outputs` vector.
- `print_lines_in_chunks()` prevents terminal overwhelm with large outputs.
- Bounded channel (128) provides backpressure for recording thread.
- Progress bar refresh rate minimized to 1 Hz to batch terminal updates.

## Adding new event types

When adding a new `TestEventKind` variant:

1. Add the variant to `TestEventKind` in `events.rs`.
2. Update `DisplayReporterImpl::write_event_impl()` in `displayer/imp.rs`.
3. Update `ProgressBarState::update_progress_bar()` in `displayer/progress.rs` if it affects progress.
4. Update `TerminalProgress::update_progress()` if it affects OSC 9;4 reporting.
5. Update `LibtestReporter::write_event()` in `structured/libtest.rs` if it maps to libtest output.
6. Consider whether it should be recorded (update `TestEventSummary::from_test_event()`).
7. Add snapshot tests for the new output.

## Common pitfalls

- **Forgetting to handle cancellation**: Output display logic must account for all `CancelReason` variants.
- **ANSI escape leakage**: Always reset colors after output that may contain ANSI escapes.
- **Progress bar visibility**: Track all hide states; don't show bar during no-capture, pause, or info display.
- **Event ordering**: Events must be processed in order for correct statistics accumulation.
- **Non-recordable events**: Interactive events (Info*, InputEnter) should not be recorded.
