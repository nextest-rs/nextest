# Environment variables

This section contains information about the environment variables nextest reads and sets.

## Environment variables nextest reads

Nextest reads some of its command-line options as environment variables. In all cases, passing in a command-line option overrides the respective environment variable.

* `NEXTEST_PROFILE` — [Nextest profile](configuration.md#profiles) to use while running tests.
* `NEXTEST_TEST_THREADS` — Number of tests to run simultaneously.
* `NEXTEST_RETRIES` — Number of times to retry running tests.
* `NEXTEST_HIDE_PROGRESS_BAR` — If set to "1", always hide the progress bar.
* `NEXTEST_FAILURE_OUTPUT` and `NEXTEST_SUCCESS_OUTPUT` — When standard output and standard error are displayed for failing and passing tests, respectively. See [Reporter options](other-options.md#reporter-options) for possible values.
* `NEXTEST_STATUS_LEVEL` — Which test statuses (**PASS**, **FAIL** etc) to display. See [Reporter options](other-options.md#reporter-options) for possible values.
* `NEXTEST_FINAL_STATUS_LEVEL` — Which test statuses (**PASS**, **FAIL** etc) to display at the end of a test run. See [Reporter options](other-options.md#reporter-options) for possible values.
* `NEXTEST_VERBOSE` — Verbose output.

Nextest also reads the following environment variables to emulate Cargo's behavior.

* `CARGO` — Path to the `cargo` binary to use for builds.
* `CARGO_TARGET_DIR` — Location of where to place all generated artifacts, relative to the current working directory.
* `CARGO_TARGET_<triple>_RUNNER` — Support for [target runners](target-runners.md).
* `CARGO_TERM_COLOR` — The default color mode: `always`, `auto` or `never`.

### Cargo-related environment variables nextest reads

Nextest delegates to Cargo for the build, which recognizes a number of environment variables. See [Environment variables Cargo reads](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads) for a full list.

## Environment variables nextest sets

Nextest exposes these environment variables to your tests *at runtime only*. They are not set at build time because cargo-nextest may reuse builds done outside of the nextest environment.

* `NEXTEST` — always set to `"1"`.
* `NEXTEST_RUN_ID` — A UUID corresponding to a particular nextest run. All tests run via a particular invocation of `cargo nextest run` will have the same UUID.
* `NEXTEST_EXECUTION_MODE` — currently, always set to `process-per-test`. More options may be added in the future if nextest gains the ability to run all tests within the same process ([#27]).
* `NEXTEST_BIN_EXE_<name>` — The absolute path to a binary target's executable. This is only set when running an [integration test] or benchmark. The `<name>` is the name of the binary target, exactly as-is. For example, `NEXTEST_BIN_EXE_my-program` for a binary named `my-program`.
  * Binaries are automatically built when the test is built, unless the binary has required features that are not enabled.
  * When [reusing builds](reusing-builds.md) from an archive, this is set to the remapped path within the target directory.
* `NEXTEST_LD_*` and `NEXTEST_DYLD_*` — These replicate the values of any environment variables that start with the prefixes `LD_` or `DYLD_`, such as `LD_PRELOAD` or `DYLD_FALLBACK_LIBRARY_PATH`.

  This is a workaround for [macOS's System Integrity Protection](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html) sanitizing dynamic linker environment variables for processes like the system `bash`, and is particularly relevant for [target runners](target-runners.md). See [this blog post](https://briandfoy.github.io/macos-s-system-integrity-protection-sanitizes-your-environment/) for more about how sanitization works.

  > Note: The `NEXTEST_LD_*` and `NEXTEST_DYLD_*` variables are set on all platforms, not just macOS.

[#27]: https://github.com/nextest-rs/nextest/issues/27
[integration test]: https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests

### Cargo-related environment variables nextest sets

Nextest delegates to Cargo for the build, which controls the environment variables that are set. See [Environment variables Cargo sets for crates](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates) for a full list.

Nextest also sets these environment variables at runtime, matching the behavior of cargo test:

* `CARGO` — Path to the `cargo` binary performing the build.
* `CARGO_MANIFEST_DIR` — The directory containing the manifest of your package. If [`--workspace-remap`](reusing-builds.md#specifying-a-new-location-for-the-workspace) is passed in, this is set to the remapped manifest directory. You can obtain the non-remapped directory using the value of this variable at compile-time, e.g. `env!("CARGO_MANIFEST_DIR")`.
* `CARGO_PKG_VERSION` — The full version of your package.
* `CARGO_PKG_VERSION_MAJOR` — The major version of your package.
* `CARGO_PKG_VERSION_MINOR` — The minor version of your package.
* `CARGO_PKG_VERSION_PATCH` — The patch version of your package.
* `CARGO_PKG_VERSION_PRE` — The pre-release version of your package.
* `CARGO_PKG_AUTHORS` — Colon separated list of authors from the manifest of your package.
* `CARGO_PKG_NAME` — The name of your package.
* `CARGO_PKG_DESCRIPTION` — The description from the manifest of your package.
* `CARGO_PKG_HOMEPAGE` — The home page from the manifest of your package.
* `CARGO_PKG_REPOSITORY` — The repository from the manifest of your package.
* `CARGO_PKG_LICENSE` — The license from the manifest of your package.
* `CARGO_PKG_LICENSE_FILE` — The license file from the manifest of your package.

### Dynamic library paths

Nextest sets the dynamic library path at runtime, similar to [what Cargo does](https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths). This helps with locating shared libraries that are part of the build process. The variable name depends on the platform:

* Windows: `PATH`
* macOS: `DYLD_FALLBACK_LIBRARY_PATH`
* Unix: `LD_LIBRARY_PATH`

Nextest includes the following paths:

* Search paths included from any build script with the [`rustc-link-search` instruction]. Paths outside of the target directory are removed. It is the responsibility of the user running nextest to properly set the environment if additional libraries on the system are needed in the search path.
* The base output directory, such as `target/debug`, and the "deps" directory. This enables support for `dylib` dependencies and rustc compiler plugins.

Nextest currently relies on being invoked as a Cargo subcommand to set the rustc sysroot library path.

[`rustc-link-search` instruction]: https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-search
