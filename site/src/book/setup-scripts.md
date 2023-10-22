# Setup scripts

- **Nextest version:** 0.9.59 and above
- **Enable with:** Add `experimental = ["setup-scripts"]` to `.config/nextest.toml`
- **Tracking issue:** [#978](https://github.com/nextest-rs/nextest/issues/978)

Nextest supports running _setup scripts_ before tests are run. Setup scripts can be scoped to
particular tests via [filter expressions](filter-expressions.md).

Setup scripts are configured in two parts: _defining scripts_, and _setting up rules_ for when they should be executed.

## Defining scripts

Setup scripts are defined using the top-level `script` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml
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

Setup scripts can have the following configuration options attached to them:

- **`slow-timeout`**: Mark a setup script [as slow](slow-tests.md) or [terminate it](slow-tests.md#terminating-tests-after-a-timeout), using the same configuration as for tests. By default, setup scripts are not marked as slow or terminated (this is different from the slow timeout for tests).
- **`leak-timeout`**: Mark setup scripts [leaky](leaky-tests.md) after a timeout, using the same configuration as for tests. By default, the leak timeout is 100ms.
- **`capture-stdout`**: `true` if the script's standard output should be captured, `false` if not. By default, this is `false`.
- **`capture-stderr`**: `true` if the script's standard error should be captured, `false` if not. By default, this is `false`.

### Example

```toml
[script.db-generate]
command = 'cargo run -p db-generate'
slow-timeout = { period = "60s", terminate-after = 2 }
leak-timeout = "1s"
capture-stdout = true
capture-stderr = false
```

## Setting up rules

In configuration, you can create rules for when to use scripts on a per-profile basis. This is done via the `profile.<profile-name>.scripts` array. For example, you can set up a script that generates a database if tests from the `db-tests` package, or any packages that depend on it, are run.

```toml
[[profile.default.scripts]]
filter = 'rdeps(db-tests)'
setup = 'db-generate'
```

(This example uses the `rdeps` [filter expression](filter-expressions.md) predicate.)

Setup scripts can also filter based on platform, using the rules listed in [Specifying platforms](specifying-platforms.md):

```toml
[[profile.default.scripts]]
platform = { host = "cfg(unix)" }
setup = 'script1'
```

A set of scripts can also be specified. All scripts in the set will be executed.

```toml
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
#!/bin/bash

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
