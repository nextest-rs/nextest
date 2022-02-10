# Running tests

To build and run all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest run
```

This will produce output that looks like:

<img src="https://user-images.githubusercontent.com/180618/153310973-a6d8d37f-5978-4231-ae5a-ee38ed008def.png"/>

In the output above:
* Tests are marked **`PASS`** or **`FAIL`**, and the amount of wall-clock time each test takes is listed within square brackets. For example, **`test_list_tests`** passed and took 0.603 seconds to execute.
* The part of the test in purple is the *test binary*. A test binary is either:
  * a *unit test binary* built from tests inline within `lib.rs`. These test binaries are shown by nextest as just the crate name, without a `::` separator inside them.
  * an *integration test binary* built from tests in the `[[test]]` section of `Cargo.toml` (typically tests in the `tests` directory.) These tests are shown by nextest in the format `crate-name::bin-name`.

  For more about unit and integration tests, see [the documentation for `cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html).
* The part after the test binary is the *test name*, including the module the test is in. The final part of the test name is highlighted in bold blue text.

`cargo nextest run` supports all the options that `cargo test` does. For example, to only execute tests for a package called `my-package`:

```
cargo nextest run -p my-package
```

For a full list of options accepted by `cargo nextest run`, see `cargo nextest run --help`.

### Filtering tests

To only run tests that match certain names:

```
cargo nextest run <test-name1> <test-name2>...
```

This is different from `cargo test`, where you have to specify a `--`, for example: `cargo test -- <test-name1> <test-name2>...`.

### Displaying live test output

By default, `cargo nextest run` will capture test output and only display it on failure. If you do *not* want to capture test output:

```
cargo nextest run --no-capture
```

In this mode, cargo-nextest will run tests *serially* so that output from different tests isn't interspersed. This is different from `cargo test -- --nocapture`, which will run tests in parallel.

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust.
