# Changelog

## [0.3.0] - 2022-03-22

### Added

- `TestListSummary` and `BinaryListSummary` have a new member called `rust_build_meta`

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

[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.2.0
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-metadata-0.1.0
