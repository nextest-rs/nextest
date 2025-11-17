---
icon: material/layers-outline
status: experimental
description: Wrapping test execution with custom commands using filtersets and platform-specific scoping.
---

# Wrapper scripts

<!-- md:version 0.9.98 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["wrapper-scripts"]` to `.config/nextest.toml`
    - **Tracking issue:** [#2384](https://github.com/nextest-rs/nextest/issues/2384)

!!! warning

    This is an advanced feature, and it can cause your tests to silently stop
    working if used incorrectly. Use with caution.

Nextest supports wrapping test execution with custom commands via _wrapper scripts_.

Wrapper scripts can be scoped to:

* Sets of tests, using [filtersets](../filtersets/index.md).
* Specific platforms, using [`cfg` expressions](../configuration/specifying-platforms.md).

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

Setting `relative-to` does not change the working directory of the test. It just prepends the directory to the command if it is relative.

```toml
[scripts.wrapper.script1]
command = { command-line = "debug/my-wrapper-bin", relative-to = "target" }

[scripts.wrapper.script2]
command = { command-line = "scripts/wrapper-script.sh", relative-to = "workspace-root" }
```

A wrapper script will be invoked with the test binary as the first argument, and the argument list passed in as subsequent arguments.

!!! warning

    Make sure your wrapper script runs the test binary and arguments passed into it! If you do not do so, your test will succeed even though it isn't being executed.

### Wrapper script configuration

Wrapper scripts can have the following configuration options attached to them:

`target-runner`
: Interaction with [target runners](../features/target-runners.md), if one is specified. The following values are permitted:

  `ignore`
  : The target runner is disabled, and only the wrapper script is used. This is the default.

  `overrides-wrapper`
  : The wrapper script is disabled, and only the target runner is used.

  `within-wrapper`
  : Run the target runner as an argument to the wrapper.

    For example, if the target runner is `qemu-arm` and the wrapper is `valgrind --leak-check=full`, the full command that's run is `valgrind --leak-check=full qemu-arm <test-binary> <args...>`.

  `around-wrapper`
  : Run the wrapper script as an argument to the target runner.

    For example, if the target runner is `my-linux-emulator` and the wrapper is `sudo`, the full command that's run is `my-linux-emulator sudo <test-binary> <args...>`.

## Setting up rules

In configuration, you can create rules for when to use scripts on a per-profile basis. This is done via the `profile.<profile-name>.scripts` array.

Wrapper scripts can be invoked:

* While listing tests, with the `list-wrapper` instruction
* While running tests, with the `run-wrapper` instruction
* Or both, if both `list-wrapper` and `run-wrapper` are specified.

### Examples

!!! danger

    While running tests as root is necessary in some situations, a test running as root on the host computer can potentially **damage the system**. If at all possible, consider having the wrapper script run the test within a container instead. Running tests as root within a container is meaningfully safer than running them as root on the host.

    For tests that must be run as root, scope them as tightly as possible using a precise filterset. A filterset of the kind `binary_id(binary_name) and test(=test_name)` is recommended.

Here's an example of setting up `sudo` on Linux in CI, assuming you have configured your CI to allow `sudo` without prompting:

```toml title="Basic rules"
[scripts.wrapper.sudo-script]
command = 'sudo'

[[profile.ci.scripts]]
filter = 'binary_id(package::binary) and test(=root_test)'
platform = 'cfg(target_os = "linux")'
run-wrapper = 'sudo-script'
```

As shown above, wrapper scripts can also filter based on platform, using the rules listed in [_Specifying platforms_](specifying-platforms.md).

In some cases you may also need to use a wrapper for listing tests. For example:

```toml title="Using a wrapper for both listing and running tests"
[scripts.wrapper.wine-script]
command = 'wine'

[[profile.windows-tests.scripts]]
filter = 'binary(windows_compat_tests)'
platform = { host = 'cfg(unix)', target = 'cfg(windows)' }
list-wrapper = 'wine-script'
run-wrapper = 'wine-script'
```

If `list-wrapper` is specified, `filter` cannot contain `test()` or `default()` predicates, since those predicates can only be evaluated after listing is completed.

## Wrapper scripts vs target runners

Both wrapper scripts and [target runners](../features/target-runners.md) can be used to wrap test executables. The key differences between the two are related to configurability and scope.

| Feature                 | Wrapper scripts                                                                                                 | Target runners                                 |
| ----------------------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| **Configuration scope** | Fine-grained filtering by test name, binary, etc.                                                               | Global, for all tests per execution            |
| **List vs run phase**   | Can be selectively used for list, run, or both                                                                  | Always used for both list and run phases       |
| **Compatibility**       | Only supported by nextest                                                                                       | Wide compatibility, supported by Cargo         |
| **Multiple scripts**    | Multiple wrapper scripts can be defined and applied selectively                                                 | Only one target runner can be active at a time |
| **Use cases**           | Running tests with `sudo`, memory checkers like `valgrind`, profilers, cross-compilation, emulators like `qemu` | Cross-compilation, emulators like `qemu`       |

## Wrapper script precedence

Wrapper scripts follow the same precedence order as [per-test settings](per-test-overrides.md#override-precedence).

* The `list-wrapper` and `run-wrapper` configurations are evaluated separately.
* Only the first wrapper script that matches a test is used.
