# Coverage

[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) has support for nextest.
Simply run the following command to generate coverage report with nextest:

```
cargo install cargo-llvm-cov
cargo llvm-cov nextest
```

## GitHub Action

You may install cargo-llvm-cov and nextest on GitHub Action, and then run llvm-cov with nextest.

```yaml
- uses: taiki-e/install-action@cargo-llvm-cov
- uses: taiki-e/install-action@nextest
```
