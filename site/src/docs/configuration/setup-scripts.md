---
icon: material/ray-start-arrow
status: experimental
description: Running scripts before tests with filtersets and platform-specific scoping.
---

# Setup scripts

<!-- md:version 0.9.98 --> (originally <!-- md:version 0.9.59 -->)

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["setup-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)

Nextest supports running _setup scripts_ before tests are run. Setup scripts can be scoped to:

* Sets of tests, using [filtersets](../filtersets/index.md).
* Specific platforms, using [`cfg` expressions](specifying-platforms.md)

Setup scripts are configured in two parts: _defining scripts_, and _setting up rules_ for when they should be executed.

## Defining scripts

Setup scripts are defined using the top-level `scripts.setup` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Setup script definition in <code>.config/nextest.toml</code>"
[scripts.setup.my-script]
command = 'my-script.sh'
```

(In versions of nextest before 0.9.98, setup scripts were specified in the top-level `[script.*]` configuration. This configuration is deprecated but still currently supported. It will be removed in a future version of nextest.)

Commands can either be specified using Unix shell rules, or as a list of arguments. In the following example, `script1` and `script2` are equivalent.

```toml
[scripts.setup.script1]
command = 'script.sh -c "Hello, world!"'

[scripts.setup.script2]
command = ['script.sh', '-c', 'Hello, world!']
```

### Specifying `relative-to`

Commands can be interpreted as relative to a particular directory by specifying the `relative-to` parameter:

<div class="compact" markdown>

`"none"`
: Do not alter the command. This is the default value.

`"target"`
: The target directory.

`"workspace-root"` <!-- md:version 0.9.99 -->
: The workspace root.

</div>

Setting `relative-to` does not change the working directory of the setup script, which is always the workspace root. It just prepends the directory to the command if it is relative.

```toml
[scripts.setup.script1]
command = { command-line = "debug/my-setup-bin", relative-to = "target" }

[scripts.setup.script2]
command = { command-line = "scripts/setup-script.sh", relative-to = "workspace-root" }
```

### Specifying `env`

A map of environment variables may be passed to a command by specifying the `env` parameter.

```toml
[scripts.setup.script1]
command = {
    command-line = "cargo run -p setup-test-db",
    env = {
        DB_PATH = "sqlite:/path/to/test.db",
    },
}
```

Note that keys cannot begin with `NEXTEST` as that is reserved for internal use, and values defined in this map will override values set by the environment and by Cargo's `config.toml`.

### Setup script configuration

Setup scripts can have the following configuration options attached to them:

`slow-timeout`
: Mark a setup script [as slow](../features/slow-tests.md) or [terminate it](../features/slow-tests.md#terminating-tests-after-a-timeout), using the same configuration as for tests. By default, setup scripts are not marked as slow or terminated (this is different from the slow timeout for tests).

`leak-timeout`
: Mark setup scripts [leaky](../features/leaky-tests.md) after a timeout, using the same configuration as for tests. By default, the leak timeout is 100ms.

`capture-stdout`
: `true` if the script's standard output should be captured, `false` if not. By default, this is `false`.

`capture-stderr`
: `true` if the script's standard error should be captured, `false` if not. By default, this is `false`.

### Example

```toml title="Advanced setup script definition"
[scripts.setup.db-generate]
command = 'cargo run -p db-generate'
slow-timeout = { period = "60s", terminate-after = 2 }
leak-timeout = "1s"
capture-stdout = true
capture-stderr = false
```

## Setting up rules

In configuration, you can create rules for when to use scripts on a per-profile basis. This is done via the `profile.<profile-name>.scripts` array. For example, you can set up a script that generates a database if tests from the `db-tests` package, or any packages that depend on it, are run.

```toml title="Basic rules"
[[profile.default.scripts]]
filter = 'rdeps(db-tests)'
setup = 'db-generate'
```

(This example uses the `rdeps` [filterset](../filtersets/index.md) predicate.)

Setup scripts can also filter based on platform, using the rules listed in [_Specifying platforms_](../configuration/specifying-platforms.md):

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

## Script execution

A given setup script _S_ is only executed if the current profile has at least one rule where the `filter` and `platform` predicates match the current execution environment, and the setup script _S_ is listed in `setup`.

Setup scripts are executed serially, in the order they are defined (_not_ the order they're specified in the rules). If any setup script exits with a non-zero exit code, the entire test run is terminated.

### Environment variables

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
[scripts.setup.my-env-script]
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
[scripts.setup.my-script]
command = 'my-script.sh'
junit.store-success-output = false
junit.store-failure-output = true
```

### Example: JUnit output

Here's an example of a `testsuite` element corresponding to a setup script:

```bash exec="true" result="xml"
cat ../fixtures/setup-script-junit.xml
```
