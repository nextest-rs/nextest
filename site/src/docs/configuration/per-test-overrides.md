---
icon: material/chevron-double-right
description: Configuring settings for subsets of tests using filterset and Cargo platform specifiers.
---

# Per-test settings

Nextest supports overriding some settings for subsets of tests, using the [filterset](../filtersets/index.md) and [Rust `cfg()`](specifying-platforms.md) syntaxes.

Overrides are set via the `[[profile.<name>.overrides]]` list.

## Selecting tests

At least one of these fields must be specified:

`filter`
: [Filterset](../filtersets/index.md) expression selecting tests this override applies to.

`platform`
: Host and/or target platforms this override applies to. Either a string,
or a map with `host` and `target` keys for cross-compiling. See
[*Specifying platforms*](specifying-platforms.md) for more information.

## Supported overrides

`priority` <!-- md:version 0.9.91 -->
: [Priority](test-priorities.md) for matching tests; higher values run sooner. A number from -100 to 100, inclusive. Default: 0.

`retries`
: [Retry policy](../features/retries.md) for matching tests.

`flaky-result` <!-- md:version 0.9.131 -->
: Whether to treat matching flaky tests as [passing or failing](../features/retries.md#failing-flaky-tests).

`threads-required`
: Number of [threads](threads-required.md) each matching test reserves from the pool.

`test-group`
: Assigns matching tests to a [test group](test-groups.md).

`slow-timeout`
: Time after which matching tests are considered [slow](../features/slow-tests.md), plus optional termination policy.

`bench.slow-timeout` <!-- md:version 0.9.117 -->
: Time after which matching benchmarks are considered [slow](../features/benchmarks.md), plus optional termination policy. Replaces `slow-timeout` when running `cargo nextest bench`.

`leak-timeout`
: Time to wait for [child processes to exit](../features/leaky-tests.md) after a matching test completes.

`success-output` and `failure-output`
: [When to display output](../reporting.md#displaying-captured-test-output) for matching successful and failed tests, respectively.

`junit.store-success-output` and `junit.store-failure-output`
: Whether to store successful and failed output, respectively, for matching tests in the [JUnit XML report](../machine-readable/junit.md).

`junit.report-skipped` <!-- md:version 0.9.141 -->
: Which skipped tests to emit as `<skipped>` testcases for matching tests in the [JUnit XML report](../machine-readable/junit.md): `"none"` (the default), `"ignored"`, or `"all"`.

`junit.flaky-fail-status` <!-- md:version 0.9.131 -->
: How matching [flaky-fail](../features/retries.md#failing-flaky-tests) tests are reported in the [JUnit XML report](../machine-readable/junit.md): as `"failure"` (default) or `"success"`.

`default-filter` <!-- md:version 0.9.84 -->
: Replaces [`default-filter`](../selecting.md#running-a-subset-of-tests-by-default) for matching platforms. Requires `platform` and must not be combined with `filter`.

`run-extra-args` <!-- md:version 0.9.86 -->
: [Extra arguments](extra-args.md) to pass to matching test binaries.

## Example

```toml title="Basic example for per-test settings in <code>.config/nextest.toml</code>"
[profile.ci]
retries = 1

[[profile.ci.overrides]]
filter = 'test(/\btest_network_/)'
retries = 4

[[profile.ci.overrides]]
platform = 'x86_64-unknown-linux-gnu'
slow-timeout = "5m"

[[profile.ci.overrides]]
filter = 'test(/\btest_filesystem_/)'
platform = { host = 'cfg(target_os = "macos")' }
leak-timeout = "500ms"
success-output = "immediate"
```

When `--profile ci` is specified:

- for test names that start with `test_network_` (including test names like `my_module::test_network_`), retry tests up to 4 times
- on `x86_64-unknown-linux-gnu`, set a slow timeout of 5 minutes
- on macOS hosts, for test names that start with `test_filesystem_` (including test names like `my_module::test_filesystem_`), set a leak timeout of 500 milliseconds, and show success output immediately.

## Override precedence

Overrides are configured as an ordered list, and are applied in the following order. For a given test _T_ and a given setting _S_, overrides are applied in the following order:

1. Command-line arguments and environment variables for _S_, if specified, take precedence over all overrides. See [*Hierarchical configuration*](index.md#hierarchical-configuration) for more.
2. If nextest is run with `--profile my-profile`, the first override within `profile.my-profile.overrides` that matches _T_ and configures _S_ is applied.
3. Otherwise, the first override within `profile.default.overrides` that matches _T_ and configures _S_ is applied.
4. Otherwise, if nextest is run with `--profile my-profile`, the global configuration for that profile is applied, if it configures _S_.
5. If none of the above conditions apply, the global configuration specified by `profile.default` is applied.

Precedence is evaluated separately for each override. If a particular override does not configure a setting, it is ignored for that setting.

### Example

```toml title="Example for per-test settings in <code>.config/nextest.toml</code>"
[profile.default]
retries = 0  # this is the default, so it doesn't need to be specified
slow-timeout = "30s"

[[profile.default.overrides]]
filter = 'package(my-package)'
retries = 2
slow-timeout = "45s"

[profile.ci]
retries = 1
slow-timeout = { period = "15s", terminate-after = 2 }

[[profile.ci.overrides]]
filter = 'package(my-package) and test(/^flaky::/)'
retries = 3
```

If nextest is run with `--retries 5`, all tests are retried 5 times. The slow timeout settings are evaluated as listed below.

Otherwise, if nextest is run with `--profile ci`:

- Tests in `my-package` that begin with `flaky::` are retried 3 times, and are run with a slow timeout of 45 seconds.
- Other tests in `my-package` are retried 2 times and are run with a slow timeout of 45 seconds.
- All other tests are retried up to one time and are run with a slow-timeout of 15 seconds. Tests that take longer than 30 seconds are terminated.

If nextest is run without `--profile`:

- Tests in `my-package` are retried 2 times and with a slow timeout of 45 seconds.
- Other tests are retried 0 times with a slow timeout of 30 seconds.
