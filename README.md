# Nextest

Nextest is a next-generation test runner for Rust. For more information, **check out [the website](https://nexte.st/)**.

This repository contains the source code for:

* [**cargo-nextest**](cargo-nextest): a new, faster Cargo test runner
  [![cargo-nextest on crates.io](https://img.shields.io/crates/v/cargo-nextest)](https://crates.io/crates/cargo-nextest)
  [![Documentation (website)](https://img.shields.io/badge/docs-nexte.st-blue)](https://nexte.st)
* libraries used by cargo-nextest:
  * [**nextest-runner**](nextest-runner): core logic for cargo-nextest
    [![nextest-runner on crates.io](https://img.shields.io/crates/v/nextest-runner)](https://crates.io/crates/nextest-runner)
    [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/nextest-runner)
    [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/nextest_runner/)
  * [**nextest-metadata**](nextest-metadata): library for calling cargo-nextest over the command line
    [![nextest-metadata on crates.io](https://img.shields.io/crates/v/nextest-metadata)](https://crates.io/crates/nextest-metadata)
    [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/nextest-metadata)
    [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/nextest_metadata)
  * [**nextest-filtering**](nextest-filtering): parser and evaluator for [filter expressions](https://nexte.st/book/filter-expressions)
    [![nextest-filtering on crates.io](https://img.shields.io/crates/v/nextest-filtering)](https://crates.io/crates/nextest-filtering)
    [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/nextest-filtering)
    [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/nextest_filtering)
* [**quick-junit**](quick-junit): a data model, serializer (and in the future deserializer) for JUnit/XUnit XML
  [![quick-junit on crates.io](https://img.shields.io/crates/v/quick-junit)](https://crates.io/crates/quick-junit)
  [![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen)](https://docs.rs/quick-junit/)
  [![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/quick_junit/)

## Minimum supported Rust version

The minimum supported Rust version to *run* nextest with is **Rust 1.38.** Nextest is not tested against versions that are that old, but it should work with any version of Rust released in the past year. (Please report a bug if not!)

The minimum supported Rust version to *build* nextest with is **Rust 1.70.** For building, at least the last 3 versions of stable Rust are supported at any given time.

See the [stability policy](https://nexte.st/book/stability) for more details.

While a crate is pre-release status (0.x.x) it may have its MSRV bumped in a patch release. Once a
crate has reached 1.x, any MSRV bump will be accompanied with a new minor version.

## Contributing

See the [CONTRIBUTING](CONTRIBUTING.md) file for how to help out.

*Looking to contribute to nextest and don't know where to get started?* Check out the list of [good first issues](https://github.com/nextest-rs/nextest/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc+label%3A%22good+first+issue%22).

## License

**Nextest is Free Software.** This project is available under the terms of either the [Apache 2.0 license](LICENSE-APACHE) or the [MIT
license](LICENSE-MIT).

**Like all Free Software, nextest is a gift.** Nextest is provided on an "AS IS" basis and there is NO WARRANTY attached to it. As a user, please treat the authors and contributors to this project as if you were treating the giver of a gift. In particular, you're asked to follow the [code of conduct](CODE_OF_CONDUCT.md).

This project is derived from [diem-devtools](https://github.com/diem/diem-devtools/). Upstream
source code is used under the terms of the [Apache 2.0
license](https://github.com/diem/diem-devtools/blob/main/LICENSE-APACHE) and the [MIT
license](https://github.com/diem/diem-devtools/blob/main/LICENSE-MIT).

## macOS support

macOS is supported through the MacStadium Open Source Developer Program.

![image](https://uploads-ssl.webflow.com/5ac3c046c82724970fc60918/5c019d917bba312af7553b49_MacStadium-developerlogo.png)
