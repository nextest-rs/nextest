# Runner module

This document provides context for the `runner` module in `nextest-runner`. It covers architecture, patterns, and conventions specific to this module that supplement the root `AGENTS.md`.

For the high-level design document, see [The runner loop](https://nexte.st/docs/design/architecture/runner-loop/).

## Architecture overview

The runner executes tests and setup scripts, coordinating scheduling and responses. It follows an **actor model** with two main components that communicate via message passing—no direct shared state.

### Dispatcher and executor

```
                    ┌─────────────────────────────────────────┐
                    │              Dispatcher                 │
                    │  (external world coordination)          │
                    │                                         │
  Signal handler ──►│  ┌────────────────────────────────┐    │
  Input handler ───►│  │  tokio::select! over sources   │    │──► Reporter callback
  Report cancel ───►│  │  → InternalEvent               │    │
                    │  │  → handle_event()              │    │
                    │  │  → HandleEventResponse         │    │
                    │  └────────────────────────────────┘    │
                    └───────────────▲────────────────────────┘
                                    │ ExecutorEvent
                          ┌─────────┴─────────┐
                          │ Unbounded channel │
                          └─────────▲─────────┘
                                    │
                    ┌───────────────┴───────────────────────┐
                    │              Executor                  │
                    │  (schedules and runs units)           │
                    │                                        │
                    │  Setup scripts (serial)                │
                    │  Tests (parallel via future_queue)     │
                    └────────────────────────────────────────┘
```

**Key principle**: The dispatcher is the source of truth for system state as seen by the user. The executor owns the state of individual units. Linearization via synchronization points ensures a consistent view.

### Communication channels

Each unit of work (test attempt or setup script) has:
1. **Response channel** (unit → dispatcher): Reports progress, completion, errors via `ExecutorEvent`.
2. **Request channel** (dispatcher → unit): Receives queries, job control, cancellation via `RunUnitRequest`.

Channels are **unbounded** because they're low-traffic and simplify the implementation. Each unit gets a dedicated channel (not broadcast) to enable future per-unit notifications.

## Module structure

```
runner/
├── mod.rs              # Public API, re-exports, platform selection
├── imp.rs              # TestRunner, TestRunnerBuilder, TestRunnerInner (~900 lines)
├── dispatcher.rs       # DispatcherContext, event handling loop (~1800 lines)
├── executor.rs         # ExecutorContext, unit execution (~1600 lines)
├── internal_events.rs  # ExecutorEvent, RunUnitRequest, internal types (~320 lines)
├── script_helpers.rs   # Setup script env file parsing
├── unix.rs             # Unix-specific: signals, process groups, termination
└── windows.rs          # Windows-specific: job objects, handle inheritance
```

## Key types

### Public API (`imp.rs`)

- **`TestRunner<'a>`**: Main entry point. Created via `TestRunnerBuilder`. Call `execute()` or `try_execute()` to run tests.
- **`TestRunnerBuilder`**: Configures capture strategy, retries, max-fail, test threads, stress conditions, interceptor (debugger/tracer).
- **`Interceptor`**: Wraps test execution with a debugger or tracer. Controls timeout disabling, stdin passthrough, process group creation.

### Dispatcher (`dispatcher.rs`)

- **`DispatcherContext<'a, F>`**: Holds all dispatcher state. `F` is the reporter callback type.
- **`InternalEvent<'a>`**: Events from all sources (executor, signals, input, global timeout, report cancel).
- **`HandleEventResponse`**: What to do after handling an event (job control, info request, cancel, none).
- **`SignalCount`**: Tracks signal escalation (Once → Twice → panic on third).
- **`CancelReason`**: Why the run was cancelled, with intentional ordering for output suppression.

### Executor (`executor.rs`)

- **`ExecutorContext<'a>`**: Holds executor configuration (profile, test list, target runner, etc.).
- **`TestPacket<'a>`**: All information needed to run a single test attempt.
- **`SetupScriptPacket<'a>`**: All information needed to run a setup script.
- **`UnitContext<'a>`**: Either a test or setup script, with timing information.
- **`BackoffIter`**: Iterator producing retry delays with optional jitter.

### Internal events (`internal_events.rs`)

- **`ExecutorEvent<'a>`**: Events from executor to dispatcher (~12 variants covering full lifecycle).
- **`RunUnitRequest<'a>`**: Requests from dispatcher to units (Signal, OtherCancel, Query).
- **`SignalRequest`**: Stop (Unix), Continue (Unix), Shutdown.
- **`ShutdownRequest`**: Once(event) or Twice (escalated to SIGKILL/job termination).

## Unit lifecycle

### Test execution flow

```
Started ──► [Slow]* ──► (AttemptFailedWillRetry ──► RetryStarted)* ──► Finished
   │                           │
   └──────────────────────────►┘ (via resp_tx channel)
```

1. **Started**: Test begins. Dispatcher registers channel, acknowledges start.
2. **Slow** (optional): Test exceeded `slow_timeout.period`. May terminate if `terminate_after` reached.
3. **AttemptFailedWillRetry** (optional): Attempt failed, retries remain. Includes delay before next.
4. **RetryStarted** (optional): New attempt beginning. Dispatcher must acknowledge.
5. **Finished**: Final attempt complete (pass or fail). Includes all output.

### Setup script execution flow

```
SetupScriptStarted ──► [SetupScriptSlow]* ──► SetupScriptFinished
```

Setup scripts run **serially** before tests. A failing setup script cancels the entire run (ignores `--no-fail-fast`).

### State machine per unit

Each unit is an async state machine selecting over:
- The thing being waited for (process exit, timeout, etc.).
- The request channel from the dispatcher.

States include: Running, Slow, Terminating, Exiting (leak detection), DelayBeforeNextAttempt.

## Synchronization points

The dispatcher and executor synchronize at key points to maintain consistency:

1. **Unit start**: Executor sends `Started`/`SetupScriptStarted` with a oneshot channel. Waits for dispatcher to send back the request receiver. Only then does execution proceed.

2. **Retry start**: Similar synchronization via `RetryStarted` with oneshot channel.

3. **Info requests**: Dispatcher broadcasts `GetInfo` query. Units respond with current state. Dispatcher waits (with timeout) for responses.

4. **Job control** (Unix): Dispatcher broadcasts `Stop`, waits for acknowledgment from all units before raising `SIGSTOP` on itself.

## Signal handling

### Signal escalation

```
First signal  → ShutdownRequest::Once(event) → Send signal to children
Second signal → ShutdownRequest::Twice       → SIGKILL / TerminateJobObject
Third signal  → Panic (immediate exit)
```

### Cancel reasons (ordered)

```rust
SetupScriptFailure < TestFailure < TestFailureImmediate < ReportError
    < GlobalTimeout < Signal < Interrupt < SecondSignal
```

Higher values suppress output to avoid spam during shutdown.

### Job control (Unix only)

- **SIGTSTP**: Pause execution. Dispatcher broadcasts `Stop`, waits for acks, pauses timers, raises `SIGSTOP`.
- **SIGCONT**: Resume execution. Dispatcher broadcasts `Continue`, resumes timers.

Timers (`PausableSleep`, `StopwatchStart`) are pausable to correctly track elapsed time across suspend/resume cycles.

## Process management

### Unix (`unix.rs`)

- **Process groups**: Created via `cmd.process_group(0)`. Signals sent to `-pid` affect the whole group.
- **Double-spawn pattern**: Not in this module, but used elsewhere to avoid SIGTSTP race between fork() and execve().
- **Termination**: Send signal (SIGTERM/SIGINT/etc.), wait grace period, escalate to SIGKILL if needed.
- **Job objects**: Stub implementation (no-op) for API compatibility.

### Windows (`windows.rs`)

- **Job objects**: Created via `win32job` crate. All child processes assigned to job for group termination.
- **Handle inheritance**: `configure_handle_inheritance()` controls whether stdout/stderr are inherited (affects no-capture mode).
- **Termination**: Wait for grace period, then `TerminateJobObject()`. Always call `TerminateJobObject` at end to clean up grandchildren.
- **No process groups**: `set_process_group()` is a no-op. Job objects provide similar functionality.

### Platform abstraction

The `os` module alias selects `unix.rs` or `windows.rs` via `#[cfg]`:

```rust
#[cfg(unix)]
#[path = "unix.rs"]
mod os;

#[cfg(windows)]
#[path = "windows.rs"]
mod os;
```

Both modules export the same API: `configure_handle_inheritance_impl`, `set_process_group`, `create_job`, `assign_process_to_job`, `terminate_child`, etc.

## Timeout handling

### Slow timeout

Configured per-test via `slow_timeout`:
- `period`: Time before marking as slow.
- `terminate_after`: Number of periods before termination.
- `grace_period`: Time after termination signal before SIGKILL.
- `on_timeout`: Result if timeout occurs (default: Fail).

The executor uses a `pausable_sleep` that resets after each period. After `terminate_after` periods, termination begins.

### Grace period

After sending the initial termination signal, the executor waits `grace_period` for the process to exit gracefully. If exceeded, SIGKILL (Unix) or TerminateJobObject (Windows) is used.

During the grace period, the unit continues handling requests (info queries, job control, further shutdown signals).

### Global timeout

Set at the profile level. The dispatcher tracks this with a separate pausable sleep. On expiration, all units receive a shutdown signal.

## Leak detection

After a process exits, the executor checks if stdout/stderr file descriptors close within `leak_timeout.period`:

```rust
enum LeakDetectInfo {
    NoLeak { time_to_close: Duration },
    Leaked,
    SkippedForInterceptor,
}
```

Leaked handles indicate grandchild processes that inherited the descriptors. Depending on `leak_timeout.result`, this can be a pass or fail.

Leak detection is **skipped** when an interceptor (debugger/tracer) is active.

## Retry and backoff

### Retry policy

```rust
enum RetryPolicy {
    Fixed { count, delay, jitter },
    Exponential { count, delay, jitter, max_delay },
}
```

### BackoffIter

Produces retry delays:
- **Fixed**: Same delay each time, optional jitter.
- **Exponential**: Delay doubles each time (factor 2.0), capped at `max_delay`, optional jitter.

Jitter applies a random factor in (0.5, 1.0] to avoid thundering herd.

### Delay between attempts

Between retry attempts, the unit enters `DelayBeforeNextAttempt` state:
- Handles job control (pause/resume the delay timer).
- Responds to info queries with remaining delay time.
- Exits early on shutdown or cancellation.

## Stress testing

### Stress conditions

```rust
enum StressCondition {
    Count(StressCount),      // Run N times or infinite
    Duration(Duration),      // Run until time elapsed
}
```

### Stress context

`DispatcherStressContext` tracks:
- Condition (count or duration).
- Sub-run stopwatch (paused during SIGTSTP).
- Completed count, failed count, cancelled flag.

Each sub-run resets `RunStats` but accumulates in `StressRunStats`.

### StressIndex

Passed through events to identify which stress iteration:
```rust
struct StressIndex {
    current: u32,           // 0-indexed
    total: Option<NonZero<u32>>,  // None if infinite or duration-based
}
```

## Interceptors (debuggers/tracers)

`Interceptor` wraps test execution:

```rust
enum Interceptor {
    None,
    Debugger(DebuggerCommand),
    Tracer(TracerCommand),
}
```

Effects:
- **Timeouts disabled**: Both debuggers and tracers.
- **Stdin passthrough**: Debuggers only (for interactive input).
- **Process group**: Not created for debuggers (needs terminal control).
- **Leak detection skipped**: Both.
- **SIGTSTP handling**: Debuggers don't receive forwarded SIGTSTP.

## Environment variables

Tests receive these environment variables:
- `NEXTEST_RUN_ID`: UUID for the run.
- `NEXTEST_RUN_MODE`: "normal", "stress", etc.
- `NEXTEST_TEST_THREADS`: Number of test threads for this run.
- `NEXTEST_WORKSPACE_ROOT`: Absolute path to the workspace root.
- `NEXTEST_VERSION`: Current nextest version (semver string).
- `NEXTEST_REQUIRED_VERSION`: Required nextest version from config, or "none".
- `NEXTEST_RECOMMENDED_VERSION`: Recommended nextest version from config, or "none".
- `NEXTEST_BINARY_ID`: Binary being run.
- `NEXTEST_TEST_NAME`: Test name.
- `NEXTEST_ATTEMPT`, `NEXTEST_TOTAL_ATTEMPTS`, `NEXTEST_ATTEMPT_ID`: Retry tracking.
- `NEXTEST_STRESS_CURRENT`, `NEXTEST_STRESS_TOTAL`: Stress run tracking.
- `NEXTEST_TEST_GLOBAL_SLOT`, `NEXTEST_TEST_GROUP`, `NEXTEST_TEST_GROUP_SLOT`: Scheduling info.

Setup scripts also receive `NEXTEST_RUN_ID`, `NEXTEST_RUN_MODE`, `NEXTEST_TEST_THREADS`, `NEXTEST_WORKSPACE_ROOT`, and the version env vars (`NEXTEST_VERSION`, `NEXTEST_REQUIRED_VERSION`, `NEXTEST_RECOMMENDED_VERSION`).

## USDT probes

The runner fires USDT (Userspace Statically Defined Tracing) probes at key points:
- `UsdtRunStart`, `UsdtRunDone`: Run lifecycle.
- `UsdtStressSubRunStart`, `UsdtStressSubRunDone`: Stress sub-runs.
- `UsdtSetupScriptStart`, `UsdtSetupScriptSlow`, `UsdtSetupScriptDone`: Setup scripts.
- `UsdtTestAttemptStart`, `UsdtTestAttemptSlow`, `UsdtTestAttemptDone`: Test attempts.

These enable external tools (DTrace, bpftrace) to observe nextest behavior.

## Testing patterns

### Unit tests in dispatcher.rs

`begin_cancel_report_signal_interrupt` tests the signal escalation state machine:
- Verifies cancel reason ordering.
- Tests all combinations of signals.
- Uses `disable_signal_3_times_panic` to avoid actual panic in tests.

### Property-based testing

Where applicable (e.g., stress condition serialization), use proptest with `#[cfg_attr(test, derive(test_strategy::Arbitrary))]`.

### Deterministic tests

Per project conventions, avoid sleeps in tests. Use explicit synchronization. The pausable timer implementations support testing without wall-clock delays.

## Common pitfalls

### Channel ordering

Events must be processed in order. The unbounded channels preserve ordering, but be careful not to introduce races by handling events out of band.

### Forgetting synchronization

New unit types must implement the start synchronization pattern (send event with oneshot, wait for response).

### Platform differences

Always consider both Unix and Windows when modifying termination or process management code. The `os` module abstraction helps, but semantics differ:
- Unix: Signals, process groups, graceful shutdown.
- Windows: Job objects, no signals except interrupt, different exit code semantics.

### Timer pausing

All timers that measure elapsed time must be pausable for correct SIGTSTP handling. Use `crate::time::pausable_sleep` and `StopwatchStart`.

### Leak detection edge cases

During leak detection, the process has already exited but file descriptors may be open. Handle info requests correctly with the cached PID (child.id() returns None after exit).

### Interceptor mode

When an interceptor is active, many behaviors change. Check `interceptor.should_*()` methods for the full list.

## Adding new unit types

If adding a new unit type (e.g., teardown scripts):

1. Add variants to `ExecutorEvent` in `internal_events.rs`.
2. Add handling in `DispatcherContext::handle_event()`.
3. Implement the execution logic in `ExecutorContext`.
4. Create a packet type (like `TestPacket` or `SetupScriptPacket`).
5. Implement the `UnitContext` pattern for info responses.
6. Add USDT probes.
7. Consider impact on stress testing, retries, and cancellation.
