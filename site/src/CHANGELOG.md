# Changelog

This page documents new features and bugfixes for cargo-nextest. Please see the [stability
policy](book/stability.md) for how versioning works with cargo-nextest.

## [0.9.15] - 2022-05-31

### Added

- Improved support for [reusing builds](https://nexte.st/book/reusing-builds):
  - New command `cargo nextest archive` automatically archives test binaries and other relevant
    files after building tests. Currently the `.tar.zst` format is supported.
  - New option `cargo nextest run --archive-file` automatically extracts archives before running the tests within them.
  - New runtime environment variable `NEXTEST_BIN_EXE_<name>` is set to the absolute path to a binary target's executable, taking path remapping into account. This is equivalent to [`CARGO_BIN_EXE_<name>`], except this is set at runtime.
  - `cargo nextest list --list-type binaries-only` now records information about non-test binaries as well.

[`CARGO_BIN_EXE_<name>`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates

### Fixed

Fix for experimental feature [filter expressions](https://nexte.st/book/filter-expressions.html):
- Fix test filtering when expression filters are set but name-based filters aren't.

## [0.9.14] - 2022-04-18

### Fixed

Fixes related to path remapping:

- Directories passed into `--workspace-remap` and `--target-dir-remap` are now canonicalized.
- If the workspace directory is remapped, `CARGO_MANIFEST_DIR` in tests' runtime environment is set to the new directory.

## [0.9.13] - 2022-04-16

### Added

- Support for [reusing builds](https://nexte.st/book/reusing-builds) is now production-ready. Build on one machine and run tests on another, including cross-compiling and test partitioning.

    To see how builds can be reused in GitHub Actions, see [this example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml).

- Experimental support for [filter expressions](https://nexte.st/book/filter-expressions.html), allowing fine-grained specifications for which tests to run.

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their fantastic work implementing both of these.

## [0.9.12] - 2022-03-22

### Added

- Support for reading some configuration as [environment variables](https://nexte.st/book/env-vars#environment-variables-nextest-reads). (Thanks [ymgyt] and [iskyzh] for their pull requests!)
- [Machine-readable output] for `cargo nextest list` now contains a `rust-build-meta` key. This key currently contains the target directory, the base output directories, and paths to [search for dynamic libraries in](https://nexte.st/book/env-vars#dynamic-library-paths) relative to the target directory.

### Fixed

- Test binaries that link to dynamic libraries built by Cargo now work correctly ([#82]).
- Crates with no tests are now skipped while computing padding widths in the reporter ([#125]).

### Changed

- MSRV updated to Rust 1.56.
- For experimental feature [reusing builds](https://nexte.st/book/reusing-builds):
  - Change `--binaries-dir-remap` to `--target-dir-remap` and expect that the entire target directory is archived.
  - Support linking to dynamic libraries ([#82]).

[#82]: https://github.com/nextest-rs/nextest/issues/82
[#125]: https://github.com/nextest-rs/nextest/issues/125
[ymgyt]: https://github.com/ymgyt
[iskyzh]: https://github.com/iskyzh
[Machine-readable output]: https://nexte.st/book/machine-readable

## [0.9.11] - 2022-03-09

### Fixed

- Update `regex` to 1.5.5 to address [GHSA-m5pq-gvj9-9vr8
  (CVE-2022-24713)](https://github.com/rust-lang/regex/security/advisories/GHSA-m5pq-gvj9-9vr8).

## [0.9.10] - 2022-03-07

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their contributions to this release!

### Added

- A new `--platform-filter` option filters tests by the platform they run on (target or host).
- `cargo nextest list` has a new `--list-type` option, with values `full` (the default, same as today) and `binaries-only` (list out binaries without querying them for the tests they contain).
- Nextest executions done as a separate process per test (currently the only supported method, though this might change in the future) set the environment variable `NEXTEST_PROCESS_MODE=process-per-test`.

### New experimental features

- Nextest can now reuse builds across invocations and machines. This is an experimental feature, and feedback is welcome in [#98]!

[#98]: https://github.com/nextest-rs/nextest/issues/98

### Changed

- The target runner is now build-platform-specific; test binaries built for the host platform will be run by the target runner variable defined for the host, and similarly for the target platform.

## [0.9.9] - 2022-03-03

### Added

- Updates for Rust 1.59:
  - Support abbreviating `--release` as `-r` ([Cargo #10133]).
  - Stabilize future-incompat-report ([Cargo #10165]).
  - Update builtin list of targets (used by the target runner) to Rust 1.59.

[Cargo #10133]: https://github.com/rust-lang/cargo/pull/10133
[Cargo #10165]: https://github.com/rust-lang/cargo/pull/10165

## [0.9.8] - 2022-02-23

### Fixed

- Target runners of the form `runner = ["bin-name", "--arg1", ...]` are now parsed correctly ([#75]).
- Binary IDs for `[[bin]]` and `[[example]]` tests are now unique, in the format `<crate-name>::bin/<binary-name>` and `<crate-name>::test/<binary-name>` respectively ([#76]).

[#75]: https://github.com/nextest-rs/nextest/pull/75
[#76]: https://github.com/nextest-rs/nextest/pull/76

## [0.9.7] - 2022-02-23

### Fixed

- If parsing target runner configuration fails, warn and proceed without a target runner rather than erroring out.

### Known issues

- Parsing an array of strings for the target runner currently fails: [#73]. A fix is being worked on in [#75].

[#73]: https://github.com/nextest-rs/nextest/issues/73
[#75]: https://github.com/nextest-rs/nextest/pull/75

## [0.9.6] - 2022-02-22

### Added

- Support Cargo configuration for [target runners](https://nexte.st/book/target-runners).

## [0.9.5] - 2022-02-20

### Fixed

- Updated nextest-runner to 0.1.2, fixing cyan coloring of module paths ([#52]).

[#52]: https://github.com/nextest-rs/nextest/issues/52

## [0.9.4] - 2022-02-16

The big new change is that release binaries are now available! Head over to [Pre-built binaries](https://nexte.st/book/pre-built-binaries) for more.

### Added

- In test output, module paths are now colored cyan ([#42]).

### Fixed

- While querying binaries to list tests, lines ending with ": benchmark" will now be ignored ([#46]).

[#42]: https://github.com/nextest-rs/nextest/pull/42
[#46]: https://github.com/nextest-rs/nextest/issues/46

## [0.9.3] - 2022-02-14

### Fixed

- Add a `BufWriter` around stderr for the reporter, reducing the number of syscalls and fixing
  issues around output overlap on Windows ([#35](https://github.com/nextest-rs/nextest/issues/35)). Thanks [@fdncred](https://github.com/fdncred) for reporting this!

## [0.9.2] - 2022-02-14

### Fixed

- Running cargo nextest from within a crate now runs tests for just that crate, similar to cargo
  test. Thanks [Yaron Wittenstein](https://twitter.com/RealWittenstein/status/1493291441384210437)
  for reporting this!

## [0.9.1] - 2022-02-14

### Fixed

- Updated nextest-runner to 0.1.1, fixing builds on Rust 1.54.

## [0.9.0] - 2022-02-14

**Initial release.** Happy Valentine's day!

### Added

Supported in this initial release:

* [Listing tests](book/listing.md)
* [Running tests in parallel](book/running.md) for faster results
* [Partitioning tests](book/partitioning.md) across multiple CI jobs
* [Test retries](book/retries.md) and flaky test detection
* [JUnit support](book/junit.md) for integration with other test tooling

[0.9.15]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.15
[0.9.14]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.14
[0.9.13]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.13
[0.9.12]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.12
[0.9.11]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.11
[0.9.10]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.10
[0.9.9]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.9
[0.9.8]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.8
[0.9.7]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.7
[0.9.6]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.6
[0.9.5]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.5
[0.9.4]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.4
[0.9.3]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.3
[0.9.2]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.2
[0.9.1]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.1
[0.9.0]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.0
