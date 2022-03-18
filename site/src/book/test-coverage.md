# Test coverage

Test coverage support is provided by third-party tools that wrap around nextest.

## llvm-cov

[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) supports nextest. To generate llvm-cov data with nextest, run:

```
cargo install cargo-llvm-cov
cargo llvm-cov nextest
```

### Using llvm-cov in GitHub Actions

Install both nextest and llvm-cov in GitHub Actions, then run `cargo llvm-cov nextest`.

```yaml
- uses: taiki-e/install-action@cargo-llvm-cov
- uses: taiki-e/install-action@nextest
```

## Integrating nextest into coverage tools

Most coverage tools work by setting a few environment variables such as `RUSTFLAGS` or `RUSTC_WRAPPER`. Nextest runs Cargo for the build, which will read those environment variables as usual. This means that it should generally be quite straightforward to integrate nextest into other coverage tools.

> If you've integrated nextest into a coverage tool, feel free to [submit a pull request] with documentation.

[submit a pull request]: https://github.com/nextest-rs/nextest/pulls
