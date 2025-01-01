---
icon: material/timer-sand-empty
status: experimental
---

<!-- md:version TODO -->

# Pre-timeout scripts

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["setup-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)


Nextest runs *pre-timeout scripts* before terminating a test that has exceeded
its timeout.

## Defining pre-timeout scripts

Pre-timeout scripts are defined using the top-level `script.pre-timeout` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Script definition in <code>.config/nextest.toml</code>"
[script.pre-timeout.my-script]
command = 'my-script.sh'
```

Pre-timeout scripts can have the following configuration options attached to them:

- TODO

### Example

```toml title="Advanced pre-timeout script definition"
[script.pre-timeout.gdb-dump]
command = 'gdb ... TODO'
# TODO options
```

## Specifying pre-timeout script rules

See [_Specifying rules_](index.md#specifying-rules).

## Pre-timeout script execution

A given pre-timeout script _S_ is executed when the current profile has at least one rule where the `platform` predicates match the current execution environment, the script _S_ is listed in `pre-timeout`, and a test matching the `filter` has reached its configured timeout.

Pre-timeout scripts are executed serially, in the order they are defined (_not_ the order they're specified in the rules). If any pre-timeout script exits with a non-zero exit code, an error is logged but the test run continues.

Nextest sets the following environment variables when executing a pre-timeout script:

  * **`NEXTEST_PRE_TIMEOUT_TEST_PID`**: the ID of the process running the test.
  * **`NEXTEST_PRE_TIMEOUT_TEST_NAME`**: the name of the running test.
  * **`NEXTEST_PRE_TIMEOUT_TEST_PACKAGE_NAME`**: the name of the package in which the test is located.
  * **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_NAME`**: the name of the test binary, if known.
  * **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_KIND`**: the kind of the test binary, if known.
