---
icon: material/hammer-wrench
---

# Buck2

`buck2-nextest` lets [Buck2](https://buck2.build/) run Rust tests with nextest. It sits where
`cargo-nextest` sits for Cargo: Buck2 decides what to test and builds it, and nextest lists and runs
the tests.

!!! warning "Experimental"

    `buck2-nextest` is not released, and the protocol it speaks is internal to Buck2 with no
    stability guarantee. Expect it to need updating alongside Buck2.

## Setting it up

Point Buck2 at the binary in your project's `.buckconfig`:

```ini
[test]
  v2_test_executor = /absolute/path/to/buck2-nextest
```

The value must be a bare absolute path; Buck2 does not accept arguments after it. A
`$BUCK2_BINARY_DIR/` prefix resolves next to the `buck2` binary itself. For a one-off, pass it on the
command line instead:

```console
$ buck2 test -c test.v2_test_executor=/path/to/buck2-nextest //...
```

## Running tests

```console
$ buck2 test //...
```

Arguments after `--` go to nextest, so nextest's own selection and configuration work as usual:

```console
$ buck2 test //... -- -E 'binary_id(root//:my-test)'
$ buck2 test //... -- --run-ignored all
$ buck2 test //... -- -P ci
$ buck2 test //... -- --env MY_VAR=1
```

Configuration comes from `.config/nextest.toml` at the Buck2 project root, exactly as it comes from
the workspace root under Cargo. See [Configuration](../configuration/index.md).

Because [filtersets](../filtersets/index.md) here have no Cargo package graph to resolve against,
`package()`, `deps()`, and `rdeps()` are unavailable. Everything else works, and binary IDs are Buck2
labels: `binary_id(cell//path/to:target)`.

## Seeing nextest's output

Buck2 renders results itself and captures the executor's output. To see nextest's own reporter as
well:

```console
$ buck2 test //... --test-executor-stderr=-
```

## What it does and does not do

Buck2 hands over one test target at a time over gRPC, then says it has sent them all.
`buck2-nextest` asks Buck2 how to run each target, then runs them with nextest's runner — so
[per-test process isolation](../design/why-process-per-test.md),
[retries](../features/retries.md), [slow-test handling](../features/slow-tests.md), and
[leak detection](../features/leaky-tests.md) all work as they do under Cargo.

Some limits follow from that:

* **Rust targets only.** Nextest lists and runs tests over the libtest protocol, so a target of any
  other test type is rejected by name. In a repository with tests in several languages, scope the
  pattern to Rust targets.
* **Local execution only.** Nextest spawns the test processes, so they do not run on Buck2's remote
  execution.
* **Nothing starts until analysis finishes.** Nextest builds one test list and runs it, so the whole
  set of targets is collected before the first test starts.

## An example

The nextest repository contains a complete, runnable Buck2 project at `buck2-nextest/example/`, with
a README covering both this flow and how to replay a run from a captured spec file.
