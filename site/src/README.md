# cargo-nextest

Welcome to the home page for **cargo-nextest**, a next-generation test runner for Rust projects.

## Features

<img src="static/cover.png" id="nextest-cover">

* **Clean, beautiful user interface.** nextest presents its results concisely so you can see which tests passed and failed at a glance.
* **[Up to 60% faster](book/benchmarks.md) than cargo test.** Nextest uses a [state-of-the-art execution model](book/how-it-works.md) for faster, more reliable test runs.
* **Detect flaky tests.** Nextest can [automatically retry](book/retries.md) failing tests for you, and if they pass later nextest will mark them as flaky.
* **Partition test runs across several CI jobs.** If your tests take too long to run in CI, nextest can automatically split them up for you across several jobs.
* **Cross-platform.** nextest works on Unix, Mac and Windows, so you get the benefits of faster test runs no matter what platform you use.
* ... and more [coming soon](https://github.com/nextest-rs/nextest/projects/1)!

## Quick start

Install cargo-nextest from crates.io (requires **Rust 1.54** or later):

```
cargo install cargo-nextest
```

Run all tests in a workspace:

```
cargo nextest run
```

For more detailed installation instructions, see [Installation and usage](book/installation.md).

## Crates in this project

| Crate                                                     |                    crates.io                   |             rustdoc (latest version)            |             rustdoc (main)             |
|-----------------------------------------------------------|:----------------------------------------------:|:-----------------------------------------------:|:--------------------------------------:|
| **cargo-nextest,** the main test binary                   | [![cargo-nextest on crates.io][cnci]][cncl]    | [![Documentation (latest release)][doci]][cndl] | [![Documentation (main)][docmi]][cnml] |
| **nextest-runner,** core nextest logic                    | [![nextest-runner on crates.io][nrci]][nrcl]   | [![Documentation (latest release)][doci]][nrdl] | [![Documentation (main)][docmi]][nrml] |
| **nextest-metadata,** parsers for machine-readable output | [![nextest-metadata on crates.io][nmci]][nmcl] | [![Documentation (latest release)][doci]][nmdl] | [![Documentation (main)][docmi]][nmml] |
| **quick-junit,** JUnit XML serializer                     | [![quick-junit on crates.io][qjci]][qjcl]      | [![Documentation (latest release)][doci]][qjcl] | [![Documentation (main)][docmi]][qjml] |

[cnci]: https://img.shields.io/crates/v/cargo-nextest
[cncl]: https://crates.io/crates/cargo-nextest
[cndl]: https://docs.rs/cargo-nextest
[cnml]: https://nexte.st/rustdoc/cargo_nextest

[nrci]: https://img.shields.io/crates/v/nextest-runner
[nrcl]: https://crates.io/crates/nextest-runner
[nrdl]: https://docs.rs/nextest-runner
[nrml]: https://nexte.st/rustdoc/nextest_runner

[nmci]: https://img.shields.io/crates/v/nextest-metadata
[nmcl]: https://crates.io/crates/nextest-metadata
[nmdl]: https://docs.rs/nextest-metadata
[nmml]: https://nexte.st/rustdoc/nextest_metadata

[qjci]: https://img.shields.io/crates/v/quick-junit
[qjcl]: https://crates.io/crates/quick-junit
[qjdl]: https://docs.rs/quick-junit
[qjml]: https://nexte.st/rustdoc/quick_junit

[doci]: https://img.shields.io/badge/docs-latest-brightgreen
[docmi]: https://img.shields.io/badge/docs-main-purple

## Contributing

The source code for nextest and this site are hosted on GitHub, at
[https://github.com/nextest-rs/nextest](https://github.com/nextest-rs/nextest).

Contributions are welcome! Please see the [CONTRIBUTING
file](https://github.com/nextest-rs/nextest/blob/main/CONTRIBUTING.md) for how to help out.

## License

The source code for nextest is licensed under the
[MIT](https://github.com/nextest-rs/nextest/blob/main/LICENSE-MIT) and [Apache
2.0](https://github.com/nextest-rs/nextest/blob/main/LICENSE-APACHE) licenses.

This document is licensed under [CC BY 4.0]. This means that you are welcome to share, adapt or
modify this material as long as you give appropriate credit.

[CC BY 4.0]: https://creativecommons.org/licenses/by/4.0/
