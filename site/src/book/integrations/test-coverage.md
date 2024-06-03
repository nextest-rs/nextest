---
icon: material/chart-donut
---

# Test coverage

Test coverage support is provided by third-party tools that wrap around nextest.

## llvm-cov

[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) supports nextest. To generate llvm-cov data with nextest, run:

```
cargo install cargo-llvm-cov
cargo llvm-cov nextest
```

### Using llvm-cov in GitHub Actions

Install Rust with the `llvm-tools-preview` component, nextest, and llvm-cov in GitHub Actions. Then, run `cargo llvm-cov nextest`.

```yaml
- uses: dtolnay/rust-toolchain@stable
  with:
    components: llvm-tools-preview
- uses: taiki-e/install-action@cargo-llvm-cov
- uses: taiki-e/install-action@nextest

- name: Collect coverage data
  run: cargo llvm-cov nextest
```

### Collecting coverage data from doctests

Nextest doesn't currently support doctests, so coverage data from nextest must be [merged](https://github.com/taiki-e/cargo-llvm-cov?tab=readme-ov-file#merge-coverages-generated-under-different-test-conditions) with doctest data.

Here's an example GitHub Actions configuration:

```yaml
# Nightly Rust is required for cargo llvm-cov --doc.
- uses: dtolnay/rust-toolchain@nightly
  with:
    components: llvm-tools-preview
- uses: taiki-e/install-action@cargo-llvm-cov
- uses: taiki-e/install-action@nextest

- name: Collect coverage data (including doctests)
  run: |
    cargo llvm-cov --no-report nextest
    cargo llvm-cov --no-report --doc
    cargo llvm-cov report --doctests --lcov --output-path lcov.info
```

### Reporting to an external coverage service

External services like [Codecov.io](https://about.codecov.io/) can be used to collect and display coverage data. Codecov is free for open source projects, and supports `lcov.info` files.

After generating an `lcov.info` file, upload it to Codecov with:

```yaml
- name: Upload coverage data to codecov
  uses: codecov/codecov-action@v3
  with:
    files: lcov.info
```

### Example

Nextest itself uses the above mechanisms to collect coverage for its project. The config is located in [`.github/workflows/coverage.yml`](https://github.com/nextest-rs/nextest/blob/main/.github/workflows/coverage.yml).

## Integrating nextest into coverage tools

Most coverage tools work by setting a few environment variables such as `RUSTFLAGS` or `RUSTC_WRAPPER`. Nextest runs Cargo for the build, which will read those environment variables as usual. This means that it should generally be quite straightforward to integrate nextest into other coverage tools.

> If you've integrated nextest into a coverage tool, feel free to [submit a pull request] with documentation.

[submit a pull request]: https://github.com/nextest-rs/nextest/pulls
