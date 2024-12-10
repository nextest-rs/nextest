---
icon: material/pencil
description: Passing in extra arguments to test binaries at runtime, enabling advanced use cases like running tests on the main thread.
---

# Passing in extra arguments

<!-- md:version 0.9.86 -->

!!! warning

    This is an advanced feature, and it can cause your tests to silently stop
    working if used incorrectly. Use with caution.

In some situations, it can be helpful to pass extra arguments into a test binary
at runtime. Nextest supports `run-extra-args` for this purpose.

## Use case: running tests on the main thread

In some environments like macOS, the initial thread created for the process,
also known as the _main thread_ or _UI thread_, is special. Tests in these
environments will often require that they always be run on the main thread (see
[#1959]). This is sometimes called "single-threaded mode", but that is a bit of
a misnomer. What matters is that the main thread of the _test_ is the same as
the main thread of the _process_.

Even though nextest uses a separate process for each test, that isn't a
guarantee that the test will be run on the main thread of the process. In fact,
the standard libtest harness and most other harnesses will create a separate
thread to run the test in, and use the main thread only to manage the test
thread.

As a workaround, some custom test harnesses support passing in arguments to
force the test to run on the main thread. For example, with [libtest-mimic],
passing in `--test-threads=1` as an extra argument forces the test to run on the
main thread.

For more about custom test harnesses and libtest-mimic, see [*Custom test
harnesses*](../design/custom-test-harnesses.md).

!!! note "You must use a custom test harness"

    If your goal is to run tests on the main thread of their corresponding
    processes, `--test-threads=1` by itself will not by itself achieve that. The
    standard libtest harness does not run tests on the main thread with
    `--test-threads=1`: see [Cargo issue #104053].

    You must also use a custom test harness. Our recommendation is
    [libtest-mimic], which follows this behavior with `--test-threads=1`.

    You may also use another custom test harness, though note the [compatibility
    rules]. Your harness may use a different argument for this purpose; in that
    case, replace `--test-threads=1` with the appropriate argument in the
    examples below.

[#1959]: https://github.com/nextest-rs/nextest/discussions/1959
[Cargo issue #104053]: https://github.com/rust-lang/rust/issues/104053
[libtest-mimic]: https://github.com/LukasKalbertodt/libtest-mimic
[compatibility rules]: ../design/custom-test-harnesses.md#manually-implementing-a-test-harness

## Defining extra arguments

Extra arguments are typically defined via [per-test
settings](per-test-overrides.md).


To run all tests in the `gui-tests` package with the `--test-threads=1`
argument:

```toml title="Extra arguments in <code>.config/nextest.toml</code>"
[[profile.default.overrides]]
filter = "package(gui-tests)"
run-extra-args = ["--test-threads=1"]
```

You can also define extra arguments that apply globally to all tests:

```toml
[profile.default]
run-extra-args = ["--test-threads=1"]
```

If libtest-mimic is in use, the above configuration will run tests on the main
thread. (Nextest's CI validates this.)

### Notes

Extra arguments are not passed in at list time, only at runtime. List-time extra
arguments may be supported in the future if there's a compelling use case.

Extra arguments are passed directly to the test binary, and nextest does not
interpret them in any way. Passing in the wrong arguments can cause your tests
to silently stop working.
