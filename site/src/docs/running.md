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
- Tests that take more than a specified amount of time (60 seconds by default) are marked **SLOW**. See [*Slow tests and timeouts*](features/slow-tests.md).
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

#### Per-platform default filters

<!-- md:version 0.9.84 -->

Default filters can be set per-platform via [the `overrides` section](configuration/per-test-overrides.md).

```toml title="Per-platform default filter configuration"
[[profile.default.overrides]]
platform = 'cfg(windows)'
default-filter = 'not test()'
```

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

## Rerunning only failed tests

<!-- md:version 0.9.XX -->

Nextest can rerun only tests that failed in the previous run, similar to pytest's `--last-failed` option. This is useful when working on fixing a set of failing tests.

To only run tests that failed in the last run:

```
cargo nextest run --last-failed
# or use the short alias:
cargo nextest run --lf
```

This will:
- Run only the tests that failed in the previous test run for the current profile
- Show a message if no failed tests were found
- Can be combined with other filters (tests must match both the failed set and the filter)

### Running failed tests first

To run all tests, but prioritize failed tests to run first:

```
cargo nextest run --failed-last
# or use the short alias:
cargo nextest run --fl
```

This is useful to get quick feedback on whether previously failing tests are now fixed.

### Clearing failed test history

To clear the history of failed tests:

```
cargo nextest run --clear-failed
```

This removes the stored information about which tests failed, without running any tests.

!!! note "Profile-specific storage"

    Failed test history is stored per [profile](configuration/index.md#profiles). Tests that failed with one profile won't affect runs with a different profile.

## Failing fast

By default, nextest cancels the test run on encountering a single failure. Tests currently running are run to completion, but new tests are not started.

This behavior can be customized through either the command-line, or through [configuration](configuration/index.md).

Through the command line:

`--max-fail=N` <!-- md:version 0.9.86 -->
: Number of tests that can fail before aborting the test run, or `all` to run all tests regardless of the number of failures. Useful for uncovering multiple issues without having to run the whole test suite.

`--no-fail-fast` (<!-- md:version 0.9.92 --> alias `--nff`)
: Do not exit the test run in case a test fails. Most useful for CI scenarios. Equivalent to `--max-fail=all`.

`--fail-fast` (<!-- md:version 0.9.92 --> alias `--ff`)
: Exit the test run on the first failure. This is the default behavior. Equivalent to `--max-fail=1`.

Through configuration:

```toml title="Fail-fast behavior in <code>.config/nextest.toml</code>"
[profile.default]
# Exit the test run after N failures, e.g. 5 failures. max-fail in configuration
# is available starting cargo-nextest 0.9.89+.
fail-fast = { max-fail = 5 }

# Exit the test run on the first failure. This is the default behavior.
fail-fast = true
# Or:
fail-fast = { max-fail = 1 }

# Do not exit the test run until all tests complete.
fail-fast = false
# Or:
fail-fast = { max-fail = "all" }
```

## Other runner options

`-jN`, `--test-threads=N`
: Number of tests to run simultaneously. Note that this is separate from the number of build jobs to run simultaneously, which is specified by `--build-jobs`.

  `N` can be:

  * `num-cpus` to run as many tests as the amount of [available parallelism] (typically the number of CPU hyperthreads). This is the default.
  * a positive integer (e.g. `8`) to run that many tests simultaneously.
  * a negative integer (e.g. `-2`) to run available parallelism minus that many tests simultaneously. For example, on a machine with 8 CPU hyperthreads, `-2` would run 6 tests simultaneously.

  Tests can be marked as taking up more than one available thread. For more, see [*Heavy tests and `threads-required`*](configuration/threads-required.md).

`--run-ignored=only` <!-- md:version 0.9.76 -->
: Run only ignored tests. (With prior nextest versions, use `--run-ignored=ignored-only`.)

`--run-ignored=all`
: Run both ignored and non-ignored tests.

`--no-tests=fail|warn|pass` <!-- md:version 0.9.75 -->
: Control behavior when no tests are run. In some cases, e.g. using nextest with [cargo-hack](https://github.com/taiki-e/cargo-hack) to test the powerset of features, it may be useful to set this to `warn` or `pass`.

    If set to `fail` and no tests are found, nextest exits with the advisory code 4 ([`NO_TESTS_RUN`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.NO_TESTS_RUN)).

    <!-- md:version 0.9.85 --> The default is `fail`. In prior versions, the default was `pass` or `warn`.

[available parallelism]: https://doc.rust-lang.org/std/thread/fn.available_parallelism.html

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
