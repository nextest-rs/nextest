# Per-test overrides

Nextest supports overriding some settings for subsets of tests, using the [filter expression](filter-expressions.md) and Rust conditional compilation syntaxes.

Overrides are set via the `[[profile.<name>.overrides]]` list. Each override consists of the following:
* `filter` — The filter expression to match.
* `platform` — The Rust [target triple](https://doc.rust-lang.org/beta/rustc/platform-support.html#platform-support) or [`cfg()` expression](https://doc.rust-lang.org/reference/conditional-compilation.html) to match.
* Supported overrides, which are optional. Currently supported are:
  * `retries` — Number of retries to run tests with.
  * `threads-required` — Number of [threads required](threads-required.md) for this test.
  * `test-group` — An optional [test group](test-groups.md) for this test.
  * `slow-timeout` — Amount of time after which [tests are marked slow](slow-tests.md).
  * `leak-timeout` — How long to wait after the test completes [for any subprocesses to exit](leaky-tests.md).
  * `success-output` and `failure-output` — Control [when standard output and standard error are displayed](other-options.md#--success-output-and---failure-output) for passing and failing tests, respectively. Values supported are:
    * `immediate`: display output as soon as the test fails. Default for `failure-output`.
    * `final`: display output at the end of the test run.
    * `immediate-final`: display output as soon as the test fails, and at the end of the run.
    * `never`: never display output. Default for `success-output`.
  * `junit.store-success-output` and `junit.store-failure-output` — Whether to store output for passing and failing tests, respectively, in [JUnit reports](junit.md).

## Example

```toml
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
platform = 'cfg(target_os = "macos")'
leak-timeout = "500ms"
success-output = "immediate"
```

When `--profile ci` is specified:
* for test names that start with `test_network_` (including test names like `my_module::test_network_`), retry tests up to 4 times
* on `x86_64-unknown-linux-gnu`, set a slow timeout of 5 minutes
* on macOS, for test names that start with `test_filesystem_` (including test names like `my_module::test_filesystem_`), set a leak timeout of 500 milliseconds, and show success output immediately.

## Override precedence

Overrides are configured as an ordered list. They're are applied in the following order. For a given test *T* and a given setting *S*:
1. If nextest is run with `--profile my-profile`, the first override within `profile.my-profile.overrides` that matches *T* and configures *S*.
2. The first override within `profile.default.overrides` that matches *T* and configures *S*.
3. If nextest is run with `--profile my-profile`, the global configuration for that profile, if it configures *S*.
4. The global configuration specified by `profile.default`.

Precedence is evaluated separately for each override. If a particular override does not configure a setting, it is ignored for that setting.

### Example

```toml
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

If nextest is run with `--profile ci`:
* Tests in `my-package` that begin with `flaky::` are retried 3 times, and are run with a slow timeout of 45 seconds.
* Other tests in `my-package` are retried 2 times and are run with a slow timeout of 45 seconds.
* All other tests are retried up to one time and are run with a slow-timeout of 15 seconds. Tests that take longer than 30 seconds are terminated.

If nextest is run without `--profile`:
* Tests in `my-package` are retried 2 times and with a slow timeout of 45 seconds.
* Other tests are retried 0 times with a slow timeout of 30 seconds.
