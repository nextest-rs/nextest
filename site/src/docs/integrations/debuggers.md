---
icon: material/debug-step-over
description: Integrating with gdb, lldb, Visual Studio Code, and other debuggers.
---

# Debugger integration

<!-- md:version 0.9.112 -->

With nextest, you can run individual tests under a text-based or graphical debugger. Supported debuggers include:

* [gdb](https://sourceware.org/gdb/)
* [lldb](https://lldb.llvm.org/)
* [WinDbg](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/)
* [CodeLLDB](https://github.com/vadimcn/codelldb) in Visual Studio Code, via [`codelldb-launch`](https://github.com/vadimcn/codelldb/tree/master/src/codelldb-launch).

Many other debuggers should work out of the box as well.

In debugger mode, nextest will:

* Do the same [environment setup](../configuration/env-vars.md#environment-variables-nextest-sets) that happens while running tests, including environment variables defined by [setup scripts](../configuration/setup-scripts.md#environment-variables).
* Disable [timeouts](../features/slow-tests.md) so that they don't interfere with the debugging process.
* Turn off [keyboard input handling such as `t`](../reporting.md#live-output), and on Unix, most signal handling.
* Disable output capturing, similar to the `--no-capture` argument.
* Pass through standard input to the debugger for interactive terminal use.

## Examples

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

### Debugging tests in Visual Studio Code

Debugging tests with [CodeLLDB](https://github.com/vadimcn/codelldb) in Visual Studio Code requires a small amount of one-time setup.

Install the [CodeLLDB extension](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb) if you haven't already.

Then, install `codelldb-launch`:

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

## How debuggers are executed

When `--debugger` is passed in, its argument is split into fields using Unix shell quoting rules. The debugger is then invoked with the corresponding test command and arguments.

For example, if nextest is invoked as:

```sh
cargo nextest run --debugger "my-debugger --my-arg" my_test
```

with the following conditions:

* `my_test` matches the binary with executable `/path/to/target/debug/deps/tests-ea14630f`.
* The test is `test_mod::my_test_1`.

Then, the command invoked is:

```sh
my-debugger --my-arg /path/to/target/debug/deps/tests-ea14630f --exact test_mod::my_test_1 --nocapture
```

In many cases, you'll want to add `--` to the end of the debugger invocation to prevent test arguments like `--exact` from being interpreted by the debugger.

If your debugger doesn't accept the program and arguments in this fashion, you may be able to write a small shell script which transforms nextest's invocation into the format desired by your debugger.
