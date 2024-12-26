---
icon: material/ray-start-arrow
status: experimental
---

# Scripts

<!-- md:version 0.9.59 -->

!!! experimental "Experimental: Setup scripts are not yet stable"

    - **Enable with:** Add `experimental = ["setup-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)

!!! experimental "Experimental: Pre-timeout scripts are not yet stable"

    - **Enable with:** Add `experimental = ["pre-timeout-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)

Nextest supports running _scripts_ when certain events occur during a test run. Scripts can be scoped to particular tests via [filtersets](../filtersets/index.md).

Nextest currently recognizes two types of scripts:

  * _Setup scripts_, which are executed at the start of a test run.
  * _Pre-timeout scripts_, which are executed before nextest begins terminating a test that has exceeded its timeout. 

Scripts are configured in two parts: _defining scripts_, and _setting up rules_ for when they should be executed.

## Defining scripts

Scripts are defined using the top-level `script` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Script definition in <code>.config/nextest.toml</code>"
[script.my-script]
command = 'my-script.sh'
```

Commands can either be specified using Unix shell rules, or as a list of arguments. In the following example, `script1` and `script2` are equivalent.

```toml
[script.script1]
command = 'script.sh -c "Hello, world!"'

[script.script2]
command = ['script.sh', '-c', 'Hello, world!']
```

Scripts can have the following configuration options attached to them:

- **`slow-timeout`**: Mark a script [as slow](../features/slow-tests.md) or [terminate it](../features/slow-tests.md#terminating-tests-after-a-timeout), using the same configuration as for tests. By default, scripts are not marked as slow or terminated (this is different from the slow timeout for tests).
- **`leak-timeout`**: Mark scripts [leaky](../features/leaky-tests.md) after a timeout, using the same configuration as for tests. By default, the leak timeout is 100ms.
- **`capture-stdout`**: `true` if the script's standard output should be captured, `false` if not. By default, this is `false`.
- **`capture-stderr`**: `true` if the script's standard error should be captured, `false` if not. By default, this is `false`.

### Example

```toml title="Advanced script definition"
[script.db-generate]
command = 'cargo run -p db-generate'
slow-timeout = { period = "60s", terminate-after = 2 }
leak-timeout = "1s"
capture-stdout = true
capture-stderr = false
```

## Setting up rules

In configuration, you can create rules for when to use scripts on a per-profile basis. This is done via the `profile.<profile-name>.scripts` array. For example, you can configure a setup script that generates a database if tests from the `db-tests` package, or any packages that depend on it, are run.

```toml title="Basic rules"
[[profile.default.scripts]]
filter = 'rdeps(db-tests)'
setup = 'db-generate'
```

(This example uses the `rdeps` [filterset](../filtersets/index.md) predicate.)

Scripts can also filter based on platform, using the rules listed in [_Specifying platforms_](../configuration/specifying-platforms.md):

```toml title="Platform-specific rules"
[[profile.default.scripts]]
platform = { host = "cfg(unix)" }
setup = 'script1'
```

A set of scripts can also be specified. All scripts in the set will be executed.

```toml title="Multiple setup scripts"
[[profile.default.scripts]]
filter = 'test(/^script_tests::/)'
setup = ['script1', 'script2']
```

Executing pre-timeout scripts follows the same pattern. For example, you can configure a pre-timeout script for every test that contains `slow` in its name.

```toml title="Basic pre-timeout rules"
[[profile.default.scripts]]
filter = 'test(slow)'
pre-timeout = 'capture-backtrace'
```

A single rule can specify any number of setup scripts and any number of pre-timeout scripts.

```toml title="Combination rules"
[[profile.default.scripts]]
filter = 'test(slow)'
setup = ['setup-1', 'setup-2']
pre-timeout = ['pre-timeout-1', 'pre-timeout-2']
```

## Script execution

### Setup scripts

A given setup script _S_ is only executed if the current profile has at least one rule where the `filter` and `platform` predicates match the current execution environment, and the setup script _S_ is listed in `setup`.

Setup scripts are executed serially, in the order they are defined (_not_ the order they're specified in the rules). If any setup script exits with a non-zero exit code, the entire test run is terminated.

#### Environment variables

Setup scripts can define environment variables that will be exposed to tests that match the script. This is done by writing to the `$NEXTEST_ENV` environment variable from within the script.

For example, let's say you have a script `my-env-script.sh`:

```bash
#!/usr/bin/env bash

# Exit with 1 if NEXTEST_ENV isn't defined.
if [ -z "$NEXTEST_ENV" ]; then
    exit 1
fi

# Write out an environment variable to $NEXTEST_ENV.
echo "MY_ENV_VAR=Hello, world!" >> "$NEXTEST_ENV"
```

And you define a setup script and a corresponding rule:

```toml
[script.my-env-script]
command = 'my-env-script.sh'

[[profile.default.scripts]]
filter = 'test(my_env_test)'
setup = 'my-env-script'
```

Then, in tests which match this script, the environment variable will be available:

```rust
#[test]
fn my_env_test() {
    assert_eq!(std::env::var("MY_ENV_VAR"), Ok("Hello, world!".to_string()));
}
```

### Pre-timeout scripts

A given pre-timeout script _S_ is executed when the current profile has at least one rule where the `platform` predicates match the current execution environment, the script _S_ is listed in `pre-timeout`, and a test matching the `filter` has reached its configured timeout.

Pre-timeout scripts are executed serially, in the order they are defined (_not_ the order they're specified in the rules). If any pre-timeout script exits with a non-zero exit code, an error is logged but the test run continues.

Nextest sets the following environment variables when executing a pre-timeout script:

  * **`NEXTEST_PRE_TIMEOUT_TEST_PID`**: the ID of the process running the test.
  * **`NEXTEST_PRE_TIMEOUT_TEST_NAME`**: the name of the running test.
  * **`NEXTEST_PRE_TIMEOUT_TEST_BINARY`**: the name of the binary running the test.

## Setup scripts in JUnit output

<!-- md:version 0.9.86 -->

If nextest's [JUnit support](../machine-readable/junit.md) is enabled, information
about setup scripts is included in the JUnit output.

JUnit doesn't have native support for setup scripts, so nextest represents them as
individual tests:

- Each setup script is represented as a separate `<testsuite>` element, with the
`name` property set to `@setup-script:[script-name]`. Each test suite has a single
`<testcase>` element, with the `name` property set to the script's name.
- As a result, setup script adds 1 to the number of tests in the root `<testsuites>`
  element.
- A failure or timeout in the script is represented as a `<failure>` element.
- An execution error to start the script is represented as an `<error>` element.
- In the `<testsuite>` element's `<properties>` section, the following properties
  are added:
  - `command`: The command that was executed.
  - `args`: The arguments that were passed to the command, concatenated via Unix shell rules.
  - For each environment variable set by the script, a property is added with the
    name `output-env:[env-name]`, and the value the environment variable's value.

### Standard output and standard error

If captured, the script's standard output and standard error are included as
`<system-out>` and `<system-err>` elements, respectively. Unlike with tests,
where by default output is only included if the test failed, with scripts they
are always included by default. To alter this behavior, use the
`junit.store-success-output` and/or `junit.store-failure-output` configuration
settings:

```toml title="Configuration to control JUnit output for setup scripts"
[script.my-script]
command = 'my-script.sh'
junit.store-success-output = false
junit.store-failure-output = true
```

### Example: JUnit output

Here's an example of a `testsuite` element corresponding to a setup script:

```bash exec="true" result="xml"
cat ../fixtures/setup-script-junit.xml
```
