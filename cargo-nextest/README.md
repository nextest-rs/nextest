# cargo-nextest

[![cargo-nextest on crates.io](https://img.shields.io/crates/v/cargo-nextest)](https://crates.io/crates/cargo-nextest)
[![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen.svg)](https://docs.rs/cargo-nextest/)
[![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/cargo_nextest)
[![Changelog](https://img.shields.io/badge/changelog-latest-blue)](https://nexte.st/CHANGELOG)
[![License](https://img.shields.io/badge/license-Apache-green.svg)](LICENSE-APACHE)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE-MIT)

cargo-nextest is a next-generation test runner for Rust.

For documentation and usage, see [the nextest site](https://nexte.st).

## Installation

To install nextest binaries (quicker), see [_Pre-built
binaries_](https://nexte.st/docs/installation/pre-built-binaries).

To install from source, run:

```sh
cargo install --locked cargo-nextest
```

**The `--locked` flag is required.** Builds without `--locked` are, and will
remain, broken.

## Minimum supported Rust versions

Nextest has two minimum supported Rust versions (MSRVs): one for _building_
nextest itself, and one for _running tests_ with `cargo nextest run`.

For more information about the MSRVs and the stability policy around them,
see [_Minimum supported Rust
versions_](https://nexte.st/docs/stability/#minimum-supported-rust-versions)
on the nextest site.

## Contributing

See the [contributing guide](https://nexte.st/docs/contributing/) for how to help out.

## License

This project is available under the terms of either the [Apache 2.0 license](../LICENSE-APACHE) or
the [MIT license](../LICENSE-MIT).

<!--
README.md is generated from README.tpl by cargo readme. To regenerate, run from the repository root:

./scripts/regenerate-readmes.sh
-->
