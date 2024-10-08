---
icon: material/run-fast
---

# Running tests

To build and run all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest run
```

This will produce output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/run-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/run-output.ansi | ../scripts/strip-ansi.sh
    ```

In nextest's run output:

- Tests are marked **`PASS`** or **`FAIL`**, and the amount of wall-clock time each test takes is listed within square brackets.
- Tests that take more than a specified amount of time (60 seconds by default) are marked **SLOW**. See [Slow tests and timeouts](features/slow-tests.md).
- The part of the test in magenta is the _binary ID_ for a unit test binary (see [Binary IDs](#binary-ids) below).

- The part after the binary ID is the _test name_, including the module the test is in. The final part of the test name is highlighted in bold blue text.

`cargo nextest run` supports all the options that `cargo test` does. For example, to only execute tests for a package called `my-package`:

```
cargo nextest run -p my-package
```

For a full list of options accepted by `cargo nextest run`, see [Options and arguments](#options-and-arguments) below, or `cargo nextest run --help`.

## Binary IDs

A test binary can be any of:

- A _unit test binary_ built from tests within `lib.rs` or its submodules. The binary ID for these are shown by nextest as just the crate name, without a `::` separator inside them.
- An _integration test binary_ built from tests in the `[[test]]` section of `Cargo.toml` (typically tests in the `tests` directory.) The binary ID for these is has the format `crate-name::bin-name`.
- Some other kind of test binary, such as a benchmark. In this case, the binary ID is `crate-name::kind/bin-name`. For example, `nextest-runner::bench/my-bench` or `quick-junit::example/show-junit`.

For more about unit and integration tests, see [the documentation for `cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html).

## Filtering tests

To only run tests that match certain names:

```
cargo nextest run <test-name1> <test-name2>...
```

### Filtersets

Tests can also be selected using the [filterset DSL]. See that page for more information.

For example, to run all tests except those in the `very-slow-tests` crate:

```
cargo nextest run -E 'not package(very-slow-tests)'
```

### Running a subset of tests by default

<!-- md:version 0.9.77 -->

By default, all discovered, non-ignored tests are run. To only run some tests by default, set the
`default-filter` configuration.

For example, some tests might need access to special resources not available to developer
workstations. To not run tests in the `special-tests` crate by default, but to run them with the
`ci` profile:

```toml title="Default filter configuration in <code>.config/nextest.toml</code>"
[profile.default]
default-filter = 'not package(special-tests)'

[profile.ci]
default-filter = 'all()'
```

The default filter is available in the filterset DSL via the `default()` predicate.

!!! info "Overriding the default filter"

    By default, command-line arguments are always interpreted with respect to the default filter. For example, `cargo nextest -E 'all()'` will run all tests that match the default filter.

    To override the default filter on the command line, use `--ignore-default-filter`. For example, `cargo nextest -E 'all()' --ignore-default-filter` will run all tests, including those not in the default filter.

Because skipping some tests can be surprising, nextest prints the number of tests and binaries
skipped due to their presence in the default filter. For example:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/default-filter-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/default-filter-output.ansi | ../scripts/strip-ansi.sh
    ```

!!! tip "Default filter vs ignored tests"

    The default filter and `#[ignore]` can both be used to filter out some tests by default. However, there are key distinctions between the two:

    1. The default filter is defined in nextest's configuration while ignored tests are annotated within Rust code.
    2. Default filters can be separately configured per-profile. Ignored tests are global to the repository.
    3. Default filters are a nextest feature, while ignored tests also work with `cargo test`.

    In practice, `#[ignore]` is often used for failing tests, while the default filter is typically used to filter out tests that are very slow or require specific resources.

### `--skip` and `--exact`

<!-- md:version 0.9.81 -->

Nextest accepts the `--skip` and `--exact` arguments after `--`, emulating the corresponding
arguments accepted by `cargo test`. The `--skip` and `--exact` arguments apply to test name filters
passed in after `--`.

For example, to run all tests matching the substring `test3`, but not including `skip1` or `skip2`:

```
cargo nextest run -- --skip skip1 --skip skip2 test3
```

To run all tests matching exactly the names `test1` and `test2`:

```
cargo nextest run -- test1 test2 --exact
```

To run all tests except those matching exactly `slow_module::my_test`:

```
cargo nextest run -- --exact --skip slow_module::my_test
```

Alternatively, and in prior versions of nextest, use a [filterset](filtersets/index.md). Some examples:

|                `cargo test` command                 |                Nextest filterset command                |
| :-------------------------------------------------: | :-----------------------------------------------------: |
|   `cargo test -- --skip skip1 --skip skip2 test3`   | `cargo nextest run -E 'test(test3) - test(/skip[12]/)'` |
|         `cargo test -- test1 test2 --exact`         |  `cargo nextest run -E 'test(=test1) + test(=test2)'`   |
| `cargo test -- --exact --skip slow_module::my_test` | `cargo nextest run -E 'not test(=slow_module::my_test)` |

### Filtering by build platform

While cross-compiling code, some tests (e.g. proc-macro tests) may need to be run on the host platform. To filter tests based on the build platform they're for, nextest's filtersets accept the `platform()` set with values `target` and `host`.

For example, to only run tests for the host platform:

```

cargo nextest run -E 'platform(host)'

```

[filterset DSL]: filtersets/index.md

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. For now, run doctests in a separate step with `cargo test --doc`.

## Other runner options

`--no-fail-fast`
: Do not exit the test run on the first failure. Most useful for CI scenarios.

`-j`, `--test-threads`
: Number of tests to run simultaneously. Note that this is separate from the number of build jobs to run simultaneously, which is specified by `--build-jobs`.

`--run-ignored only` <!-- md:version 0.9.76 -->
: Run only ignored tests. (With prior nextest versions, use `--run-ignored ignored-only`.)

`--run-ignored all`
: Run both ignored and non-ignored tests.

## Controlling nextest's output

For information about configuring the way nextest displays its human-readable output, see [_Reporting test results_](reporting.md).

## Options and arguments

=== "Summarized output"

    The output of `cargo nextest run -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest run -h
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest run -h
        ```

=== "Full output"

    The output of `cargo nextest run --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest run --help
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest run --help
        ```
