# Installing from source

If pre-built binaries are not available for your platform, or you'd otherwise like to install cargo-nextest from source, here's what you need to do:

## Installing from crates.io

Run the following command:

```
cargo install cargo-nextest --locked
```

`cargo nextest` must be compiled and installed with **Rust 1.62** or later (see [Stability policy] for more), but it can build and run
tests against any version of Rust.

[Stability policy]: stability.md#minimum-supported-rust-version-msrv

## Using a cached install in CI

Most CI users of nextest will benefit from using cached binaries. Consider using the [pre-built binaries](pre-built-binaries.md) for this purpose.

[See this example for how the nextest repository uses pre-built binaries.](https://github.com/nextest-rs/nextest/blob/0eadcdfa349ff36354de464ecf6002d89ff50fe6/.github/workflows/ci.yml#L124-L125).

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
          locked: true
          # Uncomment the following line if you'd like to stay on the 0.9 series
          # version: 0.9
      # At this point, cargo-nextest will be available on your PATH
```

Also consider using the [Swatinem/rust-cache](https://github.com/marketplace/actions/rust-cache)
action to make your builds faster.

## Installing from GitHub

Install the latest, in-development version of cargo-nextest from the GitHub repository:

```
cargo install --git https://github.com/nextest-rs/nextest --bin cargo-nextest
```
