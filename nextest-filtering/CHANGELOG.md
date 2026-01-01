# Changelog

## [0.18.0] - 2026-01-01

### Changed

- `TestQuery::test_name` is now a `&TestCaseName` instead of `&str`.

## [0.17.0] - 2025-10-29

### Added

- Support for filtering test archives by binary. The `cargo nextest archive` command now accepts filter expressions, though test-level predicates (like `test(...)`) are not supported since binaries are not executed during archiving.
- New method `FiltersetLeaf::is_runtime_only()` to check if a filter leaf requires runtime evaluation.

### Changed

- MSRV updated to Rust 1.87.
- Error messages improved for banned predicates in different contexts.

## [0.16.0] - 2025-06-04

### Added

- `ParsedLeaf` is now available as a public API.

### Changed

- MSRV updated to Rust 1.85.

## [0.15.0] - 2025-02-24

### Changed

- Added support for rejecting unknown binary IDs.

## [0.14.0] - 2025-02-10

### Changed

- Internal dependency update: winnow updated to 0.7. Thanks to [Ed Page](https://github.com/epage) for the update!

## [0.13.0] - 2025-01-15

### Changed

- MSRV updated to Rust 1.81.
- Internal dependency updates.

## [0.12.0] - 2024-08-28

### Changed

- Renamed references from "default-set" to "default-filter" to match cargo-nextest changes.

## [0.11.0] - 2024-08-25

### Changed

- Types renamed from `FilteringExpr` to `Filterset`.

## [0.10.0] - 2024-08-23

### Added

- New APIs: `CompiledExpr::matches_binary` and `matches_test`.
- Support for parsing default sets and the `default()` predicate.

### Changed

- `FilteringExpr::parse` now takes a `ParseContext`.
- The `matches_binary` and `matches_test` functions now take an `EvalContext`.
- MSRV updated to Rust 1.75.

## [0.9.0] - 2024-05-23

### Changed

- MSRV updated to Rust 1.74.
- nextest-metadata updated to 0.11.0.

## [0.8.0] - 2024-03-04

### Changed

- MSRV updated to Rust 1.73.
- Parser combinator library changed from nom to winnow. Thanks [@epage](https://github.com/epage)
  for creating winnow, and for the contribution!

## [0.7.1] - 2024-01-09

### Fixed

Internal cleanups: remove reliance on Incomplete. Thanks [@epage](https://github.com/epage) for the
contribution!

## [0.7.0] - 2023-12-10

### Changed

- `nextest-metadata` updated to 0.10.

### Misc

- The `.crate` files uploaded to crates.io now contain the `LICENSE-APACHE` and `LICENSE-MIT` license files. Thanks [@musicinmybrain](https://github.com/musicinmybrain) for your first contribution!

## [0.6.0] - 2023-12-03

### Added

- Support for glob matchers.
- Support for a new `binary_id` predicate.

For more information, see the changelog for [cargo-nextest 0.9.64](https://nexte.st/CHANGELOG.html#0964---2023-12-03).

## [0.5.1] - 2023-10-22

### Changed

- Internal dependency updates.
- MSRV updated to Rust 1.70.

## [0.5.0] - 2023-06-25

### Changed

- `guppy` updated to 0.17.0.

## [0.4.0] - 2023-05-15

### Changed

- `FilteringExpr` now carries three forms of an expression:
  - `input`, which is the raw input as provided.
  - `parsed`, which represents a parsed expression that hasn't yet been evaluated against a specific
    `PackageGraph`. This is a newly public type `ParsedExpr`.
  - `compiled`, which is an expression that has been compiled against a `PackageGraph`. This is of
    type `CompiledExpr`, which is what `FilteringExpr` used to be.
- Newlines are now supported within expressions, for e.g. multiline TOML for nextest's overrides.
- A clean, well-formatted representation of a parsed expression can now be generated via the
  `Display` impl on `ParsedExpr`.
- The parser has been extensively fuzzed. No bugs were found.
- MSRV updated to Rust 1.66.

## [0.3.0] - 2022-11-23

### Changed

- `guppy` updated to 0.15.0.
- MSRV updated to Rust 1.62.

## [0.2.2] - 2022-10-14

### Internal

- Updated private dependency [recursion](https://crates.io/crates/recursion) to 0.3.0. Thanks [Inanna](https://github.com/inanna-malick) for your contribution!

## [0.2.1] - 2022-07-30

### Internal

- Evaluation now uses a stack machine via the [recursion](https://crates.io/crates/recursion) crate. Thanks [Inanna](https://github.com/inanna-malick) for your first contribution!

## [0.2.0] - 2022-07-13

### Added

- The expression language supports several new [predicates](https://nexte.st/book/filter-expressions#basic-predicates):
  - `kind(name-matcher)`: include all tests in binary kinds (e.g. `lib`, `test`, `bench`) matching `name-matcher`.
  - `binary(name-matcher)`: include all tests in binary names matching `name-matcher`.
  - `platform(host)` or `platform(target)`: include all tests that are [built for the host or target platform](running.md#filtering-by-build-platform), respectively.

* It is now possible to evaluate a query without knowing the name of the test. The result is evaluated as a [three-valued logic (Kleene K3)](https://en.wikipedia.org/wiki/Three-valued_logic#Kleene_and_Priest_logics), and is returned as an `Option<bool>` where `None` indicates the unknown state.

### Changed

- The evaluator now takes a `TestQuery` struct, making it easier to add more parameters in the future.
- MSRV updated to Rust 1.59.

## [0.1.0] - 2022-04-16

Initial release.

[0.18.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.18.0
[0.17.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.17.0
[0.16.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.16.0
[0.15.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.15.0
[0.14.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.14.0
[0.13.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.13.0
[0.12.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.12.0
[0.11.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.11.0
[0.10.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.10.0
[0.9.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.9.0
[0.8.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.8.0
[0.7.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.7.1
[0.7.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.7.0
[0.6.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.6.0
[0.5.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.5.1
[0.5.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.5.0
[0.4.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.4.0
[0.3.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.3.0
[0.2.2]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.2.2
[0.2.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.2.1
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.2.0
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.1.0
