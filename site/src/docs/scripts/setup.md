---
icon: material/ray-start-arrow
status: experimental
---

# Setup scripts

<!-- md:version 0.9.59 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["setup-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)

Nextest runs *setup scripts* before tests are run.

## Defining setup scripts

Setup scripts are defined using the top-level `script.setup` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Script definition in <code>.config/nextest.toml</code>"
[script.setup.my-script]
command = 'my-script.sh'
```

See [_Defining scripts_](index.md#defining-scripts) for options that are common to all scripts.

Setup scripts support the following additional configuration options:

- **`capture-stdout`**: `true` if the script's standard output should be captured, `false` if not. By default, this is `false`.
- **`capture-stderr`**: `true` if the script's standard error should be captured, `false` if not. By default, this is `false`.

### Example

```toml title="Advanced setup script definition"
[script.setup.db-generate]
command = 'cargo run -p db-generate'
capture-stdout = true
capture-stderr = false
```

## Specifying setup script rules

See [_Specifying rules_](index.md#specifying-rules).

## Setup script execution

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
