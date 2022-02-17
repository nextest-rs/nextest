# Installing from source

If pre-built binaries are not available for your platform, or you'd like to otherwise install cargo-nextest from source, here's what you need to do:

## Installing from crates.io

Run the following command:

```
cargo install cargo-nextest
```

`cargo nextest` must be compiled and installed with **Rust 1.54** or later, but it can build and run
tests against any version of Rust.

## Using a cached install in CI

Most CI users of nextest will benefit from using cached binaries. Consider using the [pre-built binaries](pre-built-binaries.md) for this purpose. [See this example of how the nextest repository uses pre-built binaries.](https://github.com/nextest-rs/nextest/blob/e43ac449f53fd34e58136cd94b7a72add201fe5a/.github/workflows/ci.yml#L104-L107)

If your CI is based on GitHub Actions, you may use the
[baptiste0928/cargo-install](https://github.com/marketplace/actions/cargo-install) action to build cargo-nextest from source and cache
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

## Installing from GitHub

Install the latest, in-development version of cargo-nextest from the GitHub repository:

```
cargo install --git https://github.com/nextest-rs/nextest --bin cargo-nextest
```
