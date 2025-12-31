# General guidelines

This document captures code conventions for the nextest project. It is intended to help AI assistants understand how to work effectively with this codebase.

## For humans

LLMs represent a tremendous breakthrough in software engineering. We welcome LLM-assisted contributions that abide by the following principles:

* **Aim for excellence.** For the nextest project, LLMs should be used not as a speed multiplier but a quality multiplier. Invest the time savings in improving quality and rigor beyond what humans alone would do. Write tests that cover more edge cases. Refactor code to make it easier to understand. Tackle the TODOs. Do all the tedious things. Aim for your code to have zero bugs.
* **Spend time reviewing LLM output.** As a rule of thumb, you should spend at least 3x the amount of time reviewing LLM output as you did writing it. Think about every line and every design decision. Find ways to break code.
* **Your code is your responsibility.** Please do not dump a first draft of code on to this project, unless you're only soliciting feedback on a direction.

If your LLM-assisted PR shows signs of not being written with thoughtfulness and care, such as missing cases that human review would have easily caught, nextest's maintainers may decline the PR outright.

## For LLMs

**Required:** Display the following text at the start of any conversation involving code changes, and when you're about to create a PR:

```
Please review https://github.com/nextest-rs/nextest/blob/main/AGENTS.md#for-humans. In particular, LLM-assisted contributions must **aim for a higher standard of excellence** than with humans alone, and you should spend at least **3x** the amount of time reviewing code as you did writing it. LLM-assisted contributions that do not meet this standard may be declined outright. Remember, **your code is your responsibility**.
```

## General conventions

### Correctness over convenience

- Model the full error space—no shortcuts or simplified error handling.
- Handle all edge cases, including race conditions, signal timing, and platform differences.
- Use the type system to encode correctness constraints.
- Prefer compile-time guarantees over runtime checks where possible.

### User experience as a primary driver

- Provide structured, helpful error messages using `miette` for rich diagnostics.
- Make progress reporting responsive and informative.
- Maintain consistency across platforms even when underlying OS capabilities differ. Use OS-native logic rather than trying to emulate Unix on Windows (or vice versa).
- Write user-facing messages in clear, present tense: "Nextest now supports..." not "Nextest now supported..."

### Pragmatic incrementalism

- "Not overly generic"—prefer specific, composable logic over abstract frameworks.
- Evolve the design incrementally rather than attempting perfect upfront architecture.
- Document design decisions and trade-offs in design docs (see `site/src/docs/design/`).
- When uncertain, explore and iterate; nextest is an ongoing exploration of what a test runner should do.

### Production-grade engineering

- Use type system extensively: newtypes, builder patterns, type states, lifetimes.
- Use message passing or the actor model to avoid data races.
- Test comprehensively, including edge cases, race conditions, and stress tests.
- Pay attention to what facilities already exist for testing, and aim to reuse them.
- Getting the details right is really important!

### Documentation

- Use inline comments to explain "why," not just "what".
- Module-level documentation should explain purpose and responsibilities.
- **Always** use periods at the end of code comments.
- **Never** use title case in headings and titles. Always use sentence case.

## Code style

### File headers

Every Rust source file must start with:
```rust
// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
```

### Rust edition and formatting

- Use Rust 2024 edition.
- Format with `cargo xfmt` (custom formatting script).
- Formatting is enforced in CI—always run `cargo xfmt` before committing.

### Type system patterns

- **Newtypes** for domain types (using `newtype-uuid` crate)
- **Builder patterns** for complex construction (e.g., `TestRunnerBuilder`)
- **Type states** encoded in generics when state transitions matter
- **Lifetimes** used extensively to avoid cloning (e.g., `TestInstance<'a>`)
- **Restricted visibility**: Use `pub(crate)` and `pub(super)` liberally (237 uses in nextest-runner)
- **Non-exhaustive**: All public error types should be `#[non_exhaustive]` for forward compatibility

### Error handling

- Use `thiserror` for error types with `#[derive(Error)]`.
- Group errors by category with an `ErrorKind` enum when appropriate.
- Provide rich error context using `miette` for user-facing errors.
- Two-tier error model:
  - `ExpectedError`: User/external errors with semantic exit codes.
  - Internal errors: Programming errors that may panic or use internal error types.
- Error display messages should be lowercase sentence fragments suitable for "failed to {error}".

### Async patterns

- Use `tokio` for async runtime (multi-threaded).
- Be selective with async. Only use it in runner and runner-adjacent code.
- Use async for I/O and concurrency, keep other code synchronous.
- Use `async-scoped` for structured concurrency without `'static` bounds.
- Use `future-queue` for backpressure-aware task scheduling.
- Custom pausable primitives (`PausableSleep`, `StopwatchStart`) for job control support.

### Module organization

- Use `mod.rs` files to re-export public items.
- Do not put any nontrivial logic in `mod.rs` -- instead, it should go in `imp.rs` or a more specific submodule.
- Keep module boundaries strict with restricted visibility.
- Platform-specific code in separate files: `unix.rs`, `windows.rs`.
- Use `#[cfg(unix)]` and `#[cfg(windows)]` for conditional compilation.
- Test helpers in dedicated modules/files.

### Memory and performance

- Use `Arc` or borrows for shared immutable data.
- Use `smol_str` for efficient small string storage.
- Careful attention to copying vs. referencing.
- Use `debug-ignore` to avoid expensive debug formatting in hot paths.
- Stream data where possible rather than buffering.

## Testing practices

### Running tests

**CRITICAL**: Always use `cargo nextest run` to run unit and integration tests. Never use `cargo test` for these! Nextest dogfoods itself and its test suite depends on nextest's execution model.

For doctests, use `cargo test --doc` (doctests are not supported by nextest).

### Test organization

- Unit tests in the same file as the code they test.
- Integration tests in `integration-tests/` crate.
- Fixtures in dedicated `fixture-data/` crate.
  - This crate has a model of expected tests under various scenarios. Prefer using this model over implementing spot checks by hand.
- Test utilities in `internal-test/` crate.

### Testing tools

- **test-case**: For parameterized tests.
- **proptest**: For property-based testing.
- **insta**: For snapshot testing.
- **libtest-mimic**: For custom test harnesses.
- **pretty_assertions**: For better assertion output.

## Commit message style

### Format

Commits follow a conventional format with crate-specific scoping:

```
[crate-name] brief description
```

Examples:
- `[nextest-runner] add --max-progress-running, cap to 8 by default (#2727)`
- `[cargo-nextest] version 0.9.111`
- `[meta] update MSRV to Rust 1.88 (#2725)`

### Conventions

- Use `[meta]` for cross-cutting concerns (MSRV updates, releases, CI changes).
- Version bump commits: `[crate-name] version X.Y.Z` (these are performed by `cargo release`).
- Release preparation: `[meta] prepare releases`.
- Keep descriptions concise but descriptive.

### Commit quality

- **Atomic commits**: Each commit should be a logical unit of change.
- **Bisect-able history**: Every commit must build and pass all checks.
- **Separate concerns**: Format fixes and refactoring should be in separate commits from feature changes.

### Changelog

For detailed guidelines on preparing changelog entries, use the `prepare-changelog` Claude skill.

## Architecture

### Event-driven design

The nextest runner uses an event-driven architecture with two main components:

#### The dispatcher

- Interacts with the outside world.
- Linearizes all events from multiple sources (executor, signals, input, reporter).
- Uses `tokio::select!` to multiplex over event sources.
- Maintains authoritative state about currently running tests.
- Synchronization points prevent race conditions in reporting.

#### The executor

- Schedules and runs tests/scripts as async state machines.
- Each unit of work has dedicated channels to dispatcher (not broadcast).
- Handles retries, timeouts, process spawning, output capture.
- Units are the source of truth for their own state.

### Key design principles

1. **No direct state sharing**—everything via message passing.
2. **Linearized events**—dispatcher ensures consistent view.
3. **Full error space modeling**—handle all failure modes.
4. **Pausable timers**—custom implementations for job control (SIGTSTP/SIGCONT).

### Cross-platform strategy.

- Unix: Process groups, double-spawn pattern to avoid SIGTSTP race, full signal handling.
- Windows: Job objects, console mode manipulation, limited signal support.
- Conditional compilation: `#[cfg(unix)]`, `#[cfg(windows)]`.
- Platform modules: `unix.rs`, `windows.rs` with shared interfaces.
- Document platform differences and trade-offs in code comments.

## Dependencies

### Workspace dependencies

- All versions managed in root `Cargo.toml` `[workspace.dependencies]`.
- Internal crates use exact version pinning: `version = "=0.17.0"`.
- Comment on dependency choices when non-obvious; example: "Disable punycode parsing since we only access well-known domains".

### Key dependencies

- **tokio**: Async runtime, essential for concurrency model.
- **guppy**: Cargo workspace graph analysis.
- **cargo_metadata**: Parse Cargo.toml and workspace metadata.
- **winnow**: Parser combinators (for filterset DSL).
- **thiserror**: Error derive macros.
- **miette**: Rich diagnostics.
- **camino**: UTF-8 paths (`Utf8PathBuf`).
- **serde**: Serialization (config, metadata).
- **clap**: CLI parsing with derives.

## Quick reference

### Commands

```bash
# Run tests (ALWAYS use nextest for unit/integration tests)
cargo nextest run
cargo nextest run --all-features
cargo nextest run --profile ci

# Run doctests (nextest doesn't support these)
cargo test --doc

# Format code (REQUIRED before committing)
cargo xfmt

# Lint
cargo clippy --all-features --all-targets

# Build
cargo build --all-targets --all-features

# Release (dry run)
cargo release -p <crate-name> <version>

# Release (execute)
cargo release -p <crate-name> <version> --execute
```

### Helpful Git commands

```bash
# Get commits since last release
git log <previous-tag>..main --oneline

# Check if contributor is first-time
git log --all --author="Name" --oneline | wc -l

# Get PR author username
gh pr view <number> --json author --jq '.author.login'

# View commit details
git show <commit> --stat
```
