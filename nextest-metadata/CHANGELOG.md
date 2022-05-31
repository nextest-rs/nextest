# Changelog

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

[0.4.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.4.0
[0.3.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.3.1
[0.3.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.3.0
[0.2.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.2.1
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.2.0
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.1.0
