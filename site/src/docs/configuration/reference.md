---
icon: material/book-open-variant
description: Reference documentation for all nextest configuration parameters and settings.
---

# Configuration reference

This page provides a comprehensive reference of all configuration parameters available in nextest.

For more information about how configuration works, see the [main configuration page](index.md).

## Configuration file locations

Configuration is loaded in the following order (higher priority overrides lower):

1. Repository config (`.config/nextest.toml`)
2. Tool-specific configs (specified via `--tool-config-file`)
3. Default embedded config

Tool-specific configs allow tools to provide their own nextest configuration that integrates with repository settings.

For more information about configuration hierarchy, see [_Hierarchical configuration_](index.md#hierarchical-configuration).

## Top-level configuration

These parameters are specified at the root level of the configuration file.

### `nextest-version`

- **Type**: String or object
- **Description**: Specifies the minimum required version of nextest
- **Documentation**: [_Minimum nextest versions_](minimum-versions.md)
- **Default**: Unset: the minimum version check is disabled
- **Examples**:
  ```toml
  nextest-version = "0.9.50"
  # or
  nextest-version = { required = "0.9.20", recommended = "0.9.30" }
  ```

### `experimental`

- **Type**: Array of strings
- **Description**: Enables experimental features
- **Documentation**: [_Setup scripts_](setup-scripts.md), [_wrapper scripts_](wrapper-scripts.md)
- **Default**: `[]`: no experimental features are enabled
- **Valid values**: `["setup-scripts", "wrapper-scripts"]`
- **Example**:
  ```toml
  experimental = ["setup-scripts"]
  ```

### `store`

- **Type**: Object
- **Description**: Configuration for the nextest store directory

#### `store.dir`

- **Type**: String (path)
- **Description**: Directory where nextest stores its data
- **Default**: `target/nextest`

## Profile configuration

Profiles are configured under `[profile.<name>]`. The default profile is called `[profile.default]`.

### Core test execution

#### `profile.<name>.default-filter`

- **Type**: String (filterset expression)
- **Description**: The default set of tests to run
- **Documentation**: [_Running a subset of tests by default_](../running.md#running-a-subset-of-tests-by-default)
- **Default**: `all()`: all tests are run
- **Example**: `default-filter = "not test(very_slow_tests)"`

#### `profile.<name>.test-threads`

- **Type**: Integer or string
- **Description**: Number of threads to run tests with
- **Valid values**: Positive integer, negative integer (relative to CPU count), or `"num-cpus"`
- **Default**: `"num-cpus"`
- **Example**: `test-threads = 4` or `test-threads = "num-cpus"`

#### `profile.<name>.threads-required`

- **Type**: Integer or string
- **Description**: Number of threads each test requires
- **Documentation**: [_Heavy tests and `threads-required`_](threads-required.md)
- **Valid values**: Positive integer, `"num-cpus"`, or `"num-test-threads"`
- **Default**: `1`

#### `profile.<name>.run-extra-args`

- **Type**: Array of strings
- **Description**: Extra arguments to pass to test binaries
- **Documentation**: [_Extra arguments_](extra-args.md)
- **Default**: `[]`: no extra arguments
- **Example**: `run-extra-args = ["--test-threads", "1"]`

### Retry configuration

#### `profile.<name>.retries`

- **Type**: Integer or object
- **Description**: Retry policy for failed tests
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

### Timeout configuration

#### `profile.<name>.slow-timeout`

- **Type**: String (duration) or object
- **Description**: Time after which tests are considered slow, and timeout configuration
- **Documentation**: [_Slow tests and timeouts_](../features/slow-tests.md)
- **Default**: `60s` with no termination on timeout
- **Examples**:
  ```toml
  slow-timeout = "60s"
  # or
  slow-timeout = { period = "120s", terminate-after = 2, grace-period = "10s" }
  ```

#### `profile.<name>.leak-timeout`

- **Type**: String (duration) or object
- **Description**: Time to wait for child processes to exit after a test completes
- **Documentation**: [_Leaky tests_](../features/leaky-tests.md)
- **Examples**:
  ```toml
  leak-timeout = "100ms"
  # or
  leak-timeout = { period = "500ms", result = "fail" }
  ```

### Reporter options

#### `profile.<name>.status-level`

- **Type**: String
- **Description**: Level of status information to display during test runs
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**: `"none"`, `"fail"`, `"retry"`, `"slow"`, `"leak"`, `"pass"`, `"skip"`, `"all"`
- **Default**: `"pass"`

#### `profile.<name>.final-status-level`

- **Type**: String
- **Description**: Level of status information to display in final summary
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**: `"none"`, `"fail"`, `"flaky"`, `"slow"`, `"skip"`, `"leak"`, `"pass"`, `"all"`
- **Default**: `"fail"`

#### `profile.<name>.failure-output`

- **Type**: String
- **Description**: When to display output for failed tests
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**: `"immediate"`, `"immediate-final"`, `"final"`, `"never"`
- **Default**: `"immediate"`

#### `profile.<name>.success-output`

- **Type**: String
- **Description**: When to display output for successful tests
- **Documentation**: [_Reporting test results_](../reporting.md)
- **Valid values**: `"immediate"`, `"immediate-final"`, `"final"`, `"never"`
- **Default**: `"never"`

### Failure handling

#### `profile.<name>.fail-fast`

- **Type**: Boolean or object
- **Description**: Controls when to stop running tests after failures
- **Documentation**: [_Failing fast_](../running.md#failing-fast)
- **Examples**:
  ```toml
  fail-fast = true  # Stop after first failure
  fail-fast = false # Run all tests
  fail-fast = { max-fail = 5 }  # Stop after 5 failures
  fail-fast = { max-fail = "all" }  # Run all tests
  ```

### Test grouping

#### `profile.<name>.test-group`

- **Type**: String
- **Description**: Assigns tests to a custom group for resource management
- **Documentation**: [_Test groups for mutual exclusion_](test-groups.md)
- **Valid values**: Custom group name or `"@global"`
- **Default**: `"@global"`

### JUnit configuration

#### `profile.<name>.junit.path`

- **Type**: String (path)
- **Description**: Path to write JUnit XML report
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: JUnit support is disabled
- **Example**: `junit.path = "target/nextest/junit.xml"`

#### `profile.<name>.junit.report-name`

- **Type**: String
- **Description**: Name for the JUnit report
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `"nextest-run"`

#### `profile.<name>.junit.store-success-output`

- **Type**: Boolean
- **Description**: Whether to store successful test output in JUnit XML
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `false`

#### `profile.<name>.junit.store-failure-output`

- **Type**: Boolean
- **Description**: Whether to store failed test output in JUnit XML
- **Documentation**: [_JUnit support_](../machine-readable/junit.md)
- **Default**: `true`

### Archive configuration

#### `profile.<name>.archive.include`

- **Type**: Array of objects
- **Description**: Files to include when creating test archives
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

- `path`: Relative path to include
- `relative-to`: Base directory (`"target"`)
- `depth`: Maximum recursion depth (integer or `"infinite"`)
- `on-missing`: What to do if path is missing (`"ignore"`, `"warn"`, `"error"`)

## Override configuration

Overrides allow you to customize settings for specific tests or platforms using `[[profile.<name>.overrides]]` sections.

For detailed information, see [_Per-test settings_](per-test-overrides.md).

### Override filters

At least one of these filters must be specified.

#### `filter`

- **Type**: String (filterset expression)
- **Description**: Selects which tests this override applies to
- **Documentation**: [_Filterset DSL_](../filtersets/index.md)
- **Default**: Override applies to all tests
- **Example**: `filter = 'test(integration_test)'`

#### `platform`

- **Type**: String or Object
- **Description**: Platform specification for when override applies
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

- **Type**: String (filterset expression)
- **Description**: Override the default filter for specific platforms
- **Documentation**: [_Running a subset of tests by default_](../running.md#running-a-subset-of-tests-by-default)
- **Note**: Can only be used with `platform` specification

#### `priority`

- **Type**: Integer (-100 to 100)
- **Description**: Test priority (a greater number means a higher priority)
- **Documentation**: [_Test priorities_](test-priorities.md)
- **Default**: `0`

#### Other settings

All profile-level settings can be overridden:

- `threads-required`
- `run-extra-args`
- `retries`
- `slow-timeout`
- `leak-timeout`
- `test-group`
- `success-output`
- `failure-output`
- `junit.store-success-output`
- `junit.store-failure-output`

## Test group configuration

Custom test groups are defined under `[test-groups.<name>]` sections.

For detailed information, see [_Test groups for mutual exclusion_](test-groups.md).

#### `test-groups.<name>.max-threads`

- **Type**: Integer or string
- **Description**: Maximum number of threads this test group can use
- **Valid values**: Positive integer or `"num-cpus"`

## Script configuration

Scripts are configured under `[scripts.setup.<name>]` and `[scripts.wrapper.<name>]` sections.

### Setup scripts

For detailed information, see [_Setup scripts_](setup-scripts.md).

#### `scripts.setup.<name>.command`

- **Type**: String, array, or object
- **Description**: The command to execute
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
- **Description**: Timeout configuration for the setup script
- **Default**: No timeout
- **Examples**:
  ```toml
  slow-timeout = "30s"
  # or
  slow-timeout = { period = "60s", terminate-after = 1, grace-period = "5s" }
  ```

#### `scripts.setup.<name>.leak-timeout`

- **Type**: String (duration) or object
- **Description**: Leak timeout for the setup script
- **Default**: `100ms`
- **Examples**:
  ```toml
  leak-timeout = "500ms"
  # or
  leak-timeout = { period = "1s", result = "fail" }
  ```

#### `scripts.setup.<name>.capture-stdout`

- **Type**: Boolean
- **Description**: Whether to capture stdout from the script
- **Default**: `false`

#### `scripts.setup.<name>.capture-stderr`

- **Type**: Boolean
- **Description**: Whether to capture stderr from the script
- **Default**: `false`

#### `scripts.setup.<name>.junit.store-success-output`

- **Type**: Boolean
- **Description**: Store successful script output in JUnit
- **Default**: `true`

#### `scripts.setup.<name>.junit.store-failure-output`

- **Type**: Boolean
- **Description**: Store failed script output in JUnit
- **Default**: `true`

### Wrapper scripts

For detailed information, see [_Wrapper scripts_](wrapper-scripts.md).

#### `scripts.wrapper.<name>.command`

- **Type**: String, array, or object
- **Description**: The wrapper command to execute
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
- **Description**: How to interact with the configured target runner
- **Documentation**: [_Target runners_](../features/target-runners.md)
- **Valid values**: `"ignore"`, `"overrides-wrapper"`, `"within-wrapper"`, `"around-wrapper"`
- **Default**: `"ignore"`

## Profile script configuration

Profile-specific script configuration under `[[profile.<name>.scripts]]` sections.

### Profile script filters

At least one of these filters must be specified.

#### `platform`

- **Type**: String or object
- **Description**: Platform specification for when scripts apply
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
- **Description**: Test filter for when scripts apply
- **Documentation**: [_Filterset DSL_](../filtersets/index.md)
- **Default**: Scripts apply to all tests
- **Example**: `filter = 'test(integration_test)'`

### Profile script instructions

At least one instruction must be specified.

#### `setup`

- **Type**: String or array of strings
- **Description**: Setup script(s) to run
- **Documentation**: [_Setup scripts_](setup-scripts.md)
- **Examples**:
  ```toml
  setup = "my-setup"
  # or
  setup = ["setup1", "setup2"]
  ```

#### `list-wrapper`

- **Type**: String
- **Description**: Wrapper script to use during test listing
- **Documentation**: [_Wrapper scripts_](wrapper-scripts.md)
- **Example**: `list-wrapper = "my-list-wrapper"`

#### `run-wrapper`

- **Type**: String
- **Description**: Wrapper script to use during test execution
- **Documentation**: [_Wrapper scripts_](wrapper-scripts.md)
- **Example**: `run-wrapper = "my-run-wrapper"`

## Default configuration

The default configuration shipped with cargo-nextest is:

```bash exec="true" result="toml"
cat ../nextest-runner/default-config.toml
```
