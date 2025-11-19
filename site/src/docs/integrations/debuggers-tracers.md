---
icon: material/debug-step-over
description: Integrating with gdb, lldb, Visual Studio Code, and other debuggers, and with syscall tracers like strace and truss.
---

# Debugger and tracer integration

<!-- md:version 0.9.113 -->

With nextest, you can run individual tests under a text-based or graphical debugger using `--debugger`, or under a system call tracer using `--tracer`.

## Debuggers

Supported debuggers include:

* [gdb](https://sourceware.org/gdb/)
* [lldb](https://lldb.llvm.org/)
* [WinDbg](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/)
* [CodeLLDB](https://github.com/vadimcn/codelldb) in Visual Studio Code, via [`codelldb-launch`](https://github.com/vadimcn/codelldb/tree/master/src/codelldb-launch).

Many other debuggers should work out of the box as well.

## System call tracers

<!-- md:version 0.9.114 -->

Supported syscall tracers include:

* [strace](https://strace.io/) on Linux
* truss and/or dtruss on other Unix platforms

## Behavior comparison

Both `--debugger` and `--tracer` modify how nextest runs tests, but with somewhat different behaviors. Here's a table comparing behaviors under standard, `--no-capture`, `--tracer`, and `--debugger` modes, with differences from the standard mode bolded:

| Feature                       | Standard                        | `--no-capture`                  | `--tracer`                      | `--debugger`                    |
|:-----------------------------:|:-------------------------------:|:-------------------------------:|:-------------------------------:|:-------------------------------:|
| **Number of tests**           | multiple                        | multiple                        | **exactly one**                 | **exactly one**                 |
| **Test execution**            | parallel                        | **serial**                      | **serial**                      | **serial**                      |
| [**Retries**]                 | enabled                         | enabled                         | **disabled**                    | **disabled**                    |
| **Output capture**            | yes                             | **no**                          | **no**                          | **no**                          |
| **Standard input**            | null                            | null                            | null                            | **passthrough (interactive)**   |
| [**Timeouts**]                | enabled                         | enabled                         | **disabled**                    | **disabled**                    |
| [**Leak detection**]          | enabled                         | enabled                         | **disabled**                    | **disabled**                    |
| **Process groups** (Unix)     | created                         | created                         | created                         | **not created**                 |
| **Signal handling** (Unix)    | standard                        | standard                        | standard                        | **limited**                     |
| **Input handling** (`t` key, etc) | enabled                     | enabled                         | enabled                         | **disabled**                    |

[**Retries**]: ../features/retries.md
[**Timeouts**]: ../features/slow-tests.md
[**Leak detection**]: ../features/leaky-tests.md

Key differences:

* **`--debugger`**: Optimized for interactive debugging.
    * Passes stdin through for debugger commands.
    * On Unix, disables most signal handling to prevent interference.
    * On Unix, doesn't create process groups so the debugger can control the terminal.
* **`--tracer`**: Optimized for non-interactive syscall tracing.
    * Uses null stdin.
    * On Unix, uses standard signal handling.
    * On Unix, creates process groups for better test isolation.

Both modes:

* Do the same [environment setup](../configuration/env-vars.md#environment-variables-nextest-sets) that happens while running tests, including environment variables defined by [setup scripts](../configuration/setup-scripts.md#environment-variables).
* Disable [timeouts](../features/slow-tests.md) so that they don't interfere with the debugging/tracing process.
* Disable output capturing, similar to the `--no-capture` argument.
* Require exactly one test to be selected.

Debugger and tracer modes are intended primarily for local use rather than in CI, so some of the specifics of how the environment is set up may be tweaked over time.

## Examples

### Debuggers

Run the test matching `my_test` under [gdb](https://sourceware.org/gdb/), using `rust-gdb`:

```sh
cargo nextest run --debugger "rust-gdb --args" my_test
```

Run the test matching `my_test` under [lldb](https://lldb.llvm.org/), using `rust-lldb`:

```sh
cargo nextest run --debugger "rust-lldb --" my_test
```

Run the test matching `my_test` under [WinDbg](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/):

```sh
cargo nextest run --debugger windbgx my_test
```

### Syscall tracers

Log all system calls performed by the test matching `my_test`:

```sh
# Linux
cargo nextest run --tracer strace my_test
# macOS
cargo nextest run --tracer dtruss my_test
# illumos and other platforms with truss
cargo nextest run --tracer truss my_test
```

These utilities accept a variety of options for filtering and redirecting output; see their corresponding man pages for more information. For example, to also follow any child processes your test might create:

```sh
# Linux
cargo nextest run --tracer "strace -f" my_test
# macOS
cargo nextest run --tracer "dtruss -f" my_test
# illumos and other platforms with truss
cargo nextest run --tracer "truss -f" my_test
```

!!! note "Using `--debugger` with tracers"

    You can still use `--debugger` with syscall tracers, but `--tracer` provides better behavior for non-interactive tracing (null stdin, standard signal handling, and process groups for isolation).

### Debugging tests in Visual Studio Code

Debugging tests with [CodeLLDB](https://github.com/vadimcn/codelldb) in Visual Studio Code requires a small amount of one-time setup.

Install the [CodeLLDB extension](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb) if you haven't already.

Then, install [the `codelldb-launch` tool](https://github.com/vadimcn/codelldb/tree/master/src/codelldb-launch):

```
cargo install --locked --git https://github.com/vadimcn/codelldb codelldb-launch
```

After that, open Visual Studio Code, set up your breakpoints, and enable the [CodeLLDB RPC server](https://github.com/vadimcn/codelldb/blob/master/MANUAL.md#rpc-server) by adding `lldb.rpcServer` to the workspace configuration.

```json title="Add to .vscode/settings.json"
{
  // ...
  "lldb.rpcServer": {
    "host": "127.0.0.1",
    "port": 12345,
    "token": "secret"
  },
  // ...
}
```

In your terminal, set up the token, then execute the test under the `codelldb-launch` debugger:

```sh
export CODELLDB_LAUNCH_CONFIG="{ token: 'secret' }"
cargo nextest run --debugger "codelldb-launch --connect 127.0.0.1:12345 --" my_test
```

For more information about CodeLLDB, see [its manual](https://github.com/vadimcn/codelldb/blob/master/MANUAL.md).

!!! tip "If breakpoints aren't being hit"

    If you're not seeing your breakpoints being hit, see [these instructions in the CodeLLDB wiki](https://github.com/vadimcn/codelldb/wiki/Breakpoints-are-not-getting-hit).
    
    A common cause of breakpoints not being hit is a missing [source remap](https://github.com/vadimcn/codelldb/blob/master/MANUAL.md#source-path-remapping). This can happen if your source directory is symlinked to somewhere else. For example, if `/home/user/dev/dir` is symlinked to `/opt/src/dev/dir`, try adding to `.vscode/settings.json`:
    
    ```json
    {
      // ...
      "lldb.launch.sourceMap": {
        "/opt/src/dev/dir": "/home/user/dev/dir",
      }
      // ...
    }
    ```

## How debuggers and tracers are executed

When `--debugger` or `--tracer` is passed in, its argument is split into fields using Unix shell quoting rules. The debugger or tracer is then invoked with the corresponding test command and arguments.

For example, if nextest is invoked as:

```sh
cargo nextest run --tracer "strace -f" my_test
```

with the following conditions:

* `my_test` matches the binary with executable `/path/to/target/debug/deps/tests-ea14630f`.
* The test is `test_mod::my_test_1`.

Then, the command invoked is:

```sh
strace -f /path/to/target/debug/deps/tests-ea14630f --exact test_mod::my_test_1 --nocapture
```

In many cases, you'll want to add `--` to the end of the debugger invocation to prevent test arguments like `--exact` from being interpreted by the debugger.

If your debugger or tracer doesn't accept the program and arguments in this fashion, you may be able to write a small shell script which transforms nextest's invocation into the format desired by your tool.
