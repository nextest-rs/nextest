# Changelog

## [0.6.0] - 2022-08-17

### Added

- `RustBuildMetaSummary` has a new `target-platform` field which records the target platform. (This
  field is optional, which means that the minimum supported nextest version hasn't been bumped with
  this release.)

## [0.5.0] - 2022-07-14

(This change was included in 0.4.4, which should have been a breaking change.)

### Changed

- `RustTestSuiteSummary::testcases` renamed to `test_cases`.

## [0.4.4] - 2022-07-13

### Added

- `RustTestSuiteSummary` has a new field `status`, which is a newtype over strings:
  - `"listed"`: the test binary was executed with `--list` to gather the list of tests in it.
  - `"skipped"`: the test binary was not executed because it didn't match any expression filters.

## [0.4.3] - 2022-06-26

### Added

- New documented exit code [`WRITE_OUTPUT_ERROR`].

[`WRITE_OUTPUT_ERROR`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.WRITE_OUTPUT_ERROR

## [0.4.2] - 2022-06-13

### Added

- New [documented exit codes] related to self-updates:
  - `UPDATE_ERROR`
  - `UPDATE_AVAILABLE`
  - `UPDATE_DOWNGRADE_NOT_PERFORMED`
  - `UPDATE_CANCELED`
  - `SELF_UPDATE_UNAVAILABLE`

[documented exit codes]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html

## [0.4.1] - 2022-06-07

### Added

- New documented exit code [`TEST_LIST_CREATION_FAILED`].

[`TEST_LIST_CREATION_FAILED`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.TEST_LIST_CREATION_FAILED

## [0.4.0] - 2022-05-31

### Added

- Support for archiving test binaries:
  - Non-test binaries and dynamic libraries are now recorded to `RustBuildMetaSummary`.

### Changed

- Minimum supported nextest version bumped to 0.9.15.
- MSRV bumped to 1.59.

## [0.3.1] - 2022-04-16

### Added

- New exit code: `INVALID_FILTER_EXPRESSION`.

## [0.3.0] - 2022-03-22

### Added

- `TestListSummary` and `BinaryListSummary` have a new member called `rust_build_meta` key. This key currently contains the target directory, the base output directories, and paths to [search for dynamic libraries in](https://nexte.st/book/env-vars#dynamic-library-paths) relative to the target directory.

### Changed

- MSRV bumped to Rust 1.56.

## [0.2.1] - 2022-03-09

Add documentation about nextest-metadata's "minimum supported cargo-nextest version".

## [0.2.0] - 2022-03-07

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their contributions to this release!

This release is compatible with cargo-nextest 0.9.10 and later.

### Added

- Lists now contain the `build-platform` variable, introduced in cargo-nextest 0.9.10.
- Support for listing binaries without querying them for the tests they contain.

### Changed

- Fields common to test and binary lists have been factored out into a separate struct, `RustTestBinarySummary`. The struct is marked with `#[serde(flatten)]` so the JSON representation stays the same.

## [0.1.0] - 2022-02-14

- Initial version, with support for listing tests.

[0.6.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.6.0
[0.5.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.5.0
[0.4.4]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.4
[0.4.3]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.3
[0.4.2]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.2
[0.4.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.1
[0.4.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.0
[0.3.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.3.1
[0.3.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.3.0
[0.2.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.2.1
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.2.0
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.1.0
