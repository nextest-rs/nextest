---
icon: octicons/multi-select-24
description: Choosing subsets of tests to run with cargo-nextest.
---

# Selecting tests

By default, a [`cargo nextest run` invocation](running.md) runs all discovered, non-ignored tests. The `cargo nextest run` and `list` commands support a rich set of operators to _select_ or _filter_ which tests should be run.

## Basic usage

To only run tests that match certain names:

```sh
cargo nextest run <test-name1> <test-name2>...
```

Test names can also be passed in after `--`, similar to `cargo test`:

```sh
cargo nextest run -- <test-name1> <test-name2>...
```

To [list tests](listing.md) that would be run by `cargo nextest run`:

```sh
cargo nextest list <test-name1> <test-name2>...
```

## `--skip` and `--exact`

Nextest accepts the `--skip` and `--exact` options after `--`, emulating the corresponding arguments accepted by `cargo test`.

!!! note

    The `--skip` and `--exact` options only apply to test name filters passed in after `--`.

For example, to run all tests matching the substring `test3`, but not including `skip1` or `skip2`:

```sh
cargo nextest run -- --skip skip1 --skip skip2 test3
```

To run all tests matching exactly the names `test1` and `test2`:

```sh
cargo nextest run -- test1 test2 --exact
```

To run all tests except those matching exactly `slow_module::my_test`:

```sh
cargo nextest run -- --exact --skip slow_module::my_test
```

## Filtersets

For more complex selections, nextest includes a domain-specific language (DSL) called [filtersets](filtersets/index.md). This DSL allows for advanced filtering by test name, test binary, and much more, and includes regex and glob operators.

Filtersets are specified on the command line with `-E`, or `--filterset`.

For example, to run all tests in `my-crate` and its dependencies:

```sh
cargo nextest run -E 'deps(my-crate)'
```

For more information about filtersets, see [_Filterset DSL_](filtersets/index.md).

### `--skip` and `--exact` as filtersets

The `--skip` and `--exact` options can be translated to filtersets:

|                `cargo test` command                 |                Nextest filterset command                |
| :-------------------------------------------------: | :-----------------------------------------------------: |
|   `cargo test -- --skip skip1 --skip skip2 test3`   | `cargo nextest run -E 'test(test3) - test(/skip[12]/)'` |
|         `cargo test -- test1 test2 --exact`         |  `cargo nextest run -E 'test(=test1) + test(=test2)'`   |
| `cargo test -- --exact --skip slow_module::my_test` | `cargo nextest run -E 'not test(=slow_module::my_test)` |

### Filtering by build platform

While cross-compiling code, some tests (e.g. proc-macro tests) may need to be run on the host platform. To filter tests based on the build platform they're for, nextest's filtersets accept the `platform()` set with values `target` and `host`.

For example, to only run tests for the host platform:

```sh
cargo nextest run -E 'platform(host)'
```

## Running a subset of tests by default

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
    cat src/outputs/default-filter-output.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/default-filter-output.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

!!! tip "Default filter vs ignored tests"

    The default filter and `#[ignore]` can both be used to filter out some tests by default. However, there are key distinctions between the two:

    1. The default filter is defined in nextest's configuration while ignored tests are annotated within Rust code.
    2. Default filters can be separately configured per-profile. Ignored tests are global to the repository.
    3. Default filters are a nextest feature, while ignored tests also work with `cargo test`.

    In practice, `#[ignore]` is often used for failing tests, while the default filter is typically used to filter out tests that are very slow or require specific resources.

### Per-platform default filters

Default filters can be set per-platform via [the `overrides` section](configuration/per-test-overrides.md).

```toml title="Per-platform default filter configuration"
[[profile.default.overrides]]
platform = 'cfg(windows)'
default-filter = 'not test()'
```
