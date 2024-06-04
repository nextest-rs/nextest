# cargo-nextest

Welcome to the home page for **cargo-nextest**, a next-generation test runner for Rust projects.

## Features

<div id="nextest-features" markdown>

<img src="static/cover.png" id="nextest-cover" />

- **Clean, beautiful user interface.** Nextest presents its results concisely so you can see which tests passed and failed at a glance.
- **[Up to 3× as fast](docs/benchmarks/index.md) as cargo test.** Nextest uses a [state-of-the-art execution model](docs/design/how-it-works.md) for faster, more reliable test runs.
- **Identify [slow](docs/features/slow-tests.md) and [leaky](docs/features/leaky-tests.md) tests.** Use nextest to detect misbehaving tests, identify bottlenecks during test execution, and optionally terminate tests if they take too long.
- **Filter tests using an embedded language.** Use powerful [filtersets](docs/filtersets/index.md) to specify granular subsets of tests on the command-line, and to enable [per-test overrides](docs/configuration/per-test-overrides.md).
- **Configure [per-test settings](docs/configuration/per-test-overrides.md)**. [Automatically retry](docs/features/retries.md#per-test-overrides) subsets of tests, mark them as [heavy](docs/configuration/threads-required.md), or [run them serially](docs/configuration/test-groups.md).
- **Designed for CI.** Nextest addresses real-world pain points in continuous integration scenarios:
  - Use **[pre-built binaries](docs/installation/pre-built-binaries.md)** for quick installation.
  - Set up CI-specific **[configuration profiles](docs/configuration/index.md)**.
  - **[Reuse builds](docs/ci-features/archiving.md)** and **[partition test runs](docs/ci-features/partitioning.md)** across multiple CI jobs. (Check out [this example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml) on GitHub Actions).
  - [**Automatically retry**](docs/features/retries.md) failing tests, and mark them as flaky if they pass later.
  - Print failing output **[at the end of test runs](docs/reporting.md)**.
  - Output information about test runs as **[JUnit XML](docs/machine-readable/junit.md)**, and [in other formats](docs/machine-readable/index.md).
- **Cross-platform.** Nextest works on Linux and other Unixes, macOS, and Windows, so you get its benefits no matter what platform you use.

</div>

## Quick start

Install cargo-nextest for your platform using the [pre-built binaries](docs/installation/pre-built-binaries.md).

Run all tests in a workspace:

```
cargo nextest run
```

> **Note:** Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. For now, run doctests in a separate step with `cargo test --doc`.

## Crates in this project

| Crate                                                             |                    crates.io                    |            rustdoc (latest version)             |             rustdoc (main)             |
| ----------------------------------------------------------------- | :---------------------------------------------: | :---------------------------------------------: | :------------------------------------: |
| **cargo-nextest,** the main test binary                           |   [![cargo-nextest on crates.io][cnci]][cncl]   | [![Documentation (latest release)][doci]][cndl] | [![Documentation (main)][docmi]][cnml] |
| **nextest-runner,** core nextest logic                            |  [![nextest-runner on crates.io][nrci]][nrcl]   | [![Documentation (latest release)][doci]][nrdl] | [![Documentation (main)][docmi]][nrml] |
| **nextest-metadata,** parsers for machine-readable output         | [![nextest-metadata on crates.io][nmci]][nmcl]  | [![Documentation (latest release)][doci]][nmdl] | [![Documentation (main)][docmi]][nmml] |
| **nextest-filtering,** parser and evaluator for [filtersets]      | [![nextest-filtering on crates.io][nfci]][nfcl] | [![Documentation (latest release)][doci]][nfdl] | [![Documentation (main)][docmi]][nfml] |
| **quick-junit,** JUnit XML serializer                             |    [![quick-junit on crates.io][qjci]][qjcl]    | [![Documentation (latest release)][doci]][qjdl] | [![Documentation (main)][docmi]][qjml] |
| **datatest-stable,** [custom test harness] for data-driven tests  |  [![datatest-stable on crates.io][dsci]][dscl]  | [![Documentation (latest release)][doci]][dsdl] | [![Documentation (main)][docmi]][dsml] |
| **future-queue,** run queued futures with global and group limits |   [![future-queue on crates.io][fqci]][fqcl]    | [![Documentation (latest release)][doci]][fqdl] | [![Documentation (main)][docmi]][fqml] |

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
[nfci]: https://img.shields.io/crates/v/nextest-filtering
[nfcl]: https://crates.io/crates/nextest-filtering
[nfdl]: https://docs.rs/nextest-filtering
[nfml]: https://nexte.st/rustdoc/nextest_filtering
[qjci]: https://img.shields.io/crates/v/quick-junit
[qjcl]: https://crates.io/crates/quick-junit
[qjdl]: https://docs.rs/quick-junit
[qjml]: https://quick-junit.nexte.st
[dsci]: https://img.shields.io/crates/v/datatest-stable
[dscl]: https://crates.io/crates/datatest-stable
[dsdl]: https://docs.rs/datatest-stable
[dsml]: https://datatest-stable.nexte.st
[fqci]: https://img.shields.io/crates/v/future-queue
[fqcl]: https://crates.io/crates/future-queue
[fqdl]: https://docs.rs/future-queue
[fqml]: https://nextest-rs.github.io/future-queue/rustdoc/future_queue/
[filtersets]: docs/filtersets/index.md
[custom test harness]: docs/design/custom-test-harnesses.md
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
