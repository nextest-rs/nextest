# Changelog

## [0.3.3] - 2023-06-07

### Added

- `TestCase` now has an extra `properties` section and an `add_property` method, similar to `TestSuite`. Thanks [@skycoop](https://github.com/skycoop) for your first contribution!

### Changed

- MSRV updated to Rust 1.66.

## [0.3.2] - 2022-11-23

### Changed

- Internal dependency update: quick-xml updated to 0.26.0.
- MSRV updated to Rust 1.62.

## [0.3.1] - 2022-11-23

(This version was not published due to a code issue.)

## [0.3.0] - 2022-07-27

### Added

- `Report` contains a new `uuid` field with a unique identifier for a particular run. This is an extension to the JUnit spec.

## [0.2.0] - 2022-06-21

### Changed

- quick-xml updated to 0.23.0.
- The error type is now defined by quick-junit, so that future breaking changes to quick-xml will not necessitate a breaking change to this crate.
- MSRV bumped to Rust 1.59.

## [0.1.5] - 2022-02-14

### Changed

- Lower MSRV to Rust 1.54.

## [0.1.4] - 2022-02-07

### Fixed

- In readme, fix link to cargo-nextest.

### Changed

- Update repository location.

## [0.1.3] - 2022-01-29

- In the readme, replace Markdown checkboxes with Unicode ✅ to make them render properly on
  crates.io.

## [0.1.2] - 2022-01-29

- Expand readme.
- Add keywords and categories.

## [0.1.1] - 2022-01-28

- Fix repository field in Cargo.toml.

## [0.1.0] - 2022-01-28

- Initial version.

[0.3.2]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.3.2
[0.3.1]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.3.1
[0.3.0]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.3.0
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.2.0
[0.1.5]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.1.5
[0.1.4]: https://github.com/nextest-rs/nextest/releases/tag/quick-junit-0.1.4
[0.1.3]: https://github.com/diem/diem-devtools/releases/tag/quick-junit-0.1.3
[0.1.2]: https://github.com/diem/diem-devtools/releases/tag/quick-junit-0.1.2
[0.1.1]: https://github.com/diem/diem-devtools/releases/tag/quick-junit-0.1.1
[0.1.0]: https://github.com/diem/diem-devtools/releases/tag/quick-junit-0.1.0
