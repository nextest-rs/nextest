# Configuration

cargo-nextest supports repository-specific configuration at the location `.config/nextest.toml` from the Cargo workspace root. The location of the configuration file can be overridden with the `--config-file` option.

The default configuration shipped with cargo-nextest is:

```toml
{{#include ../../../nextest-runner/default-config.toml}}
```

## Profiles

With cargo-nextest, local and CI runs often need to use different settings. For example, CI test runs should not be cancelled as soon as the first test failure is seen.

cargo-nextest supports multiple *profiles*, where each profile is a set of options for cargo-nextest. Profiles are selected on the command line with the `-P` or `--profile` option. Most individual configuration settings can also be overridden at the command line.

Here is a recommended profile for CI runs:

```toml
[profile.ci]
# Print out output for failing tests as soon as they fail, and also at the end
# of the run (for easy scrollability).
failure-output = "immediate-final"
# Do not cancel the test run on the first failure.
fail-fast = false
```

After checking the profile into `.config/nextest.toml`, use `cargo nextest --profile ci` in your CI runs.

> **Note:** Nextest's embedded configuration may define new profiles whose names start with `default-` in the future. To avoid backwards compatibility issues, do not name custom profiles starting with `default-`.

## Hierarchical configuration

For this example:

Configuration is resolved in the following order:
1. Command-line arguments. For example, if `--retries=3` is specified on the command line, failing tests are retried up to 3 times.
2. Environment variables. For example, if `NEXTEST_RETRIES=4` is specified on the command line, failing tests are retried up to 4 times.
3. [Per-test overrides](per-test-overrides.md), if they're supported for this configuration variable.
4. Profile-specific configuration. For example, if the repository-specific configuration looks like:
    ```toml
    [profile.ci]
    retries = 2
    ```

    then, if `--profile ci` is selected, failing tests are retried up to 2 times.
5. Repository-specific configuration for the `default` profile. For example, if the repository-specific configuration looks like:
    ```toml
    [profile.default]
    retries = 5
    ```
    then failing tests are retried up to 5 times.
6. The default configuration listed above, which is that tests are never retried.
