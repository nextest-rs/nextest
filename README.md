# nextest

Nextest is a next-generation test runner for Rust. This repository contains the source code for:

* [**cargo-nextest**](cargo-nextest): a new, faster Cargo test runner [![Documentation (main)](https://img.shields.io/badge/docs-main-brightgreen)](https://nextest-rs.github.io/nextest/rustdoc/cargo_nextest/)
* libraries used by cargo-nextest:
  * [**nextest-runner**](nextest-runner): core logic for cargo-nextest [![Documentation (main)](https://img.shields.io/badge/docs-main-brightgreen)](https://nextest-rs.github.io/nextest/rustdoc/nextest_runner/)
  * [**nextest-metadata**](nextest-metadata): library for calling cargo-nextest over the command line [![Documentation (main)](https://img.shields.io/badge/docs-main-brightgreen)](https://nextest-rs.github.io/nextest/rustdoc/nextest_metadata/)
* [**quick-junit**](quick-junit): a data model, serializer (and in the future deserializer) for JUnit/XUnit XML [![quick-junit on crates.io](https://img.shields.io/crates/v/quick-junit)](https://crates.io/crates/quick-junit) [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/quick-junit/) [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nextest-rs.github.io/nextest/rustdoc/quick_junit/)

## Minimum supported Rust version

The minimum supported Rust version is **Rust 1.54.**

While a crate is pre-release status (0.x.x) it may have its MSRV bumped in a patch release. Once a
crate has reached 1.x, any MSRV bump will be accompanied with a new minor version.

## Contributing

See the [CONTRIBUTING](CONTRIBUTING.md) file for how to help out.

## License

This project is available under the terms of either the [Apache 2.0 license](LICENSE-APACHE) or the [MIT
license](LICENSE-MIT).

This project is derived from [diem-devtools](https://github.com/diem/diem-devtools/). Upstream
source code is used under the terms of the [Apache 2.0
license](https://github.com/diem/diem-devtools/blob/main/LICENSE-APACHE) and the [MIT
license](https://github.com/diem/diem-devtools/blob/main/LICENSE-MIT).
