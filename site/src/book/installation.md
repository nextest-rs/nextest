# Quick start

Here's how to get quickly started with cargo-nextest.

## Installation

cargo-nextest works on Linux and other Unix-like OSes, macOS, and Windows. Get started by installing it from `crates.io`:

```sh
cargo install cargo-nextest
```

`cargo nextest` must be compiled and installed with **Rust 1.54** or later, but it can build and run tests against older versions of Rust.

> Tip: in GitHub Actions CI, you can use the [baptiste0928/cargo-install](https://github.com/marketplace/actions/cargo-install) GitHub Action to cache the cargo-nextest binary.

> TODO: Add pre-built binaries.

## Running tests

To build and run all tests in a workspace, cd into the workspace and run:

```
cargo nextest run
```

For more information about running tests, see [Running tests](running.md).
