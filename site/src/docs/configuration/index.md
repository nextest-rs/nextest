---
icon: material/tune
description: "Configuring nextest: information about profiles and hierarchical configuration."
---

# Configuring nextest

cargo-nextest supports repository-specific configuration at the location `.config/nextest.toml` from the Cargo workspace root. The location of the configuration file can be overridden with the `--config-file` option.

For a comprehensive list of all configuration parameters, including default values, see [_Configuration reference_](reference.md).

## Profiles

With cargo-nextest, local and CI runs often need to use different settings. For example, CI test runs should not be cancelled as soon as the first test failure is seen.

cargo-nextest supports multiple _profiles_, where each profile is a set of options for cargo-nextest. Profiles are selected on the command line with the `-P` or `--profile` option. Most individual configuration settings can also be overridden at the command line.

Here is a recommended profile for CI runs:

```toml title="Configuring a CI profile in <code>.config/nextest.toml</code>"
[profile.ci]
# Do not cancel the test run on the first failure.
fail-fast = false
```

After checking the profile into `.config/nextest.toml`, use `cargo nextest --profile ci` in your CI runs.

!!! note "Default profiles"

    Nextest's embedded configuration may define new profiles whose names start with `default-` in the future. To avoid backwards compatibility issues, do not name custom profiles starting with `default-`.
  
### Profile inheritance

<!-- md:version 0.9.115 -->

By default, all custom profiles inherit their configuration from the profile named `default`. To inherit from another profile, specify the `inherits` key:

```toml title="Inheriting from another profile in <code>.config/nextest.toml</code>"
[profile.ci]
fail-fast = false
slow-timeout = "60s"

[profile.ci-extended]
inherits = "ci"
slow-timeout = "300s"
```

A series of profile `inherits` keys form an _inheritance chain_, and configuration lookups are done by iterating over the chain.

!!! note "The default profile cannot inherit from another profile"

    The `default` profile cannot be made to inherit from another profile; it is always at the root of any inheritance chain.

## Tool-specific configuration

Some tools that [integrate with nextest](../integrations/index.md) may wish to customize nextest's defaults. However, in most cases, command-line arguments and repository-specific configuration should still override those defaults.

To support these tools, nextest supports the `--tool-config-file` argument. Values to this argument are specified in the form `tool:/path/to/config.toml`. For example, if your tool `my-tool` needs to call nextest with customized defaults, it should run:

```
cargo nextest run --tool-config-file my-tool:/path/to/my/config.toml
```

The `--tool-config-file` argument may be specified multiple times. Config files specified earlier are higher priority than those that come later.

## Hierarchical configuration

Configuration is resolved in the following order:

1. Command-line arguments. For example, if `--retries=3` is specified on the command line, failing tests are retried up to 3 times.
2. Environment variables. For example, if `NEXTEST_RETRIES=4` is specified on the command line, failing tests are retried up to 4 times.
3. [Per-test overrides](per-test-overrides.md), if they're supported for this configuration variable.
4. If a profile is specified, profile-specific configuration in `.config/nextest.toml`. For example, if the repository-specific configuration looks like:

   ```toml
   [profile.ci]
   retries = 2
   ```

   then, if `--profile ci` is selected, failing tests are retried up to 2 times.

5. If a profile is specified, tool-specific configuration for the given profile.
6. For each profile in the inheritance chain, which always terminates at the `default` profile:
  1. Repository-specific configuration for that profile profile. For example, if the repository-specific configuration looks like:
   ```toml
   [profile.ci-extended]
   inherits = "ci"
   
   [profile.ci]
   retries = 5
   ```
   then, with the `ci-extended` profile, failing tests are retried up to 5 times.
  b. Tool-specific configuration for that profile.
8. The [default configuration](reference.md#default-configuration), which is that tests are never retried.

## See also

- [Configuration reference](reference.md) - comprehensive list of all configuration parameters
- [Per-test settings](per-test-overrides.md) - customize settings for specific tests
