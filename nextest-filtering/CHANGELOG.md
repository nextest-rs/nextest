# Changelog

## [0.2.1] - 2022-07-30

### Internal

- Evaluation now uses a stack machine via the [recursion](https://crates.io/crates/recursion) crate. Thanks [Inanna](https://github.com/inanna-malick) for your first contribution!

## [0.2.0] - 2022-07-13

### Added

* The expression language supports several new [predicates](https://nexte.st/book/filter-expressions#basic-predicates):
  - `kind(name-matcher)`: include all tests in binary kinds (e.g. `lib`, `test`, `bench`) matching `name-matcher`.
  - `binary(name-matcher)`: include all tests in binary names matching `name-matcher`.
  - `platform(host)` or `platform(target)`: include all tests that are [built for the host or target platform](running.md#filtering-by-build-platform), respectively.
- It is now possible to evaluate a query without knowing the name of the test. The result is evaluated as a [three-valued logic (Kleene K3)](https://en.wikipedia.org/wiki/Three-valued_logic#Kleene_and_Priest_logics), and is returned as an `Option<bool>` where `None` indicates the unknown state.

### Changed

- The evaluator now takes a `TestQuery` struct, making it easier to add more parameters in the future.
- MSRV updated to Rust 1.59.

## [0.1.0] - 2022-04-16

Initial release.

[0.2.1]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.2.1
[0.2.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.2.0
[0.1.0]: https://github.com/nextest-rs/nextest/releases/tag/nextest-filtering-0.1.0
