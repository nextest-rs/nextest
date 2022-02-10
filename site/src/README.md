# cargo-nextest

Welcome to the home page for cargo-nextest, a next-generation test runner for Rust projects.

## Features

* Nextest is **up to 60% faster** than cargo test! Nextest uses a [state-of-the-art execution model](book/how-it-works.md) for faster, more reliable test runs.
* **Detect flaky tests.** Nextest can [automatically retry](book/retries.md) failing tests for you, and if they pass later nextest will mark them as flaky.
* **Partitioning test runs across several machines.** If your tests take too long on one machine in CI, nextest can automatically shard them for you across several jobs.
* **Cross-platform.** nextest works on Unix, Mac and Windows, so you get the benefits of faster test runs no matter what platform you use.
* ... and more coming soon!

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

## Contributing

The source code for nextest and this site are hosted on GitHub, at
[https://github.com/nextest-rs/nextest](https://github.com/nextest-rs/nextest). Contributions are
welcome! Please see the [CONTRIBUTING
file](https://github.com/nextest-rs/nextest/blob/main/CONTRIBUTING.md) for how to help out.

## License

The source code for nextest is licensed under the
[MIT](https://github.com/nextest-rs/nextest/blob/main/LICENSE-MIT) and [Apache
2.0](https://github.com/nextest-rs/nextest/blob/main/LICENSE-APACHE) licenses.

This document is licensed under [CC BY 4.0]. This means that you are welcome to share, adapt or
modify this material as long as you give appropriate credit.

[CC BY 4.0]: https://creativecommons.org/licenses/by/4.0/
