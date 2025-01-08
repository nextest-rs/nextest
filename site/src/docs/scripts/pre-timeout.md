---
icon: material/timer-sand-empty
status: experimental
---

<!-- md:version 0.9.59 -->

# Pre-timeout scripts

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["pre-timeout-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#TODO](https://github.com/nextest-rs/nextest/issues/TODO)


Nextest runs *pre-timeout scripts* before terminating a test that has exceeded
its timeout.

Pre-timeout scripts are useful for automatically collecting backtraces, logs, etc. that can assist in debugging why a test is slow or hung.

## Defining pre-timeout scripts

Pre-timeout scripts are defined using the top-level `script.pre-timeout` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Script definition in <code>.config/nextest.toml</code>"
[script.pre-timeout.my-script]
command = 'my-script.sh'
```

See [_Defining scripts_](index.md#defining-scripts) for options that are common to all scripts.

Pre-timeout scripts do not support additional configuration options.

Notably, pre-timeout scripts always capture stdout and stderr. Support for not capturing stdout and stderr may be added in the future in order to support use cases like interactive debugging of a hung test.

### Example

To invoke GDB to dump backtraces before a hanging test is terminated:

```toml title="Advanced pre-timeout script definition"
[script.pre-timeout.gdb-dump]
command = ['sh', '-c', 'gdb -p $NEXTEST_PRE_TIMEOUT_TEST_PID -batch -ex "thread apply all backtrace"']
# TODO options
```

## Specifying pre-timeout script rules

See [_Specifying rules_](index.md#specifying-rules).

## Pre-timeout script execution

A given pre-timeout script _S_ is executed when the current profile has at least one rule where the `platform` predicates match the current execution environment, the script _S_ is listed in `pre-timeout`, and a test matching the `filter` has reached its configured timeout.

Pre-timeout scripts are executed serially, in the order they are defined (_not_ the order they're specified in the rules). If any pre-timeout script exits with a non-zero exit code, an error is logged but the test run continues.

Nextest will proceed with graceful termination of the test only once the pre-timeout script terminates. See [_How nextest terminates tests_](#defining-pre-timeout-scripts). If the pre-timeout script itself is slow, nextest will apply the same termination protocol to the pre-timeout script.

The pre-timeout script is not responsible for terminating the test process, but it is permissible for it to do so.

Nextest executes pre-timeout scripts with the same working directory as the test and sets the following variables in the script's environment:

* **`NEXTEST_PRE_TIMEOUT_TEST_PID`**: the ID of the process running the test.
* **`NEXTEST_PRE_TIMEOUT_TEST_NAME`**: the name of the running test.
* **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_ID`**: the ID of the binary in which the test is located.
* **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_ID_PACKAGE_NAME`**: the package name component of the binary ID.
* **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_ID_NAME`**: the name component of the binary ID, if known.
* **`NEXTEST_PRE_TIMEOUT_TEST_BINARY_ID_KIND`**: the kind component of the binary ID, if known.

<!-- TODO: a protocol for writing script logs to a file and telling nextest to attach them to JUnit reports? -->
