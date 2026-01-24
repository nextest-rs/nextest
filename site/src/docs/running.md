---
icon: material/run-fast
description: Running tests with cargo-nextest.
---

# Running tests

To build and run all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest run
```

This will produce output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/run-output.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/run-output.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

In nextest's run output:

- Tests are marked **`PASS`** or **`FAIL`**, and the amount of wall-clock time each test takes is listed within square brackets.
- Tests that take more than a specified amount of time (60 seconds by default) are marked **SLOW**. See [*Slow tests and timeouts*](features/slow-tests.md).
- The part of the test in magenta is the _binary ID_ for a unit test binary (see the [glossary](glossary.md#binary-id)).

- The part after the binary ID is the _test name_, including the module the test is in. The final part of the test name is highlighted in bold blue text.

`cargo nextest run` supports all the options that `cargo test` does. For example, to only execute tests for a package called `my-package`:

```
cargo nextest run -p my-package
```

For a full list of options accepted by `cargo nextest run`, see [Options and arguments](#options-and-arguments) below, or `cargo nextest run --help`.

## Binary IDs

The _binary ID_ uniquely identifies a test binary within a workspace. For more information, see the [glossary](glossary.md#binary-id).

## Selecting tests

To only run tests that match certain names:

```
cargo nextest run <test-name1> <test-name2>...
```

For more information, see [_Selecting tests_](selecting.md).

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. For now, run doctests in a separate step with `cargo test --doc`.

## Failing fast

By default, nextest cancels the test run on encountering a single failure. Tests currently running are run to completion, but new tests are not started.

This behavior can be customized through either the command-line, or through [configuration](configuration/index.md).

### Termination behavior

<!-- md:version 0.9.111 -->

When max-fail is exceeded, nextest supports two termination modes:

- **`wait` (default)**: Nextest stops scheduling new tests but waits for currently running tests to finish naturally. This is the safest option and ensures tests complete their cleanup logic.
- **`immediate`**: Nextest sends termination signals to running tests (respecting the [grace period](features/slow-tests.md#how-nextest-terminates-tests) configured via `slow-timeout.terminate-after`). This is faster but may interrupt tests mid-execution.

### Command-line options

`--max-fail=N[:MODE]` <!-- md:version 0.9.86 --> (`:MODE` syntax added in <!-- md:version 0.9.111 -->)
: Number of tests that can fail before aborting the test run, or `all` to run all tests regardless of the number of failures. Optionally specify termination mode as `:wait` or `:immediate`.

  Examples:

  - `--max-fail=5` - Stop after 5 failures, wait for running tests
  - `--max-fail=1:immediate` - Stop after first failure, terminate running tests immediately
  - `--max-fail=all` - Run all tests regardless of failures

`--no-fail-fast` (<!-- md:version 0.9.92 --> alias `--nff`)
: Do not exit the test run in case a test fails. Most useful for CI scenarios. Equivalent to `--max-fail=all`.

`--fail-fast` (<!-- md:version 0.9.92 --> alias `--ff`)
: Exit the test run on the first failure. This is the default behavior. Equivalent to `--max-fail=1`.

### Configuration

```toml title="Fail-fast behavior in <code>.config/nextest.toml</code>"
[profile.default]
# Exit the test run after N failures, e.g. 5 failures. max-fail in configuration
# is available starting cargo-nextest 0.9.89+.
fail-fast = { max-fail = 5 }

# Exit the test run on the first failure. This is the default behavior.
fail-fast = true
# Or:
fail-fast = { max-fail = 1 }

# Do not exit the test run until all tests complete.
fail-fast = false
# Or:
fail-fast = { max-fail = "all" }

# With termination mode control (available since 0.9.111)
fail-fast = { max-fail = 1, terminate = "wait" }       # Wait for running tests (default)
fail-fast = { max-fail = 1, terminate = "immediate" }  # Terminate running tests immediately
fail-fast = { max-fail = 5, terminate = "immediate" }  # Stop after 5 failures, terminate immediately
```

## Other runner options

`-jN`, `--test-threads=N`
: Number of tests to run simultaneously. Note that this is separate from the number of build jobs to run simultaneously, which is specified by `--build-jobs`.

  `N` can be:

  * `num-cpus` to run as many tests as the amount of [available parallelism] (typically the number of CPU hyperthreads). This is the default.
  * a positive integer (e.g. `8`) to run that many tests simultaneously.
  * a negative integer (e.g. `-2`) to run available parallelism minus that many tests simultaneously. For example, on a machine with 8 CPU hyperthreads, `-2` would run 6 tests simultaneously.

  Tests can be marked as taking up more than one available thread. For more, see [*Heavy tests and `threads-required`*](configuration/threads-required.md).

`--run-ignored=only`
: Run only ignored tests.

`--run-ignored=all`
: Run both ignored and non-ignored tests.

`--no-tests=fail|warn|pass`
: Control behavior when no tests are run. In some cases, e.g. using nextest with [cargo-hack](https://github.com/taiki-e/cargo-hack) to test the powerset of features, it may be useful to set this to `warn` or `pass`.

    If set to `fail` and no tests are found, nextest exits with the advisory code 4 ([`NO_TESTS_RUN`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.NO_TESTS_RUN)).

    <!-- md:version 0.9.85 --> The default is `fail`. In prior versions, the default was `pass` or `warn`.
    
`--debugger=DEBUGGER` <!-- md:version 0.9.113 -->
: Run an individual test with the specified debugger, such as `"rust-gdb --args"`. For more, see [_Debuggers_](integrations/debuggers-tracers.md#debuggers).

`--tracer=TRACER` <!-- md:version 0.9.114 -->
: Run an individual test with the specified syscall tracer, such as `strace`. Similar to `--debugger`, but optimized for non-interactive tracing with null stdin and process groups. For more, see [_System call tracers_](integrations/debuggers-tracers.md#system-call-tracers).

[available parallelism]: https://doc.rust-lang.org/std/thread/fn.available_parallelism.html

## Cargo build options

`--cargo-message-format=FMT` <!-- md:version 0.9.123 -->
: Control how Cargo reports build messages, including forwarding JSON messages to standard out. Accepts the same arguments that [`cargo test --message-format`](https://doc.rust-lang.org/cargo/commands/cargo-test.html#option-cargo-test---message-format) does, and produces results in the same formats.

## Controlling nextest's output

For information about configuring the way nextest displays its human-readable output, see [_Reporting test results_](reporting.md).

## Options and arguments

=== "Summarized output"

    The output of `cargo nextest run -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest run -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest run -h | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest run --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest run --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest run --help | ../scripts/strip-hyperlinks.sh
        ```
