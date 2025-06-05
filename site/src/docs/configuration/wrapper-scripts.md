---
icon: material/layers-outline
status: experimental
---

# Wrapper scripts

<!-- md:version 0.9.98 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["wrapper-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#2384](https://github.com/nextest-rs/nextest/issues/2384)

Nextest supports wrapping test execution with custom commands via _wrapper scripts_.

Wrapper scripts can be scoped to:

- Particular tests via [filtersets](../filtersets/index.md)
- And to particular platforms.

Wrapper scripts are configured in two parts: _defining scripts_, and _setting up rules_ for when they should be executed.

## Defining scripts

Wrapper scripts are defined using the top-level `[scripts.wrapper]` configuration. For example, to define a script named "my-script", which runs `my-script.sh`:

```toml title="Wrapper script definition in <code>.config/nextest.toml</code>"
[scripts.wrapper.my-script]
command = 'my-script.sh'
```

Commands can either be specified using Unix shell rules, or as a list of arguments. In the following example, `script1` and `script2` are equivalent.

```toml
[scripts.wrapper.script1]
command = 'script.sh -c "Hello, world!"'

[scripts.wrapper.script2]
command = ['script.sh', '-c', 'Hello, world!']
```

Commands can also be interpreted as relative to the target directory. This does not change the working directory of the test—it just prepends the target directory to the command if it is relative.

```toml
[scripts.wrapper.script1]
command = { command-line = "debug/my-wrapper-bin", relative-to = "target" }
```

A wrapper script will be invoked with the test binary as the first argument, and the argument list passed in as subsequent arguments.

### Wrapper script configuration

Wrapper scripts can have the following configuration options attached to them:

- **`target-runner`**: Interaction with [target runners](../features/target-runners.md), if one is specified. The following values are permitted:

  - **`within-wrapper`**: Run the target runner as an argument the wrapper script. For example, if the target runner is `qemu-arm` and the wrapper is `valgrind --leak-check=full`, the full command that's run is `valgrind --leak-check=full qemu-arm <test-binary> <args...>`.

  - **`around-wrapper`**: Run the wrapper script as an argument to the target runner. For example, if the target runner is `my-linux-emulator` and the wrapper is `sudo`, then the full command that's run is `my-linux-emulator sudo <test-binary> <args...>`.

  - **`overrides-wrapper`**: The wrapper script is disabled, and only the target runner is used.

  - **`ignore`**: The target runner is disabled, and only the wrapper script is used.

## Setting up rules

In configuration, you can create rules for when to use scripts on a per-profile basis. This is done via the `profile.<profile-name>.scripts` array.

Wrapper scripts can be invoked:

* While listing tests, with the `list-wrapper` instruction
* While running tests, with the `run-wrapper` instruction
* Or both, if both `list-wrapper` and `run-wrapper` are specified.

### Examples

In some situations, tests must be run as root. Here's an example of setting up `sudo` on Linux in CI, assuming you have configured your CI to allow `sudo` without prompting:

```toml title="Basic rules"
[scripts.wrapper.sudo-script]
command = 'sudo'

[[profile.ci.scripts]]
filter = 'test(root_tests)'
# A platform can also be specified.
platform = { host = 'cfg(target_os = "linux")' }
run-wrapper = 'sudo-script'
```

As shown above, wrapper scripts can also filter based on platform, using the rules listed in [_Specifying platforms_](specifying-platforms.md).

In some cases you may also need to use a wrapper for listing tests. For example:

```toml title="Using a wrapper for both listing and running tests"
[scripts.wrapper.wine-script]
command = 'wine'

[[profile.windows-tests.scripts]]
filter = 'binary(windows_compat_tests)'
list-wrapper = 'wine-script'
run-wrapper = 'wine-script'
```

If `list-wrapper` is specified, `filter` cannot contain `test()` or `default()` predicates, since those predicates can only be evaluated after listing is completed.

## Wrapper script precedence

Wrapper scripts follow the same precedence order as [per-test settings](per-test-overrides.md#override-precedence).

* The `list-wrapper` and `run-wrapper` configurations are evaluated separately.
* Only the first wrapper script that matches a test is run.
