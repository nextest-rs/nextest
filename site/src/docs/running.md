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

<!-- md:version 0.9.75 -->

By default, all discovered tests are run. To only run some tests by default, set the `default-set`
configuration.

For example, some tests might need access to special resources not available to developer
workstations. To not run tests in the `special-tests` crate by default, but to run them with the
`ci` profile:

```toml
[profile.default]
default-set = 'not package(special-tests)'

[profile.ci]
default-set = 'all()'
```

The default set is available in the filterset DSL via the `default()` predicate.

!!! info "Filtersets override the default set"

    Specifying any filtersets on the command line overrides the default set. To consider the default set of tests, use `default()`.

    - For example, `cargo nextest run -E 'test(my_test)'` will run all tests that contain `my_test` in the name, even if they're not in the default set.
    - To only include tests in the default set, use `cargo nextest run -E 'default() & test(my_test)'`.
    - To run all tests, overriding the default set, use `cargo nextest run -E 'all()'`.

    Specifying non-filterset arguments does not override the default set.

Because skipping some tests can be surprising, nextest prints the number of tests and binaries
skipped due to their presence in the default set. For example:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/default-set-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/default-set-output.ansi | ../scripts/strip-ansi.sh
    ```

### `--skip` and `--exact`

Nextest does not support `--skip` and `--exact` directly; instead, use a filterset which supersedes these options.

Here are some examples:

|               Cargo test command                |                     Nextest command                     |
| :---------------------------------------------: | :-----------------------------------------------------: |
| `cargo test -- --skip skip1 --skip skip2 test3` | `cargo nextest run -E 'test(test3) - test(/skip[12]/)'` |
|       `cargo test -- --exact test1 test2`       |  `cargo nextest run -E 'test(=test1) + test(=test2)'`   |

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

`--run-ignored ignored-only`
: Run only ignored tests.

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
