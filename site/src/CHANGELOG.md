# Changelog

This page documents new features and bugfixes for cargo-nextest. Please see the [stability
policy](book/stability.md) for how versioning works with cargo-nextest.

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

[0.9.7]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.7
[0.9.6]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.6
[0.9.5]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.5
[0.9.4]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.4
[0.9.3]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.3
[0.9.2]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.2
[0.9.1]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.1
[0.9.0]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.0
