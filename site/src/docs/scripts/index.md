---
icon: material/script-text
---

# Scripts

<!-- md:version 0.9.59 -->

Nextest supports running _scripts_ when certain events occur during a test run. Scripts can be scoped to particular tests via [filtersets](../filtersets/index.md).

Nextest currently recognizes two types of scripts:

* [_Setup scripts_](setup.md), which execute at the start of a test run.
* [_Pre-timeout scripts_](pre-timeout.md), which execute before nextest terminates a test that has exceeded its timeout.

Scripts are configured in two parts: _defining scripts_, and _specifying rules_ for when they should be executed.

## Defining scripts

Scripts are defined using the top-level `script.<type>` configuration.

For example, to define a setup script named "my-script", which runs `my-script.sh`:

```toml title="Setup script definition in <code>.config/nextest.toml</code>"
[script.setup.my-script]
command = 'my-script.sh'
# Additional options...
```

See [_Defining setup scripts_](setup.md#defining-setup-scripts) for the additional options available for configuring setup scripts.

To instead define a pre-timeout script named "my-script", which runs `my-script.sh`:

```toml title="Pre-timeout script definition in <code>.config/nextest.toml</code>"
[script.pre-timeout.my-script]
command = 'my-script.sh'
# Additional options...
```

See [_Defining pre-timeout scripts_](pre-timeout.md#defining-pre-timeout-scripts) for the additional options available for configuring pre-timeout scripts.

### Command specification

All script types support the `command` option, which specifies how to invoke the script. Commands can either be specified using Unix shell rules, or as a list of arguments. In the following example, `script1` and `script2` are equivalent.

```toml
[script.<type>.script1]
command = 'script.sh -c "Hello, world!"'

[script.<type>.script2]
command = ['script.sh', '-c', 'Hello, world!']
```

### Namespacing

Script names must be unique across all script types.

This means that you cannot use the same name for a setup script and a pre-timeout script:

```toml title="Pre-timeout script definition in <code>.config/nextest.toml</code>"
[script.setup.my-script]
command = 'setup.sh'

# Reusing the `my-script` name for a pre-timeout script is NOT permitted.
[script.pre-timeout.my-script]
command = 'pre-timeout.sh'
```

## Specifying rules

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
