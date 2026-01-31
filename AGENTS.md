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
- Don't add narrative comments in function bodies. Only add a comment if what you're doing is non-obvious or special in some way, or if something needs a deeper "why" explanation.
- Module-level documentation should explain purpose and responsibilities.
- **Always** use periods at the end of code comments.
- **Never** use title case in headings and titles. Always use sentence case.
- Always use the Oxford comma.
- Don't omit articles ("a", "an", "the"). Write "the file has a newer version" not "file has newer version".

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
- **Non-exhaustive in stable crates**: The `nextest-metadata` crate has a stable API and public types there should be `#[non_exhaustive]` for forward compatibility. Internal crates like `nextest-runner` do not have stable APIs, so `#[non_exhaustive]` is not required (though error types may still use it).

### Error handling

- Use `thiserror` for error types with `#[derive(Error)]`.
- Group errors by category with an `ErrorKind` enum when appropriate.
- Provide rich error context using structured error types.
  - Parts of the code use `miette` for structured error handling.
- Two-tier error model:
  - `ExpectedError`: User/external errors with semantic exit codes.
  - Internal errors: Programming errors that may panic or use internal error types.
- Error display messages should be lowercase sentence fragments suitable for "failed to {error}".

### Pluralization

- Use the `nextest_runner::helpers::plural` module for pluralizing words in user-facing messages.
- The module provides functions like `plural::runs_str(count)` that return "run" or "runs" based on count.
- Add new pluralization functions to the module as needed; do not inline pluralization logic.

### Serde patterns

- Use `serde_ignored` for ignored paths in configuration.
- Never use `#[serde(flatten)]`. Instead, copy fields to structs as necessary. The internal buffering leads to poor warnings from `serde_ignored`.
- Never use `#[serde(untagged)]` for deserializers, since it produces poor error messages. Instead, write custom visitors with an appropriate `expecting` method.

### Serialization format changes

When modifying any struct that is serialized to disk or over the wire:

1. **Trace the full version matrix**:
   - Old reader + new data: Can it deserialize? Does it lose information?
   - New reader + old data: Does `#[serde(default)]` produce correct values?
   - Old writer + new data: Can it round-trip without data loss? (This is the easy one to miss!)

2. **Bump format versions proactively**: If adding a field that will be semantically important, bump the version when adding the field, not when first using non-default values. This prevents older versions from silently corrupting data on write-back.

3. **`#[serde(default)]` is necessary but not sufficient**: It allows old readers to deserialize new data, but old writers will still drop unknown fields on write-back.

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
- Use fully qualified imports rarely, prefer importing the type most of the time, or otherwise a module if it is conventional.
- Never write `std::fmt::Display` as a fully qualified type. Instead, import `std::fmt` and use `fmt::Display`.
- **Always** import types or functions at the very top of the module, with the one exception being `cfg()`-gated functions. Never import types or modules within function contexts, other than this `cfg()`-gated exception.
- It is okay to import enum variants for pattern matching, though.

### Memory and performance

- Use `Arc` or borrows for shared immutable data.
- Use `smol_str` for efficient small string storage.
- Careful attention to cloning referencing. Avoid cloning if code has a natural tree structure.
- Stream data (e.g. iterators) where possible rather than buffering.

### String formatting

- The `clippy::format_push_string` lint is enabled. If triggered, use the `swrite!` macro from the `swrite` crate instead of `push_str(&format!(...))`.

## Testing practices

### Running tests

**CRITICAL**: Always use `cargo nextest run` to run unit and integration tests. Never use `cargo test` for these! Nextest dogfoods itself and its test suite depends on nextest's execution model.

Use `cargo local-nt` only when you need to verify changes to nextest's own behavior (e.g., testing a new CLI flag or output format). For running the test suite itself, including `integration-tests`, use `cargo nextest run`—the integration tests spawn their own inner nextest processes from the build artifacts.

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
- Use simple past and present tense: "Previously, when the user did X, Y used to happen. With this commit, now Z happens. Also add tests for U, V, and W."
- Commit messages should be Markdown. Don't use backticks in commit message titles, but do use them in bodies.

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

# Run the in-repo (locally built) copy of nextest
cargo local-nt run

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
