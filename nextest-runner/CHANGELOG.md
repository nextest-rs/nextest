# Changelog

## [0.29.1-rc.6] - 2022-11-03

This is a test release.

## [0.29.0] - 2022-11-01

### Changed

See the changelog for [cargo-nextest 0.9.42](https://nexte.st/CHANGELOG.html#0942---2022-11-01).

## [0.28.0] - 2022-10-25

### Changed

See the changelog for [cargo-nextest 0.9.40](https://nexte.st/CHANGELOG.html#0940---2022-10-25).

## [0.27.0] - 2022-10-14

### Changed

See the changelog for [cargo-nextest 0.9.39](https://nexte.st/CHANGELOG.html#0939---2022-10-14).

## [0.26.0] - 2022-10-05

### Changed

See the changelog for [cargo-nextest 0.9.38](https://nexte.st/CHANGELOG.html#0938---2022-10-05).

## [0.25.0] - 2022-09-30

### Changed

See the changelog for [cargo-nextest 0.9.37](https://nexte.st/CHANGELOG.html#0937---2022-09-30).


## [0.24.0] - 2022-09-07

### Changed

See the changelog for [cargo-nextest 0.9.36](https://nexte.st/CHANGELOG.html#0936---2022-09-07).


## [0.23.0] - 2022-08-17

### Changed

See the changelog for [cargo-nextest 0.9.35](https://nexte.st/CHANGELOG.html#0935---2022-08-17).

## [0.22.2] - 2022-08-12

### Changed

See the changelog for [cargo-nextest 0.9.34](https://nexte.st/CHANGELOG.html#0934---2022-08-12).

## [0.22.1] - 2022-07-31

### Fixed

- Reverted `indicatif` to 0.16.2 to fix regression where nextest no longer produced any output if stderr wasn't a terminal.

## [0.22.0] - 2022-07-30

### Changed

- Progress bar library `indicatif` updated to 0.17.0.

## [0.21.0] - 2022-07-27

### Changed

See the changelog for [cargo-nextest 0.9.31](https://nexte.st/CHANGELOG.html#0931---2022-07-27).

## [0.20.0] - 2022-07-25

### Changed

See the changelog for [cargo-nextest 0.9.30](https://nexte.st/CHANGELOG.html#0930---2022-07-25).

## [0.19.0] - 2022-07-24

### Changed

See the changelog for [cargo-nextest 0.9.29](https://nexte.st/CHANGELOG.html#0929---2022-07-24).

## [0.18.0] - 2022-07-22

### Changed

See the changelog for [cargo-nextest 0.9.27](https://nexte.st/CHANGELOG.html#0927---2022-07-22).

## [0.17.0] - 2022-07-14

### Changed

* nextest-metadata updated to 0.5.0.

## [0.16.0] - 2022-07-13

See the changelog for [cargo-nextest 0.9.25](https://nexte.st/CHANGELOG.html#0925---2022-07-13).

## [0.15.0] - 2022-07-01

### Added

- New config option `profile.<profile-name>.test-threads` controls the number of tests run simultaneously. This option accepts either an integer with the number of threads, or the string "num-cpus" (default) for the number of logical CPUs.

### Fixed

- Within JUnit XML, test failure descriptions (text nodes for `<failure>` and `<error>` tags) now have invalid ANSI escape codes stripped from their output.

## [0.14.0] - 2022-06-26

### Added

- On Windows, nextest now detects tests that abort due to e.g. an access violation (segfault) and prints their status as "ABORT" rather than "FAIL", along with an explanatory message on the next line.
- Improved JUnit support: nextest now heuristically detects stack traces and adds them to the text node of the `<failure>` element ([#311]).

[#311]: https://github.com/nextest-rs/nextest/issues/311

## [0.13.0] - 2022-06-21

### Added

- Benchmarks are now treated as normal tests. ([#283], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  Note that criterion.rs benchmarks are currently incompatible with nextest ([#96]) -- this change doesn't have any effect on that.

### Changed

- If nextest's output is colorized, it no longer strips ANSI escape codes from test runs.
- quick-junit updated to 0.2.0.

[#283]: https://github.com/nextest-rs/nextest/pull/283
[#96]: https://github.com/nextest-rs/nextest/issues/96

## [0.12.0] - 2022-06-17

### Added

- On Unix, tests that fail due to a signal (e.g. SIGSEGV) will print out the name of the signal rather than the generic "FAIL".

### Changed

- Progress bars now take up the entire width of the screen. This prevents issues with the bar wrapping around on terminals that aren't wide enough.

## [0.11.1] - 2022-06-13

### Fixed

- Account for skipped tests when determining the length of the progress bar.

## [0.11.0] - 2022-06-13

### Added

- Nextest can now update itself! Once this version is installed, simply run `cargo nextest self update` to update to the latest version.
    > Note to distributors: you can disable self-update by building cargo-nextest with `--no-default-features`.
- Partial, emulated support for test binary arguments passed in after `cargo nextest run --` ([#265], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  For example, `cargo nextest run -- my_test --ignored` will run ignored tests containing `my_test`, similar to `cargo test -- my_test --ignored`.

  Support is limited to test names, `--ignored` and `--include-ignored`.

  > Note to integrators: to reliably disable all argument parsing, pass in `--` twice. For example, `cargo nextest run -- -- my-filter`.

### Fixed

- Better detection for cross-compilation -- now look through the `CARGO_BUILD_TARGET` environment variable, and Cargo configuration as well. The `--target` option is still preferred.
- Slow and flaky tests are now printed out properly in the final status output ([#270]).

[#265]: https://github.com/nextest-rs/nextest/pull/265
[#270]: https://github.com/nextest-rs/nextest/issues/270

## [0.10.0] - 2022-06-08

### Added

- Support for terminating tests if they take too long, via the configuration parameter `slow-timeout.terminate-after`. For example, to time out after 120 seconds:

    ```toml
    slow-timeout = { period = "60s", terminate-after = 2 }
    ```

    Thanks [steveeJ](https://github.com/steveeJ) for your contribution ([#214])!

[#214]: https://github.com/nextest-rs/nextest/pull/214

### Fixed

- Improved support for [reusing builds](https://nexte.st/book/reusing-builds): produce better error messages if the workspace's source is missing.

### Changed

- Errors are now defined with [thiserror](https://docs.rs/thiserror). Some minor API changes were required for the migration.

## [0.9.0] - 2022-06-07

This release contains a number of user experience improvements.

### Added

- If producing output to an interactive terminal, nextest now prints out its status as a progress bar. This makes it easy to see the status of a test run at a glance.
- Nextest's configuration has a new `final-status-level` option which can be used to print out some statuses at the end of a run (defaults to `none`). On the command line, this can be overridden with the `--final-status-level` argument or `NEXTEST_FINAL_STATUS_LEVEL` in the environment.
- If a [target runner](https://nexte.st/book/target-runners) is in use, nextest now prints out its name and the environment variable or config file the definition was obtained from.

### Changed

- If the creation of a test list fails, nextest now prints a more descriptive error message, and exits with the exit code 104 ([`TEST_LIST_CREATION_FAILED`]).

[`TEST_LIST_CREATION_FAILED`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.TEST_LIST_CREATION_FAILED

## [0.8.1] - 2022-06-02

### Added

- Nextest now [sets `NEXTEST_LD_*` and `NEXTEST_DYLD_*` environment
  variables](https://nexte.st/book/env-vars.html#environment-variables-nextest-sets) to work around
  macOS System Integrity Protection sanitization.

### Fixed

- While [archiving build artifacts](https://nexte.st/book/reusing-builds), work around some libraries producing linked paths that don't exist ([#247]). Print a warning for those paths instead of failing.

[#247]: https://github.com/nextest-rs/nextest/issues/247

### Changed

- Build artifact archives no longer recurse into linked path subdirectories. This is not a behavioral change because `LD_LIBRARY_PATH` and other similar variables do not recurse into subdirectories either.

## [0.8.0] - 2022-05-31

### Added

- Support for creating and running archives of test binaries.
  - Most of the new logic is within a new `reuse_build` module.
- Non-test binaries and dynamic libraries are now recorded in `BinaryList`.

### Fixed

Fix for experimental feature [filter expressions](https://nexte.st/book/filter-expressions.html):
- Fix test filtering when expression filters are set but name-based filters aren't.

### Changed

- MSRV bumped to Rust 1.59.

## [0.7.0] - 2022-04-18

### Fixed

- `PathMapper` now canonicalizes the remapped workspace and target directories (and returns an error if that was unsuccessful).
- If the workspace directory is remapped, `CARGO_MANIFEST_DIR` in tests' runtime environment is set to the new directory.

## [0.6.0] - 2022-04-16

### Added

- Experimental support for [filter expressions](https://nexte.st/book/filter-expressions).

## [0.5.0] - 2022-03-22

### Added

- `BinaryList` and `TestList` have a new member called `rust_build_meta`, which returns Rust build-related metadata for a binary list or test list. This currently contains the target directory, the base output directories, and paths to [search for dynamic libraries in](https://nexte.st/book/env-vars#dynamic-library-paths) relative to the target directory.

### Changed

- MSRV bumped to Rust 1.56.

## [0.4.0] - 2022-03-07

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their contributions to this release!

### Added

- Filter test binaries by the build platform they're for (target or host).
- Experimental support for reusing build artifacts between the build and run steps.
- Nextest executions done as a separate process per test (currently the only supported method, though this might change in the future) set the environment variable `NEXTEST_PROCESS_MODE=process-per-test`.

### Changed

- `TargetRunner` now has separate handling for the target and host platforms. As part of this, a new struct `PlatformRunner` represents a target runner for a single platform.

## [0.3.0] - 2022-02-23

### Fixed

- Target runners of the form `runner = ["bin-name", "--arg1", ...]` are now parsed correctly ([#75]).
- Binary IDs for `[[bin]]` and `[[example]]` tests are now unique, in the format `<crate-name>::bin/<binary-name>` and `<crate-name>::test/<binary-name>` respectively ([#76]).

[#75]: https://github.com/nextest-rs/nextest/pull/75
[#76]: https://github.com/nextest-rs/nextest/pull/76

## [0.2.1] - 2022-02-23

- Improvements to `TargetRunnerError` message display: source errors are no longer displayed directly, only in "caused by".

## [0.2.0] - 2022-02-22

### Added

- Support for [target runners](https://nexte.st/book/target-runners).

## [0.1.2] - 2022-02-20

### Added

- In test output, module paths are now colored cyan ([#42]).

[#42]: https://github.com/nextest-rs/nextest/pull/42

## [0.1.1] - 2022-02-14

### Changed

- Updated quick-junit to 0.1.5, fixing builds on Rust 1.54.

## [0.1.0] - 2022-02-14

- Initial version.

[0.29.1-rc.6]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.29.1-rc.6
[0.29.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.29.0
[0.28.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.28.0
[0.27.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.27.0
[0.26.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.26.0
[0.25.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.25.0
[0.24.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.24.0
[0.23.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.23.0
[0.22.2]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.22.2
[0.22.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.22.1
[0.22.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.22.0
[0.21.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.21.0
[0.20.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.20.0
[0.19.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.19.0
[0.18.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.18.0
[0.17.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.17.0
[0.16.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.16.0
[0.15.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.15.0
[0.14.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.14.0
[0.13.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.13.0
[0.12.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.12.0
[0.11.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.11.1
[0.11.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.11.0
[0.10.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.10.0
[0.9.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.9.0
[0.8.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.8.1
[0.8.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.8.0
[0.7.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.7.0
[0.6.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.6.0
[0.5.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.5.0
[0.4.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.4.0
[0.3.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.3.0
[0.2.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.2.1
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.2.0
[0.1.2]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.1.2
[0.1.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.1.1
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-runner-0.1.0
