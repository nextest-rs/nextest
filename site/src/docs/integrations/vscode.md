---
icon: material/microsoft-visual-studio-code
description: Integrate nextest with Visual Studio Code.
---

# Visual Studio Code

Nextest integrates with Visual Studio Code so you can run and debug tests.

## Requirements

- rust-analyzer 0.3.2862 or above (installed automatically by VS Code's [rust-analyzer extension](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer&ssr=false#review-details))
- A recent version of nextest.

## Running tests from within VS Code

Add this to your [VS Code settings](https://code.visualstudio.com/docs/configure/settings), either for an individual workspace or globally in your user settings:

```json title="Add to .vscode/settings.json"
{
    // ...
    "rust-analyzer.runnables.test.overrideCommand": [
        "cargo",
        "nextest",
        "run",
        "--package",
        "${package}",
        "${target_arg}",
        "${target}",
        "--",
        "${test_name}",
        "${exact}",
        "${include_ignored}"
    ],
    // ...
}
```

Then, the Run Test and Run Tests buttons will invoke nextest.

![Screenshot of VS Code showing a Rust test file. Inline "Run Tests" and "Run Test" CodeLens buttons appear above a test module and an individual test function, both circled in red. The integrated terminal below shows nextest output with one test passing and 603 skipped.](../../static/vscode-run-tests.png)

## Debugging tests in VS Code

To debug a test with the nextest environment configured, you must use nextest's [debugger integration](debuggers-tracers.md) with [CodeLLDB](https://github.com/vadimcn/codelldb). This requires a small amount of one-time setup.

!!! note "Invoking the debugger from VS Code"

    Invoking a debug session from within VS Code will not configure [nextest-specific environment variables](../configuration/env-vars.md#environment-variables-nextest-sets), including those set by [setup scripts](../configuration/setup-scripts.md#environment-variables). If your test depends on any of these variables, you must invoke nextest from the command line.

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
