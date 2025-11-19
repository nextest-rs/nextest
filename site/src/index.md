---
title: Home
description: A next-generation test runner for Rust.
icon: material/home
---

# cargo-nextest

Welcome to the home page for **cargo-nextest**, a next-generation test runner for Rust projects.

## Features

<div class="grid cards" markdown>

-   :octicons-sparkles-fill-16:{ .lg .middle } __Clean, beautiful user interface__

    ---

    <img src="static/cover.png" id="nextest-cover" />

    See which tests passed and failed at a glance.

    [:octicons-arrow-right-24: Running tests](docs/running.md)

-   :material-clock-fast:{ .lg .middle } __Up to 3x as fast as cargo test__

    ---

    Nextest uses a modern [execution model](docs/design/how-it-works.md) for faster, more reliable test runs.

    [:octicons-arrow-right-24: Benchmarks](docs/benchmarks/index.md)

-   :material-filter-variant:{ .lg .middle } __Powerful test selection__

    ---

    Use a sophisticated [expression language](docs/filtersets/index.md) to select exactly the tests you need. Filter by name, binary, platform, or any combination.

    [:octicons-arrow-right-24: Filtersets](docs/filtersets/index.md)

-   :material-speedometer-slow:{ .lg .middle } __Identify misbehaving tests__

    ---

    Treat tests as cattle, not pets. Detect and terminate [slow tests](docs/features/slow-tests.md). Loop over tests many times with [stress testing](docs/features/stress-tests.md).

    [:octicons-arrow-right-24: Slow tests and timeouts](docs/features/slow-tests.md)

-   :material-chevron-double-right:{ .lg .middle } __Customize settings by test__

    ---

    Automatically [retry](docs/features/retries.md) some tests, mark them as [heavy](docs/configuration/threads-required.md), run them [serially](docs/configuration/test-groups.md), and much more.

    [:octicons-arrow-right-24: Per-test settings](docs/configuration/per-test-overrides.md)

-   :octicons-git-merge-24:{ .lg .middle } __Designed for CI__

    ---

    [Archive](docs/ci-features/archiving.md) and [partition](docs/ci-features/partitioning.md) tests across multiple workers, export [JUnit XML](docs/machine-readable/junit.md), and use [profiles](docs/configuration/index.md#profiles) for different environments.

    [:octicons-arrow-right-24: Configuration profiles](docs/configuration/index.md#profiles)

-   :material-vector-combine:{ .lg .middle } __An ecosystem of tools__

    ---

    Collect [test coverage](docs/integrations/test-coverage.md). Do [mutation testing](docs/integrations/cargo-mutants.md). Spin up [debuggers](docs/integrations/debuggers-tracers.md). Observe system behavior with [DTrace and bpftrace probes](docs/integrations/usdt.md).

    [:octicons-arrow-right-24: Integrations](docs/integrations/index.md)

-   :material-language-rust:{ .lg .middle } __Cross-platform__

    ---

    Runs on Linux, Mac, Windows, and other Unix-like systems. Download binaries or build it [from source](docs/installation/from-source.md).

    [:octicons-arrow-right-24: Pre-built binaries](docs/installation/pre-built-binaries.md)

-   :material-scale-balance:{ .lg .middle } __Open source, widely trusted__

    ---

    Powers Rust development at every scale, from independent open source projects to the world's largest tech companies.

    [:octicons-arrow-right-24: License (Apache 2.0)](https://github.com/nextest-rs/nextest/blob/main/LICENSE-APACHE)

-   :material-heart-circle:{ .lg .middle } __State-of-the-art, made with love__

    ---

    Nextest brings [infrastructure-grade reliability](docs/design/why-process-per-test.md) to test runners, [with _care_](docs/design/architecture/runner-loop.md) about getting the details right.

    [:octicons-arrow-right-24: Sponsor on GitHub](https://github.com/sponsors/sunshowers)

</div>

## Quick start

Install cargo-nextest for your platform using the [pre-built binaries](docs/installation/pre-built-binaries.md).

For most Rust projects, nextest works out of the box. To run all tests in a workspace:

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

For information about code signing, see [*Code signing policy*](docs/installation/pre-built-binaries.md#code-signing-policy).

This document is licensed under [CC BY 4.0]. This means that you are welcome to share, adapt or
modify this material as long as you give appropriate credit.

[CC BY 4.0]: https://creativecommons.org/licenses/by/4.0/
