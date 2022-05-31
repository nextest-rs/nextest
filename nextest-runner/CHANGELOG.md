# Changelog

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
