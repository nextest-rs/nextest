# Usage

This section covers usage, features and options for cargo-nextest.

## Basic usage

To build and run all tests in a workspace, cd into the workspace and run:

```
cargo nextest run
```

For more information about running tests, see [Running tests](running.md).

## Limitations

* The [nextest execution model](how-it-works.md) means that **each individual test is executed as a separate process**. Tests that depend on being executed within the same process [may not work correctly](https://github.com/nextest-rs/nextest/issues/27).

    To work around this, consider combining those tests into one so that nextest runs them as a
    unit, or excluding those tests from nextest.
* There's [no way](https://github.com/nextest-rs/nextest/issues/28) to mark a particular test binary as excluded from nextest.
* Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. Locally and in CI, after `cargo nextest run`, use `cargo test --doc` to run all doctests.
