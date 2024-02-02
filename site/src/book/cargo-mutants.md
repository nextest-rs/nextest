# Mutation testing with cargo-mutants

[cargo-mutants](https://mutants.rs/) helps finds gaps in your test coverage by injecting bugs and observing if your tests catch them.

This is complementary to coverage testing: coverage measures whether the tests execute your code, while mutation testing measures whether the tests depend on the results and side-effects of your code.

Mutation testing is typically slower than coverage testing because it runs the test suite many times, once for each mutation, but it is potentially more effective at finding gaps in your test suite, easier to set up, and may produce results that are easier to interpret or act on.

## Using cargo-mutants with nextest

First, install cargo-mutants with `cargo install cargo-mutants`.

cargo-mutants has a `--test-tool=nextest` command line option to run the tests with nextest. You can use this if your project's test suite requires, recommends, or is faster under nextest. For example,

```sh
cargo mutants --test-tool=nextest
```

If your tree should always be built with nextest, you can configure this in `.cargo/mutants.toml`:

```toml
test_tool = "nextest"
```

The [cargo-mutants documentation](https://mutants.rs/) has more information on how to use cargo-mutants and how to interpret its results, including examples of how to configure it in CI.
