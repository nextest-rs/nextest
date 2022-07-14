# Changelog

This page documents new features and bugfixes for cargo-nextest. Please see the [stability
policy](book/stability.md) for how versioning works with cargo-nextest.

## [0.9.26] - 2022-07-14

This is a quick hotfix release to update the version of nextest-metadata, to which a breaking change was accidentally committed.

## [0.9.25] - 2022-07-13

This is a major release with several new features.

### Filter expressions

[Filter expressions](https://nexte.st/book/filter-expressions) are now ready for production. For example, to run all tests in `nextest-runner` and all its transitive dependencies within the workspace:

```
cargo nextest run -E 'deps(nextest-runner)'
```

This release includes a number of additions and changes to filter expressions.

#### Added

* The expression language supports several new [predicates](https://nexte.st/book/filter-expressions#basic-predicates):
  - `kind(name-matcher)`: include all tests in binary kinds (e.g. `lib`, `test`, `bench`) matching `name-matcher`.
  - `binary(name-matcher)`: include all tests in binary names matching `name-matcher`.
  - `platform(host)` or `platform(target)`: include all tests that are [built for the host or target platform](running.md#filtering-by-build-platform), respectively.

#### Changed

* If a filter expression is guaranteed not to match a particular binary, it will not be listed by nextest. (This allows `platform(host)` and `platform(target)` to work correctly.)

* If both filter expressions and standard substring filters are passed in, a test must match filter expressions AND substring filters to be executed. For example:

```
cargo nextest run -E 'package(nextest-runner)' test_foo test_bar
```

This will execute only the tests in `nextest-runner` that match `test_foo` or `test_bar`.

### Per-test overrides

Nextest now supports [per-test overrides](https://nexte.st/book/per-test-overrides). These overrides let you customize settings for subsets of tests. For example, to retry tests that contain the substring `test_e2e` 3 times:

```toml
[[profile.default.overrides]]
filter = "test(test_e2e)"
retries = 3
```

Currently, only `retries` are supported. In the future, more kinds of customization will be added.

### Other changes

- A new environment variable `NEXTEST_RETRIES` controls the number of retries tests are run with. In terms of precedence, this slots in between the command-line `--retries` option and per-test overrides for retries.
- `cargo nextest list` now hides skipped tests and binaries by default. To print out skipped tests and binaries, use `cargo nextest list --verbose`.
- The [Machine-readable output](https://nexte.st/book/machine-readable) for `cargo nextest list` now contains a new `"status"` key. By default, this is set to `"listed"`, and for binaries that aren't run because they don't match expression filters this is set to `"skipped"`.
- The `--platform-filter` option is deprecated, though it will keep working for all versions within the nextest 0.9 series. Use `-E 'platform(host)'` or `-E 'platform(target)'` instead.
- `cargo nextest run -- --skip` and `--exact` now suggest using a filter expression instead.

## [0.9.24] - 2022-07-01

### Added

- New config option `profile.<profile-name>.test-threads` controls the number of tests run simultaneously. This option accepts either an integer with the number of threads, or the string "num-cpus" (default) for the number of logical CPUs. As usual, this option is overridden by `--test-threads` and `NEXTEST_TEST_THREADS`, in that order.
- The command-line `--test-threads` option and the `NEXTEST_TEST_THREADS` environment variable now accept `num-cpus` as their argument.
- nextest now works with [cargo binstall](https://github.com/ryankurte/cargo-binstall) ([#332]). Thanks [Remoun] for your first contribution!

### Fixed

- Within JUnit XML, test failure descriptions (text nodes for `<failure>` and `<error>` tags) now have invalid ANSI escape codes stripped from their output.

[#332]: https://github.com/nextest-rs/nextest/pull/332
[@remoun]: https://github.com/remoun

## [0.9.23] - 2022-06-26

### Added

- On Windows, nextest now detects tests that abort due to e.g. an access violation (segfault) and prints their status as "ABORT" rather than "FAIL", along with an explanatory message on the next line.
- Improved JUnit support: nextest now heuristically detects stack traces and adds them to the text node of the `<failure>` element ([#311]).

### Changed

- Errors that happen while writing data to the output now have a new documented exit code: [`WRITE_OUTPUT_ERROR`].

[#311]: https://github.com/nextest-rs/nextest/issues/311
[`WRITE_OUTPUT_ERROR`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.WRITE_OUTPUT_ERROR

## [0.9.22] - 2022-06-21

### Added

- Benchmarks are now treated as normal tests. ([#283], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  Note that criterion.rs benchmarks are currently incompatible with nextest ([#96]) -- this change doesn't have any effect on that.

- Added `-F` as a shortcut for `--features`, mirroring an upcoming addition to Cargo 1.62 ([#287], thanks [Alexendoo](https://github.com/Alexendoo) for your first contribution!)

### Changed

- If nextest's output is colorized, it no longer strips ANSI escape codes from test runs.

[#283]: https://github.com/nextest-rs/nextest/pull/283
[#287]: https://github.com/nextest-rs/nextest/pull/287
[#96]: https://github.com/nextest-rs/nextest/issues/96

## [0.9.21] - 2022-06-17

### Added

- On Unix, tests that fail due to a signal (e.g. SIGSEGV) will print out the name of the signal rather than the generic "FAIL".
- `cargo-nextest` has a new `"default-no-update"` feature that will contain all default features except for self-update. If you're distributing nextest or installing it in CI, the recommended, forward-compatible way to build cargo-nextest is with `--no-default-features --features default-no-update`.

### Changed

- Progress bars now take up the entire width of the screen. This prevents issues with the bar wrapping around on terminals that aren't wide enough.

## [0.9.20] - 2022-06-13

### Fixed

- Account for skipped tests when determining the length of the progress bar.

## [0.9.19] - 2022-06-13

### Added

- Nextest can now update itself! Once this version is installed, simply run `cargo nextest self update` to update to the latest version.
    > Note to distributors: you can disable self-update by building cargo-nextest with `--no-default-features`.
- Partial, emulated support for test binary arguments passed in after `cargo nextest run --` ([#265], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  For example, `cargo nextest run -- my_test --ignored` will run ignored tests containing `my_test`, similar to `cargo test -- my_test --ignored`.

  Support is limited to test names, `--ignored` and `--include-ignored`.

  > Note to integrators: to reliably disable all argument parsing, pass in `--` twice. For example, `cargo nextest run -- -- <filters...>`.

### Fixed

- Better detection for cross-compilation -- now look through the `CARGO_BUILD_TARGET` environment variable, and Cargo configuration as well. The `--target` option is still preferred.
- Slow and flaky tests are now printed out properly in the final status output ([#270]).

[#265]: https://github.com/nextest-rs/nextest/pull/265
[#270]: https://github.com/nextest-rs/nextest/issues/270

This is a test release.

## [0.9.18] - 2022-06-08

### Added

- Support for terminating tests if they take too long, via the configuration parameter `slow-timeout.terminate-after`. For example, to time out after 120 seconds:

    ```toml
    slow-timeout = { period = "60s", terminate-after = 2 }
    ```

    Thanks [steveeJ](https://github.com/steveeJ) for your contribution ([#214])!

[#214]: https://github.com/nextest-rs/nextest/pull/214

### Fixed

- Improved support for [reusing builds](https://nexte.st/book/reusing-builds): produce better error messages if the workspace's source is missing.

## [0.9.17] - 2022-06-07

This release contains a number of user experience improvements.

### Added

- If producing output to an interactive terminal, nextest now prints out its status as a progress bar. This makes it easy to see the status of a test run at a glance.
- Nextest's configuration has a new `final-status-level` option which can be used to print out some statuses at the end of a run (defaults to `none`). On the command line, this can be overridden with the `--final-status-level` argument or `NEXTEST_FINAL_STATUS_LEVEL` in the environment.
- If a [target runner](https://nexte.st/book/target-runners) is in use, nextest now prints out its name and the environment variable or config file the definition was obtained from.

### Changed

- If the creation of a test list fails, nextest now prints a more descriptive error message, and exits with the exit code 104 ([`TEST_LIST_CREATION_FAILED`]).

[`TEST_LIST_CREATION_FAILED`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.TEST_LIST_CREATION_FAILED

## [0.9.16] - 2022-06-02

### Added

- Nextest now [sets `NEXTEST_LD_*` and `NEXTEST_DYLD_*` environment
  variables](https://nexte.st/book/env-vars.html#environment-variables-nextest-sets) to work around
  macOS System Integrity Protection sanitization.

### Fixed

- While [archiving build artifacts](https://nexte.st/book/reusing-builds), work around some libraries producing linked paths that don't exist ([#247]). Print a warning for those paths instead of failing.

[#247]: https://github.com/nextest-rs/nextest/issues/247

### Changed

- Build artifact archives no longer recurse into linked path subdirectories. This is not a behavioral change because `LD_LIBRARY_PATH` and other similar variables do not recurse into subdirectories either.

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

[0.9.26]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.26
[0.9.25]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.25
[0.9.24]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.24
[0.9.23]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.23
[0.9.22]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.22
[0.9.21]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.21
[0.9.20]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.20
[0.9.19]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.19
[0.9.18]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.18
[0.9.17]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.17
[0.9.16]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.16
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
