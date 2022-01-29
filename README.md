# diem-devtools

This repository contains the source code for developer tools and libraries built for
[Diem Core](https://github.com/diem/diem/). Currently, this includes:

* [**nextest**](nextest): a new, faster Cargo test runner [![Documentation (main)](https://img.shields.io/badge/docs-main-brightgreen)](https://diem.github.io/diem-devtools/rustdoc/nextest-runner/)
* [**quick-junit**](quick-junit): a data model, serializer (and in the future deserializer) for JUnit/XUnit XML [![quick-junit on crates.io](https://img.shields.io/crates/v/quick-junit)](https://crates.io/crates/quick-junit) [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/quick-junit/) [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://diem.github.io/diem-devtools/rustdoc/quick_junit/)
* [**datatest-stable**](datatest-stable): data-driven testing on stable Rust [![datatest-stable on crates.io](https://img.shields.io/crates/v/datatest-stable)](https://crates.io/crates/datatest-stable) [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/datatest-stable/) [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://diem.github.io/diem-devtools/rustdoc/datatest_stable/) 

## Minimum supported Rust version

These crates target the latest stable version of Rust.

While a crate is pre-release status (0.x.x) it may have its MSRV bumped in a patch release. Once a crate has reached
1.x, any MSRV bump will be accompanied with a new minor version.

## Contributing

See the [CONTRIBUTING](CONTRIBUTING.md) file for how to help out.

## License

This project is available under the terms of either the [Apache 2.0 license](LICENSE-APACHE) or the [MIT
license](LICENSE-MIT).
