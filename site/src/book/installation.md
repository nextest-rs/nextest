# Installation and usage

## Installation

cargo-nextest works on Linux and other Unix-like OSes, macOS, and Windows.

### Installing from crates.io

Run the following command:

```
cargo install cargo-nextest
```

`cargo nextest` must be compiled and installed with **Rust 1.54** or later, but it can build and run
tests against any version of Rust.

> TODO: Add pre-built binaries.

### Using a cached install in CI

Most CI users of nextest will benefit from using cached binaries. If your CI is based on GitHub
Actions, you may use the
[baptiste0928/cargo-install](https://github.com/marketplace/actions/cargo-install) action to cache
the cargo-nextest binary.

```yml
jobs:
  ci:
    # ...
    steps:
      - uses: actions/checkout@v2
      # Install a Rust toolchain here.
      - name: Install cargo-nextest
        uses: baptiste0928/cargo-install@v1
        with:
          crate: cargo-nextest
          version: 0.9
      # At this point, cargo-nextest will be available on your PATH
```

Also consider using the [Swatinem/rust-cache](https://github.com/marketplace/actions/rust-cache)
action to make your builds faster.

### Installing from GitHub

Install the latest, in-development version of cargo-nextest from the GitHub repository:

```
cargo install --git https://github.com/nextest-rs/nextest --bin cargo-nextest
```

## Basic usage

To build and run all tests in a workspace, cd into the workspace and run:

```
cargo nextest run
```

For more information about running tests, see [Running tests](running.md).

## Doctests

Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. In CI and locally, run `cargo test --doc` to run all doctests.
