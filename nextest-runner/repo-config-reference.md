For more information about how repository configuration works, see the [main configuration page](index.md).

## Configuration file locations

Configuration is loaded in the following order (higher priority overrides lower):

1. Repository config (`.config/nextest.toml`)
2. Tool-specific configs (specified via `--tool-config-file`)
3. Default embedded config

Tool-specific configs allow tools to provide their own nextest configuration that integrates with repository settings.

For more information about configuration hierarchy, see [_Hierarchical configuration_](index.md#hierarchical-configuration).

## Repository configuration schema

<!-- md:version 0.9.134 -->

A JSON Schema is available for nextest's repository configuration. The schema can be obtained through:

- For the latest released version of nextest, at [`https://nexte.st/schemas/repo-config.json`](https://nexte.st/schemas/repo-config.json). This URL is updated with new nextest releases.
- For the schema corresponding to a particular version of nextest, by running `cargo nextest self schema repo-config`.

The schema can be used:

- To validate a repository configuration file.
- With the [Tombi](https://tombi-toml.github.io/tombi) language server for TOML or [RustRover](../integrations/rustrover.md), to provide autocomplete in supported IDEs and editors. (The taplo language server is not supported due to a [crash bug](https://github.com/tamasfe/taplo/pull/779).)

Note that the schema is somewhat stricter than nextest's own config parser: unknown configuration items will fail schema validation, while nextest itself will only print out a warning.

The schema is part of the [JSON Schema Store](https://www.schemastore.org/), so both Tombi and RustRover will automatically download it for you. Because nextest's configuration (outside of experimental configuration) is [append-only](../stability/index.md), the schema will automatically be updated as new nextest versions are released.

## Top-level configuration

These parameters are specified at the root level of the configuration file.

### `nextest-version`

<!-- md:version 0.9.55 -->

- **Type**: String or object
- **Description**: The minimum required (and optionally recommended) version of nextest for this configuration.
- **Documentation**: [_Minimum nextest versions_](minimum-versions.md)
- **Default**: Unset (the minimum version check is disabled)
- **Examples**:
  ```toml
  nextest-version = "0.9.50"
  # or
  nextest-version = { required = "0.9.20", recommended = "0.9.30" }
  ```

### `experimental`

- **Type**: Array of strings
- **Description**: Enables experimental, non-stable features.
- **Documentation**: [_Setup scripts_](setup-scripts.md), [_wrapper scripts_](wrapper-scripts.md)
- **Default**: `[]` (no experimental features enabled)
- **Valid values**:
  - `"setup-scripts"` <!-- md:version 0.9.98 --> (originally <!-- md:version 0.9.59 -->)
  - `"wrapper-scripts"` <!-- md:version 0.9.98 -->
- **Example**:
  ```toml
  experimental = ["setup-scripts"]
  ```

### `store`

- **Type**: Object
- **Description**: Configuration for the nextest store directory.

#### `store.dir`

- **Type**: String (path)
- **Description**: Directory where nextest stores its data.
- **Default**: `target/nextest`

## Profile configuration

Profiles are configured under `[profile.<name>]`. The default profile is called `[profile.default]`.

### General configuration

#### `profile.<name>.inherits`

<!-- md:version 0.9.115 -->

- **Type**: String
- **Description**: The profile to inherit settings from.
- **Documentation**: [_Profile inheritance_](index.md#profile-inheritance)
- **Default**: `"default"`

### Core test execution

#### `profile.<name>.default-filter`

- **Type**: String (filterset expression)
- **Description**: The default set of tests run by `cargo nextest run`, as a filterset expression.
- **Documentation**: [_Running a subset of tests by default_](../selecting.md#running-a-subset-of-tests-by-default)
- **Default**: `"all()"` (all tests are run)
- **Example**: `default-filter = "not test(very_slow_tests)"`

#### `profile.<name>.global-timeout`

<!-- md:version 0.9.100 -->

- **Type**: String (duration)
- **Description**: A global timeout for the entire test run.
- **Documentation**: [_Setting a global timeout_](../features/slow-tests.md#setting-a-global-timeout)
- **Default**: Unset (no global timeout)
- **Example**: `global-timeout = "2h"`

#### `profile.<name>.test-threads`

- **Type**: Integer or string
- **Description**: Number of threads to run tests with.
- **Valid values**: Positive integer, negative integer (relative to CPU count), or `"num-cpus"`
- **Default**: `"num-cpus"`
- **Example**: `test-threads = 4` or `test-threads = "num-cpus"`

#### `profile.<name>.threads-required`

- **Type**: Integer or string
- **Description**: Number of threads each test reserves from the pool.
- **Documentation**: [_Heavy tests and `threads-required`_](threads-required.md)
- **Valid values**: Positive integer, `"num-cpus"`, or `"num-test-threads"`
- **Default**: `1`

#### `profile.<name>.run-extra-args`

<!-- md:version 0.9.86 -->

- **Type**: Array of strings
- **Description**: Extra arguments to pass to test binaries.
- **Documentation**: [_Extra arguments_](extra-args.md)
- **Default**: `[]`
- **Example**: `run-extra-args = ["--test-threads", "1"]`

### Retry configuration

#### `profile.<name>.retries`

- **Type**: Integer or object
- **Description**: Retry policy for failed tests.
- **Documentation**: [_Retries and flaky tests_](../features/retries.md)
- **Default**: `0`
- **Examples**:
  ```toml
  retries = 3
  # or
  retries = { backoff = "fixed", count = 3, delay = "1s" }
  # or
  retries = { backoff = "exponential", count = 4, delay = "2s", max-delay = "10s", jitter = true }
  ```

#### `profile.<name>.flaky-result`

<!-- md:version 0.9.131 -->

- **Type**: String
- **Description**: Whether to treat flaky tests as passing or failing.
- **Documentation**: [_Failing flaky tests_](../features/retries.md#failing-flaky-tests)
- **Valid values**: `"pass"`, `"fail"`
- **Default**: `"pass"`
- **Example**: `flaky-result = "fail"`

### Timeout configuration

#### `profile.<name>.slow-timeout`

- **Type**: String (duration) or object
- **Description**: Time after which tests are considered slow, plus optional termination policy.
- **Documentation**: [_Slow tests and timeouts_](../features/slow-tests.md)
- **Default**: `60s` with no termination on timeout
- **Examples**:
  ```toml
  slow-timeout = "60s"
  # or
  slow-timeout = { period = "120s", terminate-after = 2, grace-period = "10s" }
  # or
  slow-timeout = { period = "30s", terminate-after = 4, on-timeout = "pass" }
  ```

The `slow-timeout` object accepts the following parameters:

- `period`: Time period after which a test is considered slow (required)
- `terminate-after`: Number of periods after which to terminate the test (default: do not terminate)
- `grace-period`: Time to wait for graceful shutdown before force termination (default: 10s)
- `on-timeout`: <!-- md:version 0.9.115 --> What to do when a test times out: `"fail"` (default) or `"pass"`

#### `profile.<name>.leak-timeout`

- **Type**: String (duration) or object
- **Description**: Time to wait for child processes to exit after a test completes.
- **Documentation**: [_Leaky tests_](../features/leaky-tests.md)
- **Examples**:
  ```toml
  leak-timeout = "100ms"
  # or
  leak-timeout = { period = "500ms", result = "fail" }
  ```

### Benchmark configuration

#### `profile.<name>.bench.global-timeout`

<!-- md:version 0.9.117 -->

- **Type**: String (duration)
- **Description**: Global timeout for the entire benchmark run. Replaces `global-timeout` when running `cargo nextest bench`.
- **Documentation**: [_Running benchmarks_](../features/benchmarks.md)
- **Default**: Unset (no global timeout)
- **Example**: `bench.global-timeout = "2h"`

#### `profile.<name>.bench.slow-timeout`

<!-- md:version 0.9.117 -->

- **Type**: String (duration) or object
- **Description**: Time after which benchmarks are considered slow, plus optional termination policy. Replaces `slow-timeout` when running `cargo nextest bench`.
- **Documentation**: [_Running benchmarks_](../features/benchmarks.md)
- **Default**: Unset
- **Examples**:
  ```toml
  bench.slow-timeout = "120s"
  # or
  bench.slow-timeout = { period = "60s", terminate-after = 10, grace-period = "10s" }
  ```

The `bench.slow-timeout` object accepts the same parameters as `slow-timeout`:

- `period`: Time period after which a benchmark is considered slow (required)
- `terminate-after`: Number of periods after which to terminate the benchmark (default: do not terminate)
- `grace-period`: Time to wait for graceful shutdown before force termination (default: 10s)

### Reporter options

#### `profile.<name>.status-level`

- **Type**: String
- **Description**: Level of status information to display during test runs.
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values** (incremental; each level includes those above it):
  - `"none"`: no output.
  - `"fail"`: only output test failures.
  - `"retry"`: output test retries; includes `fail`.
  - `"slow"`: output slow tests; includes `retry`.
  - `"leak"`: output leaky tests; includes `slow`.
  - `"pass"`: output passing tests; includes `leak`.
  - `"skip"`: output skipped tests; includes `pass`.
  - `"all"`: equivalent to `"skip"` today; reserved for future expansion.
- **Default**: `"pass"`

#### `profile.<name>.final-status-level`

- **Type**: String
- **Description**: Level of status information to display in the final summary.
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values** (incremental; each level includes those above it):
  - `"none"`: no output.
  - `"fail"`: only output test failures.
  - `"flaky"`: output flaky tests; includes `fail`. Accepts `"retry"` as an alias.
  - `"slow"`: output slow tests; includes `flaky`.
  - `"skip"`: output skipped tests; includes `slow`.
  - `"leak"`: output leaky tests; includes `skip`.
  - `"pass"`: output passing tests; includes `leak`.
  - `"all"`: equivalent to `"pass"` today; reserved for future expansion.
- **Default**: `"flaky"`

#### `profile.<name>.failure-output`

- **Type**: String
- **Description**: When to display output for failed tests.
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**:
  - `"immediate"`: show captured output as soon as the test completes.
  - `"immediate-final"`: show captured output when the test completes _and_ again at the end of the run.
  - `"final"`: show captured output only at the end of the run.
  - `"never"`: never show captured output.
- **Default**: `"immediate"`

#### `profile.<name>.success-output`

- **Type**: String
- **Description**: When to display output for successful tests.
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**: same as [`failure-output`](#profilenamefailure-output)
- **Default**: `"never"`

### Failure handling

#### `profile.<name>.fail-fast`

- **Type**: Boolean or object
- **Description**: Controls when to stop running tests after failures.
- **Documentation**: [_Failing fast_](../running.md#failing-fast)
- **Default**: `true` (stop after first failure, wait for running tests to complete)
- **Examples**:
  ```toml
  fail-fast = true  # Stop after first failure (waits for running tests)
  fail-fast = false # Run all tests
  fail-fast = { max-fail = 5 }  # Stop after 5 failures (waits for running tests)
  fail-fast = { max-fail = "all" }  # Run all tests

  # With termination mode (since 0.9.111)
  fail-fast = { max-fail = 1, terminate = "wait" }  # Wait for running tests (default)
  fail-fast = { max-fail = 1, terminate = "immediate" }  # Terminate running tests immediately
  ```

When `max-fail` is exceeded:
- **`terminate = "wait"`** (default): nextest stops scheduling new tests but waits for currently running tests to finish naturally.
- **`terminate = "immediate"`** <!-- md:version 0.9.111 -->: nextest sends termination signals to running tests (respecting the grace period configured via `slow-timeout.terminate-after`).

### Test grouping

#### `profile.<name>.test-group`

<!-- md:version 0.9.48 -->

- **Type**: String
- **Description**: Assigns matching tests to a custom test group for resource management.
- **Documentation**: [_Test groups for mutual exclusion_](test-groups.md)
- **Valid values**: Custom group name or `"@global"`
- **Default**: `"@global"`

### JUnit configuration

#### `profile.<name>.junit.path`

- **Type**: String (path)
- **Description**: Path to write the JUnit XML report to. If unset, JUnit reporting is disabled.
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: Unset
- **Example**: `junit.path = "target/nextest/junit.xml"`

#### `profile.<name>.junit.report-name`

- **Type**: String
- **Description**: Name for the JUnit XML report.
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `"nextest-run"`

#### `profile.<name>.junit.store-success-output`

- **Type**: Boolean
- **Description**: Whether to store successful test output in the JUnit XML report.
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `false`

#### `profile.<name>.junit.store-failure-output`

- **Type**: Boolean
- **Description**: Whether to store failed test output in the JUnit XML report.
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `true`

#### `profile.<name>.junit.flaky-fail-status`

<!-- md:version 0.9.131 -->

- **Type**: String
- **Description**: How flaky-fail tests are reported in the JUnit XML report.
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Valid values**: `"failure"` or `"success"`
- **Default**: `"failure"`

### Archive configuration

<!-- md:version 0.9.70 -->

#### `profile.<name>.archive.include`

- **Type**: Array of objects
- **Description**: Extra paths to include in the archive.
- **Documentation**: [_Archiving and reusing builds_](../ci-features/archiving.md)
- **Example**:
  ```toml
  [[profile.default.archive.include]]
  path = "fixtures"
  relative-to = "target"
  depth = 2
  on-missing = "warn"
  ```

##### Archive include parameters

- `path`: Path to include, relative to `relative-to`.
- `relative-to`: Base directory `path` is interpreted relative to. Valid values: `"target"`.
- `depth`: Maximum recursion depth (non-negative integer or `"infinite"`).
- `on-missing`: What to do if `path` is missing. Valid values: `"ignore"`, `"warn"`, `"error"`.

## Override configuration

Overrides allow you to customize settings for specific tests or platforms using `[[profile.<name>.overrides]]` sections.

For detailed information, see [_Per-test settings_](per-test-overrides.md).

### Override filters

At least one of these filters must be specified.

#### `filter`

- **Type**: String (filterset expression)
- **Description**: Filterset expression selecting tests this override applies to.
- **Documentation**: [_Filterset DSL_](../filtersets/index.md)
- **Default**: Override applies to all tests
- **Example**: `filter = 'test(integration_test)'`

#### `platform`

<!-- md:version 0.9.58 -->

- **Type**: String or Object
- **Description**: Host and/or target platforms this override applies to.
- **Documentation**: [_Specifying platforms_](specifying-platforms.md)
- **Default**: Override applies to all platforms
- **Examples**:
  ```toml
  platform = "x86_64-unknown-linux-gnu"
  # or
  platform = { host = "cfg(unix)", target = "aarch64-apple-darwin" }
  ```

### Override settings

#### `default-filter`

<!-- md:version 0.9.84 -->

- **Type**: String (filterset expression)
- **Description**: Replaces `default-filter` for matching platforms. Requires `platform` and must not be combined with `filter`.
- **Documentation**: [_Running a subset of tests by default_](../selecting.md#running-a-subset-of-tests-by-default)

#### `priority`

<!-- md:version 0.9.91 -->

- **Type**: Integer (-100 to 100)
- **Description**: Priority for matching tests; higher values run sooner.
- **Documentation**: [_Test priorities_](test-priorities.md)
- **Default**: `0`

#### Other settings

All profile-level settings can be overridden:

- `threads-required`
- `run-extra-args`
- `retries`
- `flaky-result`
- `slow-timeout`
- `bench.slow-timeout`
- `leak-timeout`
- `test-group`
- `success-output`
- `failure-output`
- `junit.store-success-output`
- `junit.store-failure-output`

## Test group configuration

<!-- md:version 0.9.48 -->

Custom test groups are defined under `[test-groups.<name>]` sections.

For detailed information, see [_Test groups for mutual exclusion_](test-groups.md).

#### `test-groups.<name>.max-threads`

- **Type**: Integer or string
- **Description**: Maximum number of threads this test group may use concurrently.
- **Valid values**: Positive integer or `"num-cpus"`

## Script configuration

<!-- md:version 0.9.59 -->

Scripts are configured under `[scripts.setup.<name>]` and `[scripts.wrapper.<name>]` sections.

### Setup scripts

<!-- md:version 0.9.98 -->

For detailed information, see [_Setup scripts_](setup-scripts.md).

#### `scripts.setup.<name>.command`

- **Type**: String, array, or object
- **Description**: The command to run for this setup script.
- **Examples**:
  ```toml
  command = "echo hello"
  # or
  command = ["cargo", "run", "--bin", "setup"]
  # or
  command = { command-line = "debug/my-setup", relative-to = "target" }
  ```

#### `scripts.setup.<name>.slow-timeout`

- **Type**: String (duration) or object
- **Description**: Slow-timeout configuration for this setup script.
- **Default**: No timeout
- **Examples**:
  ```toml
  slow-timeout = "30s"
  # or
  slow-timeout = { period = "60s", terminate-after = 1, grace-period = "5s" }
  ```

#### `scripts.setup.<name>.leak-timeout`

- **Type**: String (duration) or object
- **Description**: Leak-timeout configuration for this setup script.
- **Default**: `200ms`
- **Examples**:
  ```toml
  leak-timeout = "500ms"
  # or
  leak-timeout = { period = "1s", result = "fail" }
  ```

#### `scripts.setup.<name>.capture-stdout`

- **Type**: Boolean
- **Description**: Whether to capture stdout from this setup script.
- **Default**: `false`

#### `scripts.setup.<name>.capture-stderr`

- **Type**: Boolean
- **Description**: Whether to capture stderr from this setup script.
- **Default**: `false`

#### `scripts.setup.<name>.junit.store-success-output`

- **Type**: Boolean
- **Description**: Whether to store this setup script's output on success in the JUnit XML report.
- **Default**: `true`

#### `scripts.setup.<name>.junit.store-failure-output`

- **Type**: Boolean
- **Description**: Whether to store this setup script's output on failure in the JUnit XML report.
- **Default**: `true`

### Wrapper scripts

<!-- md:version 0.9.98 -->

For detailed information, see [_Wrapper scripts_](wrapper-scripts.md).

#### `scripts.wrapper.<name>.command`

- **Type**: String, array, or object
- **Description**: The command to run as the wrapper.
- **Examples**:
  ```toml
  command = "my-script.sh"
  # or
  command = ["cargo", "run", "--bin", "setup", "--"]
  # or
  command = { command-line = "debug/my-setup", relative-to = "target" }
  ```

#### `scripts.wrapper.<name>.target-runner`

- **Type**: String
- **Description**: How this wrapper composes with a configured target runner.
- **Documentation**: [_Target runners_](../features/target-runners.md)
- **Valid values**:
  - `"ignore"`: the target runner is ignored.
  - `"overrides-wrapper"`: when a target runner is configured, it replaces the wrapper; otherwise the wrapper runs as usual.
  - `"within-wrapper"`: the target runner runs within the wrapper script (command line: `<wrapper> <target-runner> <test-binary> <args>`).
  - `"around-wrapper"`: the target runner runs around the wrapper script (command line: `<target-runner> <wrapper> <test-binary> <args>`).
- **Default**: `"ignore"`

## Profile script configuration

<!-- md:version 0.9.59 -->

Profile-specific script configuration under `[[profile.<name>.scripts]]` sections.

### Profile script filters

At least one of these filters must be specified.

#### `platform`

- **Type**: String or object
- **Description**: Host and/or target platforms these scripts apply to.
- **Documentation**: [_Specifying platforms_](specifying-platforms.md)
- **Default**: Scripts apply to all platforms
- **Examples**:
  ```toml
  platform = "x86_64-unknown-linux-gnu"
  # or
  platform = { host = "cfg(unix)", target = "aarch64-apple-darwin" }
  ```

#### `filter`

- **Type**: String (filterset expression)
- **Description**: Filterset expression selecting tests these scripts apply to.
- **Documentation**: [_Filterset DSL_](../filtersets/index.md)
- **Default**: Scripts apply to all tests
- **Example**: `filter = 'test(integration_test)'`

### Profile script instructions

At least one instruction must be specified.

#### `setup`

- **Type**: String or array of strings
- **Description**: Names of setup scripts to run (single name or array).
- **Documentation**: [_Setup scripts_](setup-scripts.md)
- **Examples**:
  ```toml
  setup = "my-setup"
  # or
  setup = ["setup1", "setup2"]
  ```

#### `list-wrapper`

- **Type**: String
- **Description**: Name of the wrapper script used during test listing.
- **Documentation**: [_Wrapper scripts_](wrapper-scripts.md)
- **Example**: `list-wrapper = "my-list-wrapper"`

#### `run-wrapper`

- **Type**: String
- **Description**: Name of the wrapper script used during test execution.
- **Documentation**: [_Wrapper scripts_](wrapper-scripts.md)
- **Example**: `run-wrapper = "my-run-wrapper"`
