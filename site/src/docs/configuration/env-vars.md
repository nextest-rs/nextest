---
icon: material/bash
description: "Environment variables nextest reads and sets, and whether it's safe to alter them within tests."
---

# Environment variables

This section contains information about the environment variables nextest reads and sets.

## Environment variables nextest reads

Nextest reads some of its command-line options as environment variables. In all cases, passing in a command-line option overrides the respective environment variable.

<div class="compact" markdown>

`NEXTEST_PROFILE`
: [Nextest profile](index.md#profiles) to use while running tests

`NEXTEST_TEST_THREADS`
: Number of tests to run simultaneously

`NEXTEST_RETRIES`
: Number of times to retry running tests

`NEXTEST_HIDE_PROGRESS_BAR`
: If set to `1`, always hide the progress bar

`NEXTEST_STATUS_LEVEL`
: Status level during test runs (see [_Status levels_](../reporting.md#status-levels))

`NEXTEST_FINAL_STATUS_LEVEL`
: Status level at the end of a test run (see [_Status levels_](../reporting.md#status-levels))

`NEXTEST_SUCCESS_OUTPUT`
: Display output for passing tests (see [_Displaying captured test output_](../reporting.md#displaying-captured-test-output))

`NEXTEST_FAILURE_OUTPUT`
: Display output for failing tests (see [_Displaying captured test output_](../reporting.md#displaying-captured-test-output))

`NEXTEST_NO_INPUT_HANDLER`
: <!-- md:version 0.9.86 --> Disable [interactive keyboard handling](../reporting.md#live-output)

`NEXTEST_NO_OUTPUT_INDENT`
: <!-- md:version 0.9.95 --> Disable [indentation of captured test output](../reporting.md#other-options)

`NEXTEST_VERBOSE`
: Verbose output

</div>

Nextest also reads the following environment variables to emulate Cargo's behavior.

<div class="compact" markdown>

`CARGO`
: Path to the `cargo` binary to use for builds

`CARGO_BUILD_TARGET`
: Build target for cross-compilation

`CARGO_TARGET_DIR`
: Where to place generated artifacts

`CARGO_TARGET_<triple>_RUNNER`
: Support for [target runners](../features/target-runners.md)

`CARGO_TERM_COLOR`
: Default color mode: `always`, `auto`, or `never`

`CARGO_TERM_PROGRESS_TERM_INTEGRATION`
: <!-- md:version 0.9.100 --> Report progress to the terminal emulator for display in places like the task bar: `true` or `false`

</div>

### Cargo-related environment variables nextest reads

Nextest delegates to Cargo for the build, which recognizes a number of environment variables. See [Environment variables Cargo reads](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads) for a full list.

## Environment variables nextest sets

Nextest exposes these environment variables to your tests _at runtime only_. They are not set at build time because cargo-nextest may reuse builds done outside of the nextest environment.

`NEXTEST`
: Always `"1"`.

`NEXTEST_RUN_ID`
: A UUID corresponding to a particular nextest run. Set for both tests and [setup scripts](setup-scripts.md).

`NEXTEST_PROFILE` <!-- md:version 0.9.89 -->
: The [nextest profile](index.md#profiles) in use.

`NEXTEST_VERSION` <!-- md:version 0.9.130 -->
: The current nextest version as a semver string (e.g. `"0.9.120"`). Set for both tests and [setup scripts](setup-scripts.md).

`NEXTEST_REQUIRED_VERSION` <!-- md:version 0.9.130 -->
: The minimum required nextest version from the repository's [`nextest-version`](index.md#minimum-nextest-version) configuration, as a semver string. If no required version is configured, this is `"none"`. Set for both tests and [setup scripts](setup-scripts.md).

`NEXTEST_RECOMMENDED_VERSION` <!-- md:version 0.9.130 -->
: The minimum recommended nextest version from the repository's [`nextest-version`](index.md#minimum-nextest-version) configuration, as a semver string. If no recommended version is configured, this is `"none"`. Set for both tests and [setup scripts](setup-scripts.md).

`NEXTEST_EXECUTION_MODE`
: Currently, always `process-per-test`. More options may be added in the future if nextest gains the ability to run multiple tests within the same process ([#27]).

`NEXTEST_BINARY_ID` <!-- md:version 0.9.116 -->
: The [binary ID](../glossary.md#binary-id) corresponding to the test.

`NEXTEST_TEST_NAME` <!-- md:version 0.9.116 -->
: The name of the test.

`NEXTEST_ATTEMPT` <!-- md:version 0.9.116 -->
: The 1-indexed attempt number for the test. In the default case where no [retries](../features/retries.md) are configured for the test, this is always `"1"`.

`NEXTEST_TOTAL_ATTEMPTS` <!-- md:version 0.9.116 -->
: The total number of attempts configured for the test. In the default case where no [retries](../features/retries.md) are configured for the test, this is always `"1"`.

`NEXTEST_ATTEMPT_ID` <!-- md:version 0.9.116 -->
: A [unique identifier](../glossary.md#attempt-id) for this test attempt.

  !!! note "Quote this environment variable within shells"

      `NEXTEST_ATTEMPT_ID` contains an embedded `$`. If accessing `NEXTEST_ATTEMPT_ID` within a shell, be sure to put it in quotes. (Quoting environment variables is always good practice; a linter like [shellcheck](https://www.shellcheck.net/) can warn you about this.)

`NEXTEST_STRESS_CURRENT` <!-- md:version 0.9.116 -->
: For [stress tests](../features/stress-tests.md), the 0-indexed stress index. If not a stress run, this is set to `none`.

`NEXTEST_STRESS_TOTAL` <!-- md:version 0.9.116 -->
: For [stress tests](../features/stress-tests.md), the total number of stress runs that will be performed. If the total number is not unknown, this is set to `unknown`. If not a stress run, this is set to `none`.

`NEXTEST_TEST_GROUP` <!-- md:version 0.9.90 -->
: The [test group](test-groups.md) the test is in, or `"@global"` if the test is not in any groups.

`NEXTEST_TEST_GLOBAL_SLOT` <!-- md:version 0.9.90 -->
: The [global slot number](../glossary.md#slot-numbers). Global slot numbers are non-negative integers starting from 0 that are unique within the run for the lifetime of the test, but are reused after the test finishes.

`NEXTEST_TEST_GROUP_SLOT` <!-- md:version 0.9.90 -->
: If the test is in a group, the [group slot number](../glossary.md#slot-numbers). Group slot numbers are non-negative integers that are unique within the test group for the lifetime of the test, but are reused after the test finishes.

    If the test is not in any groups, this is `"none"`.

`NEXTEST_TEST_THREADS` <!-- md:version 0.9.130 -->
: The number of [test threads](../features/test-threads.md) configured for this run. This is the computed value after considering the profile, command-line overrides, and capture strategy. Set for both tests and [setup scripts](setup-scripts.md).

`NEXTEST_WORKSPACE_ROOT` <!-- md:version 0.9.130 -->
: The absolute path to the workspace root. Set for both tests and [setup scripts](setup-scripts.md).

    When [`--workspace-remap`](../ci-features/archiving.md#specifying-a-new-location-for-the-source-code) is passed in, this is set to the remapped workspace root.

`NEXTEST_BIN_EXE_<name>`
: The absolute path to a binary target's executable. This is only set when running an [integration test] or benchmark. The `<name>` is the name of the binary target, exactly as-is. For example, `NEXTEST_BIN_EXE_my-program` for a binary named `my-program`.

    Binaries are automatically built when the test is built, unless the binary has required features that are not enabled.

    When [reusing builds](../ci-features/archiving.md) from an archive, this is set to the remapped path within the target directory.
    
    <!-- md:version 0.9.113 --> Because some shells and [debuggers](../integrations/debuggers-tracers.md) drop [environment variables with hyphens in their names](https://unix.stackexchange.com/a/23714), nextest also sets an alternative form of these variables where hyphens in the name are replaced with underscores. For example, for a binary named `my-program`, the environment variable `NEXTEST_BIN_EXE_my_program` is also set to the absolute path of the executable.

`NEXTEST_LD_*` and `NEXTEST_DYLD_*`
: Replicates the values of any environment variables that start with the prefixes `LD_` or `DYLD_`, such as `LD_PRELOAD` or `DYLD_FALLBACK_LIBRARY_PATH`.

    This is a workaround for [macOS's System Integrity Protection](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html) environment sanitization. For more, see [_Dynamic linker environment variables_](../installation/macos.md#dynamic-linker-environment-variables).

    For consistency, `NEXTEST_LD_*` and `NEXTEST_DYLD_*` are exported on all platforms, not just macOS.

[#27]: https://github.com/nextest-rs/nextest/issues/27
[integration test]: https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests

### Cargo-related environment variables nextest sets

Nextest delegates to Cargo for the build, which controls the environment variables that are set. See [Environment variables Cargo sets for crates](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates) for a full list.

Nextest also sets these environment variables at runtime, matching the behavior of `cargo test`:

`CARGO`
: Path to the `cargo` binary performing the build.

    This is set by Cargo, not nextest, so if you invoke nextest with `cargo-nextest nextest run` it will not be set.

`CARGO_MANIFEST_DIR`
: The directory containing the manifest of the test's package.

    If [`--workspace-remap`](../ci-features/archiving.md#specifying-a-new-location-for-the-source-code) is passed in, this is set to the remapped manifest directory. You can obtain the non-remapped directory using the value of this variable at compile-time, e.g. `env!("CARGO_MANIFEST_DIR")`.

`CARGO_PKG_VERSION`
: The full version of the test's package.

`CARGO_PKG_VERSION_MAJOR`
: The major version of the test's package.

`CARGO_PKG_VERSION_MINOR`
: The minor version of the test's package.

`CARGO_PKG_VERSION_PATCH`
: The patch version of the test's package.

`CARGO_PKG_VERSION_PRE`
: The pre-release version of the test's package.

`CARGO_PKG_AUTHORS`
: Colon-separated list of authors from the manifest of the test's package.

`CARGO_PKG_NAME`
: The name of the test's package.

`CARGO_PKG_DESCRIPTION`
: The description from the manifest of the test's package.

`CARGO_PKG_HOMEPAGE`
: The home page from the manifest of the test's package.

`CARGO_PKG_REPOSITORY`
: The repository from the manifest of the test's package.

`CARGO_PKG_LICENSE`
: The license from the manifest of the test's package.

`CARGO_PKG_LICENSE_FILE`
: The license file from the manifest of the test's package.

`OUT_DIR`
: The path to the test's build script output directory.

    Only set if the crate has a build script.

Additionally, the following environment variables are also set:

* Variables defined by [the `[env]` section of `.cargo/config.toml`](https://doc.rust-lang.org/cargo/reference/config.html#env).
* <!-- md:version 0.9.82 --> Variables specified in the build script via [`cargo::rustc-env`](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-env). However, note that [`cargo` discourages using these variables at runtime](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-env).

### Dynamic library paths

Nextest sets the dynamic library path at runtime, similar to [what Cargo does](https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths). This helps with locating shared libraries that are part of the build process. The variable name depends on the platform:

- Windows: `PATH`
- macOS: `DYLD_FALLBACK_LIBRARY_PATH`
- Unix: `LD_LIBRARY_PATH`

Nextest includes the following paths:

- Search paths included from any build script with the [`rustc-link-search` instruction](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-search). Paths outside of the target directory are removed. If additional libraries on the system are needed in the search path, consider using a [setup script <!-- md:flag experimental -->](setup-scripts.md) to configure the environment.
- The base output directory, such as `target/debug`, and the "deps" directory. This enables support for `dylib` dependencies and rustc compiler plugins.
- <!-- md:version 0.9.72 --> The rustc sysroot library path, to enable proc-macro tests and binaries compiled with `-C prefer-dynamic` to work.

## Altering the environment within tests

*(The information in this section is true as of Rust 1.84. We'll update this section in the unlikely event that changes occur.)*

Many tests will want to alter their own environment variables[^altering-env]. The [`std::env::set_var`](https://doc.rust-lang.org/std/env/fn.set_var.html) and [`std::env::remove_var`](https://doc.rust-lang.org/std/env/fn.remove_var.html) functions are unsafe in general, and can lead to races if another thread is accessing (writing to or reading from) the environment at the same time.

In particular, the safety contract for `set_var` and `remove_var` requires at least one of these three statements to be true:

1. The program is single-threaded.
2. The underlying implementations of `set_var` and `remove_var` are thread-safe. This is true on Windows, and on [some Unix platforms like illumos][illumos-env]. Notably, **this is not true on Linux**.
3. While these functions are being called, no code is simultaneously accessing the environment other than through Rust's `std::env` module. (For example, no C code is accessing the environment.)

Nextest runs [each test in its own process](../design/why-process-per-test.md). Does that make it safe to alter the environment in tests? The answer is generally **yes**. Let's look at this case-by-case.

!!! warning "This does not apply to `cargo test`"

    The discussion below only applies to tests run via nextest's process-per-test model.

    `cargo test` runs many tests in the same process, so one has to be much more cautious. A more detailed analysis of `cargo test` is out of scope for this discussion.

### A standard test

When nextest starts a standard test annotated with `#[test]` (also known as a *libtest test*), the resulting process has two threads:

- The main thread, used to monitor the test.
- The test thread.

So the test process is multithreaded, making statement 1 above false.

But with current versions of Rust, while the test thread is running, the main thread does not read or write environment variables.

This is unlikely to ever change—there is no reason for the main thread to ever access the environment, and doing so would likely break many existing tests. So statement 3 is true.

In other words, if the test itself hasn't created any threads yet, it is de facto **safe** to alter the environment.

!!! tip "Alter the environment at the beginning of tests"

    Practically speaking, it is best to call `set_var` and `remove_var` at the very beginning of tests, before there's the chance for any threads to be created.

### A test under a custom test harness

[Custom test harnesses](../design/custom-test-harnesses.md) may run arbitrary code before test execution, so it's hard to make a general statement.

With the recommended [libtest-mimic harness](https://github.com/LukasKalbertodt/libtest-mimic), the environment is not accessed while tests are running, so statement 3 above is true.

In addition, libtest-mimic can be forced to not create any threads by [passing in extra arguments](extra-args.md). Not creating a thread for the test makes statements 1 and 3 above both true.

This means that if you're using libtest-mimic directly, with or without extra arguments, it is **safe** to alter the environment.

Harnesses written on top of libtest-mimic might create their own threads, though. You're encouraged to analyze the harnesses you're using.

!!! info "The `datatest-stable` harness"

    The [`datatest-stable` harness](https://docs.rs/datatest-stable) maintained by the nextest organization does not spin up any threads, so it is safe to alter the environment at the beginning of tests.

### Tests annotated with custom proc-macros

Some tests use a procedural macro that generates a wrapper `#[test]` function. One common example is [the `#[tokio::test]` macro](https://docs.rs/tokio/latest/tokio/attr.test.html).

Like with custom test harnesses, these wrappers can run arbitrary code before test execution, so a general statement cannot be made. One would have to analyze the generated code and the runtime to make a complete determination.

It's worth looking at `#[tokio::test]` as an example:

* By default, the macro runs Tokio in single-threaded mode. This case is equivalent to [a standard test](#a-standard-test), so it is **safe** to alter the environment.

* If `flavor = "multi_thread"` is specified, `#[tokio::test]` does create at least one worker thread.

  However, the Tokio runtime does not access the environment after the worker thread pool is created, and any other worker threads are dormant at the beginning of the test.

  So statement 3 remains true, and it is **safe** to alter the environment at the beginning of tests.

### For test harness and proc-macro maintainers

If you maintain a custom test harness or proc-macro, it is recommended that you document environment safety as a property.

It is safe for nextest users to alter the environment at the beginning of tests, as long as you can guarantee that created threads, if any, won't access the environment concurrently.

(This may not be true for `cargo test` users due to its shared-process model. Be sure to make this clear in your documentation.)

[illumos-env]: https://github.com/illumos/illumos-gate/blob/3da9c6ab7ef58a10539f1228227de9c34e1baf33/usr/src/lib/libc/port/gen/getenv.c#L47-L75
[^altering-env]: In general, altering the current process's environment variables is fraught with peril since it touches shared mutable state, but tests are a reasonable use case for this functionality. The cargo-nextest binary never alters its environment variables, but many of nextest's own tests do so.
