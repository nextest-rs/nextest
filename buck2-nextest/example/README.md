# A Buck2 example project for `buck2-nextest`

A small Rust project built by [Buck2](https://buck2.build/), used to exercise `buck2-nextest` end to
end against a real Buck2 build.

## Prerequisites

* `buck2` on your `PATH`. The project uses the prelude bundled with the `buck2` binary, so no
  submodule or vendored Starlark is needed -- but it does mean the prelude tracks whichever version
  of buck2 you have installed. Developed against `2026-01-06-6f90b727`.
* `rustc` and a C linker on your `PATH`. `toolchains/BUCK` uses the prelude's demo toolchains, which
  find both there.

## Running the tests

```console
$ ./run.sh
```

That builds `buck2-nextest` and runs:

```console
$ buck2 test -c test.v2_test_executor=/path/to/buck2-nextest //...
```

Anything you pass to `run.sh` goes to `buck2 test`, so all of Buck2's usual selection works, and
arguments after `--` reach nextest:

```console
$ ./run.sh //:demo-lib-test                              # one target
$ ./run.sh //... -- -E 'binary_id(root//:demo-lib-test)' # a nextest filterset
$ ./run.sh //... -- --run-ignored all                    # including the ignored test
$ ./run.sh //... -- -P ci                                # the `ci` profile in .config/nextest.toml
$ ./run.sh //... -- --env DEMO=1                         # an extra variable for test processes
```

Buck2 renders the results itself, the way it does for any test executor. To see nextest's own
reporter output as well, ask Buck2 for the executor's standard error:

```console
$ ./run.sh //... --test-executor-stderr=-
```

## Pointing Buck2 at nextest in your own project

`run.sh` passes the executor on the command line because the path depends on the checkout. In a real
project it goes in `.buckconfig`:

```ini
[test]
  v2_test_executor = /absolute/path/to/buck2-nextest
```

The value must be a bare absolute path -- Buck2 does not accept arguments after it. A
`$BUCK2_BINARY_DIR/` prefix is understood, and resolves next to the `buck2` binary itself.

With that set, `buck2 test //...` uses nextest instead of Buck2's built-in runner.

## How it works

Buck2 launches the executor with two already-connected sockets and speaks gRPC over them: it sends
one test target at a time, then says it has sent them all. `buck2-nextest` asks Buck2 how to run each
target, runs them all with nextest's runner -- keeping per-test process isolation, timeouts, retries,
and leak detection -- and reports each result back as it finishes.

The whole set is collected before anything runs, because nextest builds one test list and runs it.
On a large `buck2 test //...` that means no test starts until analysis has finished.

Only Rust test targets are supported, since nextest lists and runs tests over the libtest protocol.
A target of any other type is rejected by name.

## What the project contains

| Target | What it covers |
| --- | --- |
| `root//:demo` | A library, so the integration test has a dependency to link against. |
| `root//:greeting` | A `genrule` output, so a test depends on something Buck2 built. |
| `root//:demo-lib-test` | Unit tests, including an `#[ignore]`d one for `--run-ignored`. |
| `root//:demo-integration-test` | Reads `DEMO_GREETING_PATH` from its environment, so it fails unless the environment and the generated file both made it through. |

Two test binaries rather than one, so `binary_id()` filtersets have something to select between.
Note that the binary IDs are Buck2 labels: `root//:demo-lib-test`, not a Cargo-style name.

## Running from a spec file instead

Buck2 derives what it sends from each target's `ExternalRunnerTestInfo` provider. The same
information can be written to a file and replayed without a live Buck2, which is useful when working
on `buck2-nextest` itself. [`bxl/nextest.bxl`](bxl/nextest.bxl) produces such a file:

```console
$ buck2 bxl //bxl/nextest.bxl:generate -- --target //...
/path/to/example/buck-out/v2/gen-bxl/.../spec.json
$ cargo run -p buck2-nextest --features spec-file -- run --spec <that path> --project-root .
```

This path is behind the non-default `spec-file` feature, so it is absent from release builds. It also
cannot resolve the argument and environment handles Buck2 may put in a spec, since resolving one
means asking Buck2 -- which is exactly what the gRPC path is for.

Two details in the BXL script are worth knowing if you adapt it:

* `ExternalRunnerTestInfo.command` is a list of `cmd_args`, each of which can expand to several
  arguments. Wrapping the whole list in one `cmd_args` flattens it, so `write_json` writes one string
  per argument instead of a list of lists.
* `write_json(..., with_inputs = True)` ties the artifacts the spec names -- the test binaries, and
  the file `DEMO_GREETING_PATH` points at -- to the JSON file, so ensuring it materializes them too.

## Automated coverage

Two tests in `buck2-nextest/tests/` drive this project:

* [`buck2_test.rs`](../tests/buck2_test.rs) runs `buck2 test` and asserts on Buck2's own output and
  exit code. This is the one that covers how people actually use it.
* [`example.rs`](../tests/example.rs) covers the spec-file path, and checks that what a real Buck2
  prelude emits still parses into what the crate expects. It needs `--features spec-file`.

Both are gated on `buck2` being installed: without it they print a message and pass without checking
anything. There is no skip state in nextest, so a pass there does not by itself mean the example was
exercised -- check the test's output if you need to be sure.

```console
$ cargo nextest run -p buck2-nextest --all-features
```
