---
icon: material/list-box
toc_depth: 1
---

# Changelog

This page documents new features and bugfixes for cargo-nextest. Please see the [stability
policy](https://nexte.st/docs/stability/) for how versioning works with cargo-nextest.

## [0.9.85-rc.1] - 2024-11-24

### Changed

When no tests are run, the default behavior now is to exit with code 4
(`NO_TESTS_RUN`). This is a behavior change, as documented in [#1646].

[#1646]: https://github.com/nextest-rs/nextest/discussions/1646

### Added

SHA-256 and BLAKE2 checksum files are now published for each release.

## [0.9.84] - 2024-11-15

### Fixed

- Fixed a rare crash during test run cancellation ([#1876]).

[#1876]: https://github.com/nextest-rs/nextest/pull/1876

## [0.9.83] - 2024-11-15

### Added

- Per-platform default filters are now supported via overrides. For example, to
  skip over tests with the substring `unix_tests` by default on Windows, add
  this to `.config/nextest.toml`:

  ```toml
  [[profile.default.overrides]]
  platform = "cfg(windows)"
  default-filter = "not test(unix_tests)"
  ```

- `cargo nextest run --build-jobs` now accepts negative numbers as arguments,
  similar to other commands like `cargo nextest run --test-threads` and `cargo
  build`. Negative numbers mean "use all available cores except for this many".

  Thanks to [mattsse](https://github.com/mattsse) for your first contribution!

### Internal improvements

- Internal targets updated to Rust 1.82.
- Runner logic refactored to handle upcoming features.

## [0.9.82] - 2024-10-28

### Added

- For crates with a build script, nextest now reads their output and sets environment variables from
  within them for tests. This matches `cargo test`'s behavior. However, note that this usage [is
  discouraged by Cargo](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-env).

  Thanks to [chrjabs](https://github.com/chrjabs) for your first contribution!

- On Unix platforms, nextest now also intercepts the `SIGQUIT` signal, in addition to the existing
  `SIGINT`, `SIGTERM`, etc. More signals will be added to this list as makes sense.

### Internal improvements

- Switch internal logging over to the fantastic `tracing` library. Nextest doesn't do much
  structured logging or event/span logging yet, but tracing provides a great foundation to add that
  in the future.
- Internal dependency updates.

## [0.9.81] - 2024-10-06

### Fixed

Fixed semantics of `--exact` to match Rust's libtest: `--exact` now makes it so that all filters passed in after `--` (including `--skip` filters) are matched exactly.

## [0.9.80] - 2024-10-05

### Added

Support for `--skip` and `--exact` as emulated test binary arguments. The semantics match those of `libtest` binaries.

For example, to run all tests other than those matching the substring `slow_tests`:

```
cargo nextest run -- --skip slow_tests
```

To run all tests matching either the substring `my_test` or the exact string `exact_test`:

```
cargo nextest run -- my_test --exact exact_test
```

Thanks to [svix-jplatte](https://github.com/svix-jplatte) for your first contribution!

## [0.9.79] - 2024-10-02

### Added

- Expanded version information: `cargo nextest -V` now shows commit and date information similar to `rustc` and `cargo`, and `cargo nextest --version` shows this information in long form.

### Fixed

- Nextest will now enable colors by default in more situations, particularly over SSH connections. For more information, see [this issue](https://github.com/zkat/supports-color/pull/19).
- Fixed a case of `cargo metadata` parsing with renamed packages ([#1746]).

[#1746]: https://github.com/nextest-rs/nextest/issues/1746

## [0.9.78] - 2024-09-05

### Added

For failing tests, if nextest finds text matching patterns that indicate failure, such as "thread
panicked at", it now highlights those lines (if color is enabled).

Rust's libtest doesn't provide structured output for this, so nextest uses heuristics. These
heuristics will be tweaked over time; to see what nextest would highlight, run `cargo nextest debug
extract highlight`, and provide either `--stdout` and `--stderr`, or `--combined` if stdout and
stderr are combined.

## [0.9.77] - 2024-08-28

### Changed

A couple of UI changes:

- `default-set` is now `default-filter`.
- `--bound=all` is now `--ignore-default-filter`.

Sorry about the breakage here -- this should be the last of the changes.

## [0.9.76] - 2024-08-25

### Added

- A new `--bound=all` option disables the default set on the command line.
- `--run-ignored ignored-only` has been shortened to `--run-ignored only`. (The old name still works
  as an alias.)

### Fixed

- Documentation links updated to point to the new website.

### Changed

- Previously, passing in any `-E` options would disable the default set. However in practice that
  was found to be too confusing, and this behavior has been removed. Instead, use `--bound`.

  This is technically a breaking change, but default sets aren't in wide use yet so this should have
  minimal impact.

## [0.9.75] - 2024-08-23

### Added

- Support for default sets of tests to run via the `default-set` configuration. See [_Running a
  subset of tests by default_](https://nexte.st/docs/running#running-a-subset-of-tests-by-default)
  for more information.
- A new `--no-tests` option controls the behavior of nextest when no tests are run. The possible
  values are `pass`, `warn` and `fail`. Per the behavior changed described in [discussion
  #1646](https://github.com/nextest-rs/nextest/discussions/1646), the current default is `warn`, and
  it will change to `fail` in the future.

## [0.9.74] - 2024-08-18

### Added

Warnings are now printed in the following cases:

- If some tests are not run, e.g. due to `--fail-fast`.
- If no tests are run.

### Changed

- Updated MSRV for compiling nextest to Rust 1.75.

### Upcoming behavior changes

If no tests are run, nextest will start exiting with the advisory code **4** in versions released after 2024-11-18. See [discussion #1646](https://github.com/nextest-rs/nextest/discussions/1646) for more.

## [0.9.73] - 2024-08-18

(This version was not released due to a publishing issue.)

## [0.9.72] - 2024-05-23

### Fixed

Previously, nextest would be unable to run proc-macro tests in some circumstances:

- On Windows, with rustup 1.27.2 and above
- On all platforms, if cargo is used without rustup, or if the `cargo-nextest` binary is invoked directly

With this release, proc-macros tests now work in all circumstances. This is done by nextest detecting Rust libdirs for the host and target platforms, and adding them to the library path automatically.

(There's also the less-common case of test binaries compiled with `-C prefer-dynamic`. These
situations now also work.)

See [#267](https://github.com/nextest-rs/nextest/issues/267) and [#1493](https://github.com/nextest-rs/nextest/issues/1493) for more details.

Thanks to [06393993](https://github.com/06393993) for your first contribution!

### Changed

As part of the above fix, libstd is now included in all archives. This makes archives around 4MB bigger, or around 8MB in cross-compilation scenarios. (It is possible to address this via config knobs -- if this is particularly bothersome to you, please post in [#1515](https://github.com/nextest-rs/nextest/issues/1515).)

## [0.9.71] - 2024-05-23

(This version was not published due to a release issue.)

## [0.9.70] - 2024-04-24

### Added

- Archives can now include extra paths in them. For example:

  ```toml
  [profile.default]
  archive.include = [
      { path = "my-extra-path", relative-to = "target" }
  ]
  ```

  For more information, see [_Adding extra files to an archive_](https://nexte.st/book/reusing-builds#adding-extra-files-to-an-archive).

  Thanks to [@rukai](https://github.com/rukai) for your first contribution!

- You can now pass in `--cargo-quiet` twice to completely discard standard error for the Cargo
  commands run by nextest. This is equivalent to `2> /dev/null`.

### Fixed

- The initial `cargo metadata` execution now passes in `--frozen`, `--locked`, `--offline` and `--quiet` if the corresponding flags are passed into nextest.
- Previously, `NEXTEST_HIDE_PROGRESS_BAR=1` did not work (only `NEXTEST_HIDE_PROGRESS_BAR=true`
  did). Now both `1` and `true` work.

### Changed

- Updated MSRV for compiling nextest to Rust 1.74.

## [0.9.69] - 2024-04-24

This release wasn't published due to a build issue.

## [0.9.68] - 2024-03-16

This is a maintenance release with many internal improvements, and preparation for future features.

### Changed

- Nextest binaries now ship with symbols, producing better stack traces. This is aligned with the behavior. See [issue #1345](https://github.com/nextest-rs/nextest/issues/1345) for more information.

- Thanks to recent improvements, Miri is now significantly less taxing. As a result, [nextest with Miri](https://nexte.st/book/miri) has been changed to use all threads by default. You can restore the old Miri behavior (run one test at a time) with `-j1`, or by setting in `.config/nextest.toml`:

  ```toml
  [profile.default-miri]
  test-threads = 1
  ```

  Rules for [heavy tests](https://nexte.st/book/threads-required) and [test groups](https://nexte.st/book/test-groups) will continue to be followed with Miri.

  Thanks to [Ben Kimock](https://github.com/saethlin) for driving the Miri improvements and updating nextest!

### Misc

- The [filter expression](https://nexte.st/book/filter-expressions) parser now uses [winnow](https://docs.rs/winnow).
- [get.nexte.st](https://get.nexte.st) now uses Cloudflare rather than Netlify for hosting. See [this discussion](https://github.com/nextest-rs/nextest/discussions/1383) for more.

## [0.9.67] - 2024-01-09

### Added

- More work on [machine-readable output for test runs](https://nexte.st/book/run-machine-readable): for failing tests, output is now included under the `stdout` field. This was a large effort which required figuring out how to combine stdout and stderr into the same buffer. Thanks again [Jake](https://github.com/Jake-Shadle) for your contribution!

### Fixed

- On SIGTERM and SIGHUP, outputs for cancelled and failed tests are now displayed. (Output is still hidden on SIGINT to match typical user expectations.)

## [0.9.66] - 2023-12-10

### Added

#### Experimental feature: machine-readable output for test runs

Nextest now has experimental support for machine-readable output during `cargo nextest run` invocations ([#1086]), in a format similar to `cargo test`'s libtest JSON output. For more information, see [the documentation](https://nexte.st/book/run-machine-readable).

Thanks [Jake Shadle](https://github.com/Jake-Shadle) for your contribution!

[#1086]: https://github.com/nextest-rs/nextest/pull/1086

#### `OUT_DIR` support

Improvements to [build script `OUT_DIR`](https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script)
support:

- Matching the behavior of `cargo test`, nextest now sets the `OUT_DIR` environment variable at
  runtime if there's a corresponding build script.

- While [creating archives](https://nexte.st/book/reusing-builds), nextest now archives `OUT_DIR`s if:

  - The build script is for a crate in the workspace, and
  - There's at least one test binary for that crate.

  This is so that the `OUT_DIR` environment variable continues to be relevant for test runs out of
  archives.

  Currently, `OUT_DIR`s are only archived one level deep to avoid bloating archives too much. In the
  future, we may add configuration to archive more or less of the output directory. If you have a
  use case that would benefit from this, please [file an
  issue](https://github.com/nextest-rs/nextest/issues/new).

### Misc

- The `.crate` files uploaded to crates.io now contain the `LICENSE-APACHE` and `LICENSE-MIT` license files. Thanks [@musicinmybrain](https://github.com/musicinmybrain) for your first contribution!

## [0.9.65] - 2023-12-10

This version was not released due to a build issue on illumos.

## [0.9.64] - 2023-12-03

### Added

- Stabilized and documented [the binary ID format](https://nexte.st/book/running#binary-ids).
- Support for glob matchers in [filter expressions](https://nexte.st/book/filter-expressions). For example, `package(foo*)` will match all tests whose names start with `foo`.
- A new `binary_id()` predicate matches against the binary ID.

### Changed

- Unit tests in proc-macro crates now have a binary ID that consists of just the crate name, similar to unit tests in normal crates.

- The default string matcher for the following predicates has changed from _equality_ to _glob_:

  - Package-related matchers: `package()`, `deps()`, and `rdeps()`
  - Binary-related matchers: `binary()`

  The new `binary_id()` predicate also uses the glob matcher by default.

### Fixed

- Fixed a regression with some Cargo nightly-only features: see [guppy-rs/guppy#174](https://github.com/guppy-rs/guppy/pull/174) for more details.

## [0.9.63] - 2023-11-17

### Fixed

- Fixed regressions under some Cargo edge cases, e.g. [guppy-rs/guppy#157](https://github.com/guppy-rs/guppy/issues/157).

## [0.9.62] - 2023-11-14

### Added

- If you create a symlink manually named `cargo-ntr` and pointing to `cargo-nextest`, you can now
  shorten `cargo nextest run` to `cargo ntr`. In the future, this symlink will be automatically
  created at install time.

### Changed

- Deprecated test name filters passed in before `--`. For example, `cargo nextest run my_test` is deprecated; use `cargo nextest run -- my_test` instead. See [#1109] for motivation and more information.
- `x86_64-unknown-linux-gnu` builds are now performed using `cargo zigbuild`. The minimum glibc version remains unchanged at 2.27. Please [file a bug report](https://github.com/nextest-rs/nextest/issues/new) if you encounter any issues with this change.

### Fixed

- Fixed crashes under some Cargo edge cases, e.g. [#1090].

[#1109]: https://github.com/nextest-rs/nextest/issues/1109
[#1090]: https://github.com/nextest-rs/nextest/issues/1090

## [0.9.61] - 2023-10-22

### Changed

- The grace period in `slow-timeout.grace-period` now applies to terminations as well. Thanks [@kallisti-dev](https://github.com/kallisti-dev) for your first contribution!

### Fixed

- JUnit output now strips ANSI escapes as well. Thanks [@MaienM](https://github.com/MaienM) for your first contribution!

## [0.9.60] - 2023-10-22

This version was not released due to a packaging issue.

## [0.9.59] - 2023-09-27

### Added

- Experimental support for [setup scripts](https://nexte.st/book/setup-scripts). Please try them out, and provide feedback in the [tracking issue](https://github.com/nextest-rs/nextest/issues/978)!
- Support for newer error messages generated by Rust 1.73.

### Fixed

- `deps()` and `rdeps()` predicates in [per-test overrides](https://nexte.st/book/per-test-overrides) were previously not working correctly. With this version they now work.

## [0.9.58] - 2023-09-20

### Added

- Per-test overrides [can now be filtered separately](https://nexte.st/book/per-test-overrides#specifying-platforms) by host and target platforms.
- New `--cargo-quiet` and `--cargo-verbose` options to control Cargo's quiet and verbose output options. Thanks [Oliver Tale-Yazdi](https://github.com/ggwpez) for your first contribution!

### Fixed

- Improved color support by pulling in [zkat/supports-color#14](https://github.com/zkat/supports-color/pull/14). Now nextest should produce color more often when invoked over SSH.

## [0.9.57] - 2023-08-02

### Fixed

- Fixed case when `.config/nextest.toml` isn't present ([#926](https://github.com/nextest-rs/nextest/issues/926)).

## [0.9.56] - 2023-08-02

### Fixed

- `nextest-version` is now parsed in a separate pass. This means that error reporting in case
  there's an incompatible config is now better.

## [0.9.55] - 2023-07-29

### Added

- Support for Cargo's `--timings` option ([#903](https://github.com/nextest-rs/nextest/issues/903)).
- Support for required and recommended versions, via a new `nextest-version` top-level configuration option. See [Minimum nextest versions](https://nexte.st/book/minimum-versions) for more.

### Fixed

- Detect tests if debug level `line-tables-only` is passed in ([#910](https://github.com/nextest-rs/nextest/issues/910)).

## [0.9.54] - 2023-06-25

### Added

#### Custom targets

- Nextest now supports custom targets specified via `--target`, `CARGO_BUILD_TARGET`, or configuration. See the Rust Embedonomicon for [how to create a custom target](https://docs.rust-embedded.org/embedonomicon/custom-target.html).
- For [per-test overrides](https://nexte.st/book/per-test-overrides), platform filters now support custom target triples.

## [0.9.53] - 2023-05-15

### Added

- Filter expressions in TOML files can now be specified as multiline TOML strings. For example:

```toml
[[profile.default.overrides]]
filter = '''
  test(my_test)
  | package(my-package)
'''
# ...
```

### Changed

- `show-config test-groups` now shows a clean representation of filter expressions, to enable
  printing out multiline expressions neatly.

## [0.9.52] - 2023-05-04

### Fixed

- Updated dependencies to resolve a build issue on Android ([#862]).

[#862]: https://github.com/nextest-rs/nextest/issues/862

## [0.9.51] - 2023-03-19

### Changed

- The definition of `threads-required` has changed slightly. Previously, it was possible for global and group concurrency limits to be exceeded in some circumstances. Now, concurrency limits are never exceeded. This enables some new use cases, such as being able to declare that a test is mutually exclusive with all other tests globally.

## [0.9.50] - 2023-03-13

### Added

- `cargo nextest r` added as a shortcut for `cargo nextest run`.

### Fixed

- Switched to using OpenSSL on RISC-V, since ring isn't available on that platform.

## [0.9.49] - 2023-01-13

### Added

- New configuration settings added to [JUnit reports](https://nexte.st/book/junit): `junit.store-success-output` (defaults to false) and `junit.store-failure-output` (defaults to true) control whether output for passing and failing tests should be stored in the JUnit report.
- The following configuration options can now be specified as [per-test overrides](https://nexte.st/book/per-test-overrides):
  - `success-output` and `failure-output`.
  - `junit.store-success-output` and `junit.store-failure-output`.

## [0.9.48] - 2023-01-02

### Added

- You can now mark certain _groups_ of tests to be run with a limited amount of concurrency within the group. This can be used to run tests within a group serially, similar to the [`serial_test` crate](https://crates.io/crates/serial_test).

  For more about test groups, see [Test groups and mutual exclusion](https://nexte.st/book/test-groups).

- A new `show-config test-groups` command shows test groups currently in effect. (`show-config` will be broadened to show other kinds of configuration in future releases.)

- Nextest now warns you if you've defined a profile in the `default-` namespace that isn't already known. Any profile names starting with `default-` are reserved for future use.

  Thanks [Marcelo Nicolas Gomez Rivera](https://github.com/nextest-rs/nextest/pull/747) for your first contribution!

### Changed

- On Unix platforms, nextest now uses a new _double-spawn_ test execution mode. This mode resolves some race conditions around signal handling without an apparent performance cost.

  This mode is not expected to cause any issues. However, if it does, you can turn it off by setting `NEXTEST_DOUBLE_SPAWN=0` in your environment. (Please [report an issue](https://github.com/nextest-rs/nextest/issues/new) if it does!)

- MSRV updated to Rust 1.64.

## [0.9.47] - 2022-12-10

### Fixed

- `cargo nextest run -E 'deps(foo)` queries now work again. Thanks [Simon Paitrault](https://github.com/Freyskeyd) for your first contribution!

## [0.9.46] - 2022-12-10

This version was not published due to a packaging issue.

## [0.9.45] - 2022-12-04

### Added

- Support for listing and running tests in examples with the `--examples` and `--example <EXAMPLE>` command-line arguments. Thanks [Jed Brown](https://github.com/jedbrown) for your first contribution!
- [Pre-built binaries](https://nexte.st/book/pre-built-binaries) are now available for FreeBSD and illumos. Due to GitHub Actions limitations, nextest is not tested on these platforms and might be broken.

## [0.9.44] - 2022-11-23

### Added

#### Double-spawning test processes

On Unix platforms, a new experimental "double-spawn" approach to running test binaries has been added. With the double-spawn approach, when listing or running tests, nextest will no longer spawn test processes directly. Instead, nextest will first spawn a copy of itself, which will do some initial setup work and then `exec` the test process.

The double-spawn approach is currently disabled by default. It can be enabled by setting `NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN=1` in your environment.

The double-spawn approach will soon be enabled the default.

#### Pausing and resuming test runs

Nextest now has initial support for handling SIGTSTP (Ctrl-Z) and SIGCONT (`fg`). On SIGTSTP (e.g. when Ctrl-Z is pressed), all running tests and timers are paused, and nextest is suspended. On SIGCONT (e.g. when `fg` is run), tests and timers are resumed.

Note that, by default, pressing Ctrl-Z in the middle of a test run can lead to [nextest runs hanging sometimes](https://inbox.sourceware.org/libc-help/87tu64w33v.fsf@oldenburg.str.redhat.com/T/#m5e40bfa2378e9df29e077addf1aa72b191902b86). These nondeterministic hangs will not happen if both of the following are true:

- Nextest is built with Rust 1.66 (currently in beta) or above. Rust 1.66 contains [a required fix to upstream Rust](https://github.com/rust-lang/rust/pull/101077).

  Note that the [pre-built binaries](https://nexte.st/book/pre-built-binaries) for this version are built with beta Rust to pick this fix up.

- The double-spawn approach is enabled (see above) with `NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN=1`.

**Call for testing:** Please try out the double-spawn approach by setting `NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN=1` in your environment. It has been extensively tested and should not cause any breakages, but if it does, please [report an issue](https://github.com/nextest-rs/nextest/issues/new). Thank you!

### Fixed

- Fixed an issue with nextest hanging on Windows with spawned processes that outlive the test ([#656](https://github.com/nextest-rs/nextest/issues/656)). Thanks to [Chip Senkbeil](https://github.com/chipsenkbeil) for reporting it and providing a minimal example!

## [0.9.43] - 2022-11-04

Nextest is now built with Rust 1.65. This version of Rust is the first one to spawn processes using [`posix_spawn`](https://pubs.opengroup.org/onlinepubs/007904975/functions/posix_spawn.html) rather than `fork`/`exec` on macOS, which should lead to performance benefits in some cases.

For example, on an M1 Mac Mini, with the [clap repository](https://github.com/clap-rs/clap) at `520145e`, and the command `cargo nextest run -E 'not (test(ui_tests) + test(example_tests))'`:

- **Before:** 0.636 seconds
- **After:** 0.284 seconds (2.23x faster)

This is a best-case scenario; tests that take longer to run will generally benefit less.

### Added

- The [`threads-required` configuration](https://nexte.st/book/threads-required) now supports the
  values "num-cpus", for the total number of logical CPUs available, and "num-test-threads", for the
  number of test threads nextest is running with.
- Nextest now prints a warning if a configuration setting is unknown.

### Fixed

- Configuring `retries = 0` now works correctly. Thanks [xxchan](https://github.com/xxchan) for your first contribution!

## [0.9.42] - 2022-11-01

### Added

- Added a new [`threads-required` configuration](https://nexte.st/book/threads-required) that can be specified as a [per-test override](https://nexte.st/book/per-test-overrides). This can be used to limit concurrency for heavier tests, to avoid overwhelming CPU or running out of memory.

## [0.9.41] - 2022-11-01

This release ran into an issue during publishing and was skipped.

## [0.9.40] - 2022-10-25

### Added

- Overrides can now be restricted to certain platforms, using triples or `cfg()` expressions. For example, to add retries, but only on macOS:

  ```toml
  [[profile.default.overrides]]
  platform = 'cfg(target_os = "macos")'
  retries = 3
  ```

  For an override to match, `platform` and `filter` (if specified) must both be true for a given test. While [cross-compiling code](https://nexte.st/book/running#filtering-by-build-platform), `platform` is matched against the host platform for host tests, and against the target platform for target tests.

- Nextest now reads environment variables specified in [the `[env]` section](https://doc.rust-lang.org/cargo/reference/config.html#env) from `.cargo/config.toml` files. The full syntax is supported including `force` and `relative`.

  Thanks to [Waleed Khan](https://github.com/arxanas) for your first contribution!

- Nextest now sets the `CARGO_PKG_RUST_VERSION` environment variable when it runs tests. For `cargo test` this was added in Rust 1.64, but nextest sets it across all versions of Rust.

## [0.9.39] - 2022-10-14

### Added

- On Unix platforms, if a process times out, nextest attempts to terminate it gracefully by sending it `SIGTERM`, waiting for a grace period of 10 seconds, and then sending it `SIGKILL`. A custom grace period can now be specified through the `slow-timeout.grace-period` parameter. For more information, see [How nextest terminates tests](https://nexte.st/book/slow-tests#how-nextest-terminates-tests).

  Thanks to [Ben Kimock](https://github.com/saethlin) for your first contribution!

### Internal improvements

- Updated clap to version 4.0.

## [0.9.38] - 2022-10-05

### Added

- Test retries now support fixed delays and exponential backoffs, with optional jitter. See [Delays and backoff](https://nexte.st/book/retries#delays-and-backoff) for more information. Thanks [Tomas Olvecky](https://github.com/tomasol) for your first contribution!

### Internal improvements

- Reading test data from standard output and standard error no longer buffers twice, just once. Thanks [Jiahao XU](https://github.com/NobodyXu) for your first contribution!

> Note to distributors: now that Rust 1.64 is out, `process_group_bootstrap_hack` is no longer supported or required. Please remove the following environment variables if you've set them:
>
> - `RUSTC_BOOTSTRAP=1`
> - `RUSTFLAGS='--cfg process_group --cfg process_group_bootstrap_hack'`

## [0.9.37] - 2022-09-30

### Added

- Support for a negative value for `--test-threads`/`-j`, matching support in recent versions of
  Cargo. A value of -1 means the number of logical CPUs minus 1, and so on. Thanks [Onigbinde
  Oluwamuyiwa Elijah](https://github.com/OLUWAMUYIWA) for your first contribution!
- Add a note to the help text for `--test-threads` indicating that its default value is obtained
  from the profile. Thanks [jiangying](https://github.com/jiangying000) for your first contribution!

### Changed

- Internal dependency target-spec bumped to 1.2.0 -- this means that newer versions of the windows
  crate are now supported.
- MSRV updated to Rust 1.62.

## [0.9.36] - 2022-09-07

### Added

- A new `--hide-progress-bar` option (environment variable `NEXTEST_HIDE_PROGRESS_BAR`) forces the
  progress bar to be hidden. Thanks [Remo Senekowitsch](https://github.com/remlse) for your first
  contribution!

### Changed

- Nextest now prints out a list of failing and flaky tests at the end of output by default (the
  `final-status-level` config is set to `flaky`).
- The progress bar is now hidden if a CI environment is detected.

## [0.9.35] - 2022-08-17

### Added

- Support for the `--config` argument, stabilized in Rust 1.63. This option is used to configure
  Cargo, not nextest. This argument is passed through to Cargo, and is also used by nextest to
  determine e.g. [the target runner](https://nexte.st/book/target-runners.html) for a platform.

  `--config` is also how [Miri](https://nexte.st/book/miri.html) communicates with nextest.

- [Target runners](https://nexte.st/book/target-runners.html) for cross-compilation now work with
  [build archives](https://nexte.st/book/reusing-builds.html). Thanks [Pascal
  Kuthe](https://github.com/pascalkuthe) for your first contribution!

## [0.9.34] - 2022-08-12

### Added

- For `cargo nextest self update`, added `-f` as a short-form alias for `--force`.

### Fixed

- Tests are no longer retried after a run is canceled. Thanks [iskyzh] for your contribution!

[iskyzh]: https://github.com/iskyzh

## [0.9.33] - 2022-07-31

### Fixed

- Fixed regression in cargo-nextest 0.9.32 where it no longer produced any output if stderr wasn't a terminal.

## [0.9.32] - 2022-07-30

### Added

- `cargo nextest run` now has a new `--no-run` feature to build but not run tests. (This was previously achievable with `cargo nextest list -E 'none()'`, but is more intuitive this way.)
- Pre-built binaries are now available for i686 Windows. Thanks [Guiguiprim](https://github.com/Guiguiprim)!

### Internal improvements

- Filter expression evaluation now uses a stack machine via the [recursion](https://crates.io/crates/recursion) crate. Thanks [Inanna](https://github.com/inanna-malick) for your first contribution!

## [0.9.31] - 2022-07-27

### Added

- Nextest sets a new `NEXTEST_RUN_ID` environment variable with a UUID for a test run. All tests run
  within a single invocation of `cargo nextest run` will set the same run ID. Thanks [mitsuhiko] for
  your first contribution!

[mitsuhiko]: https://github.com/mitsuhiko

## [0.9.30] - 2022-07-25

### Fixed

- Fixed target runners specified as relative paths.
- On Unix, cargo-nextest's performance had regressed (by 3x on [clap](https://github.com/clap-rs/clap)) due to the change introduced in version 0.9.29 to put each test process into its own process group. In this version, this regression has been fixed, but only if you're using the pre-built binaries or building on Rust 1.64+ (currently in nightly).

  > Note to distributors: to fix this regression while building with stable Rust 1.62, set the following environment variables:
  >
  > - `RUSTC_BOOTSTRAP=1`
  > - `RUSTFLAGS='--cfg process_group --cfg process_group_bootstrap_hack'`
  >
  > This is temporary until [the `process_set_process_group` feature is stabilized](https://github.com/rust-lang/rust/issues/93857) in Rust 1.64.

## [0.9.29] - 2022-07-24

### Added

- On Unix, each test process is now put into its own [process group]. If a test times out or Ctrl-C is pressed, the entire process group is signaled. This means that most subprocesses spawned by tests are also killed.

  However, because process groups aren't nested, if a test creates a process group itself, those groups won't be signaled. This is a relatively uncommon situation.

- On Windows, each test process is now associated with a [job object]. On timeouts, the entire job object is terminated. Since job objects _are_ nested in recent versions of Windows, this should result in all subprocesses spawned by tests being killed.

  (On Windows, the Ctrl-C behavior hasn't changed. Nextest also doesn't do graceful shutdowns on Windows yet, though this may change in the future.)

- Nextest can now parse Cargo configs specified via the unstable `--config` option.

- Nextest now publishes binaries for `aarch64-unknown-linux-gnu` ([#398]) and `x86_64-unknown-linux-musl` ([#399]). Thanks [messense] and [Teymour] for your first contributions!

### Fixed

- Per-test overrides are now additive across configuration files (including tool-specific configuration files).

[process group]: https://en.wikipedia.org/wiki/Process_group
[job object]: https://docs.microsoft.com/en-us/windows/win32/procthread/job-objects
[#398]: https://github.com/nextest-rs/nextest/pull/398
[#399]: https://github.com/nextest-rs/nextest/pull/399
[messense]: https://github.com/messense
[Teymour]: https://github.com/teymour-aldridge

## [0.9.28] - 2022-07-22

This is a quick hotfix release to ensure that the right `tokio` features are enabled under
`default-no-update`.

## [0.9.27] - 2022-07-22

This is a major architectural rework of nextest. We've tested it thoroughly to the best of our ability, but if you see regressions please [report them](https://github.com/nextest-rs/nextest/issues/new)!

If you encounter a regression, you can temporarily pin nextest to the previous version in CI. If you're on GitHub Actions and are using `taiki-e/install-action`, use this instead:

```yaml
- uses: taiki-e/install-action@v1
- with:
    tool: nextest
    version: 0.9.26
```

### Added

- Nextest now works with [the Miri interpreter](https://nexte.st/book/miri). Use `cargo miri nextest run` to run your tests with Miri.
- Nextest now detects some situations where tests [leak subprocesses](https://nexte.st/book/leaky-tests). Previously, these situations would cause nextest to hang.
- [Per-test overrides](https://nexte.st/book/per-test-overrides) now support `slow-timeout` and the new `leak-timeout` config parameter.
- A new option `--tool-config-file` allows tools that wrap nextest to specify custom config settings, while still prioritizing repository-specific configuration.

### Changed

- Major internal change: The nextest list and run steps now use [Tokio](https://tokio.rs/). This change enables the leak detection described above.
- The list step now runs list commands in parallel. This should result in speedups in most cases.

### Fixed

- Nextest now redirects standard input during test runs to `/dev/null` (or `NUL` on Windows). Most tests do not read from standard input, but if a test does, it will no longer cause nextest to hang.
- On Windows, nextest configures standard input, standard output and standard error to [not be inherited](https://github.com/nextest-rs/nextest/commit/0c109db35a5315d7d8c9121e36a9e706e7393049). This prevents some kinds of test hangs on Windows.
- If a [dynamic library link path](https://nexte.st/book/env-vars.html#dynamic-library-paths) doesn't exist, nextest no longer adds it to `LD_LIBRARY_PATH` or equivalent. This should have no practical effect.
- Archiving tests now works even if the target directory is not called `"target"`.

## [0.9.26] - 2022-07-14

This is a quick hotfix release to update the version of nextest-metadata, to which a breaking change was accidentally committed.

## [0.9.25] - 2022-07-13

This is a major release with several new features.

### Filter expressions

[Filter expressions](https://nexte.st/book/filter-expressions) are now ready for production. For example, to run all tests in `nextest-runner` and all its transitive dependencies within the workspace:

```
cargo nextest run -E 'deps(nextest-runner)'
```

This release includes a number of additions and changes to filter expressions.

#### Added

- The expression language supports several new [predicates](https://nexte.st/book/filter-expressions#basic-predicates):
  - `kind(name-matcher)`: include all tests in binary kinds (e.g. `lib`, `test`, `bench`) matching `name-matcher`.
  - `binary(name-matcher)`: include all tests in binary names matching `name-matcher`.
  - `platform(host)` or `platform(target)`: include all tests that are [built for the host or target platform](https://nexte.st/book/running#filtering-by-build-platform), respectively.

#### Changed

- If a filter expression is guaranteed not to match a particular binary, it will not be listed by nextest. (This allows `platform(host)` and `platform(target)` to work correctly.)

- If both filter expressions and standard substring filters are passed in, a test must match filter expressions AND substring filters to be executed. For example:

```
cargo nextest run -E 'package(nextest-runner)' test_foo test_bar
```

This will execute only the tests in `nextest-runner` that match `test_foo` or `test_bar`.

### Per-test overrides

Nextest now supports [per-test overrides](https://nexte.st/book/per-test-overrides). These overrides let you customize settings for subsets of tests. For example, to retry tests that contain the substring `test_e2e` 3 times:

```toml
[[profile.default.overrides]]
filter = "test(test_e2e)"
retries = 3
```

Currently, only `retries` are supported. In the future, more kinds of customization will be added.

### Other changes

- A new environment variable `NEXTEST_RETRIES` controls the number of retries tests are run with. In terms of precedence, this slots in between the command-line `--retries` option and per-test overrides for retries.
- `cargo nextest list` now hides skipped tests and binaries by default. To print out skipped tests and binaries, use `cargo nextest list --verbose`.
- The [Machine-readable output](https://nexte.st/book/machine-readable) for `cargo nextest list` now contains a new `"status"` key. By default, this is set to `"listed"`, and for binaries that aren't run because they don't match expression filters this is set to `"skipped"`.
- The `--platform-filter` option is deprecated, though it will keep working for all versions within the nextest 0.9 series. Use `-E 'platform(host)'` or `-E 'platform(target)'` instead.
- `cargo nextest run -- --skip` and `--exact` now suggest using a filter expression instead.

## [0.9.24] - 2022-07-01

### Added

- New config option `profile.<profile-name>.test-threads` controls the number of tests run simultaneously. This option accepts either an integer with the number of threads, or the string "num-cpus" (default) for the number of logical CPUs. As usual, this option is overridden by `--test-threads` and `NEXTEST_TEST_THREADS`, in that order.
- The command-line `--test-threads` option and the `NEXTEST_TEST_THREADS` environment variable now accept `num-cpus` as their argument.
- nextest now works with [cargo binstall](https://github.com/ryankurte/cargo-binstall) ([#332]). Thanks [@remoun] for your first contribution!

### Fixed

- Within JUnit XML, test failure descriptions (text nodes for `<failure>` and `<error>` tags) now have invalid ANSI escape codes stripped from their output.

[#332]: https://github.com/nextest-rs/nextest/pull/332
[@remoun]: https://github.com/remoun

## [0.9.23] - 2022-06-26

### Added

- On Windows, nextest now detects tests that abort due to e.g. an access violation (segfault) and prints their status as "ABORT" rather than "FAIL", along with an explanatory message on the next line.
- Improved JUnit support: nextest now heuristically detects stack traces and adds them to the text node of the `<failure>` element ([#311]).

### Changed

- Errors that happen while writing data to the output now have a new documented exit code: [`WRITE_OUTPUT_ERROR`].

[#311]: https://github.com/nextest-rs/nextest/issues/311
[`WRITE_OUTPUT_ERROR`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.WRITE_OUTPUT_ERROR

## [0.9.22] - 2022-06-21

### Added

- Benchmarks are now treated as normal tests. ([#283], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  Note that criterion.rs benchmarks are currently incompatible with nextest ([#96]) -- this change doesn't have any effect on that.

- Added `-F` as a shortcut for `--features`, mirroring an upcoming addition to Cargo 1.62 ([#287], thanks [Alexendoo](https://github.com/Alexendoo) for your first contribution!)

### Changed

- If nextest's output is colorized, it no longer strips ANSI escape codes from test runs.

[#283]: https://github.com/nextest-rs/nextest/pull/283
[#287]: https://github.com/nextest-rs/nextest/pull/287
[#96]: https://github.com/nextest-rs/nextest/issues/96

## [0.9.21] - 2022-06-17

### Added

- On Unix, tests that fail due to a signal (e.g. SIGSEGV) will print out the name of the signal rather than the generic "FAIL".
- `cargo-nextest` has a new `"default-no-update"` feature that will contain all default features except for self-update. If you're distributing nextest or installing it in CI, the recommended, forward-compatible way to build cargo-nextest is with `--no-default-features --features default-no-update`.

### Changed

- Progress bars now take up the entire width of the screen. This prevents issues with the bar wrapping around on terminals that aren't wide enough.

## [0.9.20] - 2022-06-13

### Fixed

- Account for skipped tests when determining the length of the progress bar.

## [0.9.19] - 2022-06-13

### Added

- Nextest can now update itself! Once this version is installed, simply run `cargo nextest self update` to update to the latest version.
  > Note to distributors: you can disable self-update by building cargo-nextest with `--no-default-features`.
- Partial, emulated support for test binary arguments passed in after `cargo nextest run --` ([#265], thanks [@tabokie](https://github.com/tabokie) for your contribution!).

  For example, `cargo nextest run -- my_test --ignored` will run ignored tests containing `my_test`, similar to `cargo test -- my_test --ignored`.

  Support is limited to test names, `--ignored` and `--include-ignored`.

  > Note to integrators: to reliably disable all argument parsing, pass in `--` twice. For example, `cargo nextest run -- -- <filters...>`.

### Fixed

- Better detection for cross-compilation -- now look through the `CARGO_BUILD_TARGET` environment variable, and Cargo configuration as well. The `--target` option is still preferred.
- Slow and flaky tests are now printed out properly in the final status output ([#270]).

[#265]: https://github.com/nextest-rs/nextest/pull/265
[#270]: https://github.com/nextest-rs/nextest/issues/270

This is a test release.

## [0.9.18] - 2022-06-08

### Added

- Support for terminating tests if they take too long, via the configuration parameter `slow-timeout.terminate-after`. For example, to time out after 120 seconds:

  ```toml
  slow-timeout = { period = "60s", terminate-after = 2 }
  ```

  Thanks [steveeJ](https://github.com/steveeJ) for your contribution ([#214])!

[#214]: https://github.com/nextest-rs/nextest/pull/214

### Fixed

- Improved support for [reusing builds](https://nexte.st/book/reusing-builds): produce better error messages if the workspace's source is missing.

## [0.9.17] - 2022-06-07

This release contains a number of user experience improvements.

### Added

- If producing output to an interactive terminal, nextest now prints out its status as a progress bar. This makes it easy to see the status of a test run at a glance.
- Nextest's configuration has a new `final-status-level` option which can be used to print out some statuses at the end of a run (defaults to `none`). On the command line, this can be overridden with the `--final-status-level` argument or `NEXTEST_FINAL_STATUS_LEVEL` in the environment.
- If a [target runner](https://nexte.st/book/target-runners) is in use, nextest now prints out its name and the environment variable or config file the definition was obtained from.

### Changed

- If the creation of a test list fails, nextest now prints a more descriptive error message, and exits with the exit code 104 ([`TEST_LIST_CREATION_FAILED`]).

[`TEST_LIST_CREATION_FAILED`]: https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.TEST_LIST_CREATION_FAILED

## [0.9.16] - 2022-06-02

### Added

- Nextest now [sets `NEXTEST_LD_*` and `NEXTEST_DYLD_*` environment
  variables](https://nexte.st/book/env-vars.html#environment-variables-nextest-sets) to work around
  macOS System Integrity Protection sanitization.

### Fixed

- While [archiving build artifacts](https://nexte.st/book/reusing-builds), work around some libraries producing linked paths that don't exist ([#247]). Print a warning for those paths instead of failing.

[#247]: https://github.com/nextest-rs/nextest/issues/247

### Changed

- Build artifact archives no longer recurse into linked path subdirectories. This is not a behavioral change because `LD_LIBRARY_PATH` and other similar variables do not recurse into subdirectories either.

## [0.9.15] - 2022-05-31

### Added

- Improved support for [reusing builds](https://nexte.st/book/reusing-builds):
  - New command `cargo nextest archive` automatically archives test binaries and other relevant
    files after building tests. Currently the `.tar.zst` format is supported.
  - New option `cargo nextest run --archive-file` automatically extracts archives before running the tests within them.
  - New runtime environment variable `NEXTEST_BIN_EXE_<name>` is set to the absolute path to a binary target's executable, taking path remapping into account. This is equivalent to [`CARGO_BIN_EXE_<name>`], except this is set at runtime.
  - `cargo nextest list --list-type binaries-only` now records information about non-test binaries as well.

[`CARGO_BIN_EXE_<name>`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates

### Fixed

Fix for experimental feature [filter expressions](https://nexte.st/book/filter-expressions.html):

- Fix test filtering when expression filters are set but name-based filters aren't.

## [0.9.14] - 2022-04-18

### Fixed

Fixes related to path remapping:

- Directories passed into `--workspace-remap` and `--target-dir-remap` are now canonicalized.
- If the workspace directory is remapped, `CARGO_MANIFEST_DIR` in tests' runtime environment is set to the new directory.

## [0.9.13] - 2022-04-16

### Added

- Support for [reusing builds](https://nexte.st/book/reusing-builds) is now production-ready. Build on one machine and run tests on another, including cross-compiling and test partitioning.

  To see how builds can be reused in GitHub Actions, see [this example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml).

- Experimental support for [filter expressions](https://nexte.st/book/filter-expressions.html), allowing fine-grained specifications for which tests to run.

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their fantastic work implementing both of these.

## [0.9.12] - 2022-03-22

### Added

- Support for reading some configuration as [environment variables](https://nexte.st/book/env-vars#environment-variables-nextest-reads). (Thanks [ymgyt] and [iskyzh] for their pull requests!)
- [Machine-readable output] for `cargo nextest list` now contains a `rust-build-meta` key. This key currently contains the target directory, the base output directories, and paths to [search for dynamic libraries in](https://nexte.st/book/env-vars#dynamic-library-paths) relative to the target directory.

### Fixed

- Test binaries that link to dynamic libraries built by Cargo now work correctly ([#82]).
- Crates with no tests are now skipped while computing padding widths in the reporter ([#125]).

### Changed

- MSRV updated to Rust 1.56.
- For experimental feature [reusing builds](https://nexte.st/book/reusing-builds):
  - Change `--binaries-dir-remap` to `--target-dir-remap` and expect that the entire target directory is archived.
  - Support linking to dynamic libraries ([#82]).

[#82]: https://github.com/nextest-rs/nextest/issues/82
[#125]: https://github.com/nextest-rs/nextest/issues/125
[ymgyt]: https://github.com/ymgyt
[iskyzh]: https://github.com/iskyzh
[Machine-readable output]: https://nexte.st/book/machine-readable

## [0.9.11] - 2022-03-09

### Fixed

- Update `regex` to 1.5.5 to address [GHSA-m5pq-gvj9-9vr8
  (CVE-2022-24713)](https://github.com/rust-lang/regex/security/advisories/GHSA-m5pq-gvj9-9vr8).

## [0.9.10] - 2022-03-07

Thanks to [Guiguiprim](https://github.com/Guiguiprim) for their contributions to this release!

### Added

- A new `--platform-filter` option filters tests by the platform they run on (target or host).
- `cargo nextest list` has a new `--list-type` option, with values `full` (the default, same as today) and `binaries-only` (list out binaries without querying them for the tests they contain).
- Nextest executions done as a separate process per test (currently the only supported method, though this might change in the future) set the environment variable `NEXTEST_PROCESS_MODE=process-per-test`.

### New experimental features

- Nextest can now reuse builds across invocations and machines. This is an experimental feature, and feedback is welcome in [#98]!

[#98]: https://github.com/nextest-rs/nextest/issues/98

### Changed

- The target runner is now build-platform-specific; test binaries built for the host platform will be run by the target runner variable defined for the host, and similarly for the target platform.

## [0.9.9] - 2022-03-03

### Added

- Updates for Rust 1.59:
  - Support abbreviating `--release` as `-r` ([Cargo #10133]).
  - Stabilize future-incompat-report ([Cargo #10165]).
  - Update builtin list of targets (used by the target runner) to Rust 1.59.

[Cargo #10133]: https://github.com/rust-lang/cargo/pull/10133
[Cargo #10165]: https://github.com/rust-lang/cargo/pull/10165

## [0.9.8] - 2022-02-23

### Fixed

- Target runners of the form `runner = ["bin-name", "--arg1", ...]` are now parsed correctly ([#75]).
- Binary IDs for `[[bin]]` and `[[example]]` tests are now unique, in the format `<crate-name>::bin/<binary-name>` and `<crate-name>::test/<binary-name>` respectively ([#76]).

[#75]: https://github.com/nextest-rs/nextest/pull/75
[#76]: https://github.com/nextest-rs/nextest/pull/76

## [0.9.7] - 2022-02-23

### Fixed

- If parsing target runner configuration fails, warn and proceed without a target runner rather than erroring out.

### Known issues

- Parsing an array of strings for the target runner currently fails: [#73]. A fix is being worked on in [#75].

[#73]: https://github.com/nextest-rs/nextest/issues/73
[#75]: https://github.com/nextest-rs/nextest/pull/75

## [0.9.6] - 2022-02-22

### Added

- Support Cargo configuration for [target runners](https://nexte.st/book/target-runners).

## [0.9.5] - 2022-02-20

### Fixed

- Updated nextest-runner to 0.1.2, fixing cyan coloring of module paths ([#52]).

[#52]: https://github.com/nextest-rs/nextest/issues/52

## [0.9.4] - 2022-02-16

The big new change is that release binaries are now available! Head over to [Pre-built binaries](https://nexte.st/book/pre-built-binaries) for more.

### Added

- In test output, module paths are now colored cyan ([#42]).

### Fixed

- While querying binaries to list tests, lines ending with ": benchmark" will now be ignored ([#46]).

[#42]: https://github.com/nextest-rs/nextest/pull/42
[#46]: https://github.com/nextest-rs/nextest/issues/46

## [0.9.3] - 2022-02-14

### Fixed

- Add a `BufWriter` around stderr for the reporter, reducing the number of syscalls and fixing
  issues around output overlap on Windows ([#35](https://github.com/nextest-rs/nextest/issues/35)). Thanks [@fdncred](https://github.com/fdncred) for reporting this!

## [0.9.2] - 2022-02-14

### Fixed

- Running cargo nextest from within a crate now runs tests for just that crate, similar to cargo
  test. Thanks [Yaron Wittenstein](https://twitter.com/RealWittenstein/status/1493291441384210437)
  for reporting this!

## [0.9.1] - 2022-02-14

### Fixed

- Updated nextest-runner to 0.1.1, fixing builds on Rust 1.54.

## [0.9.0] - 2022-02-14

**Initial release.** Happy Valentine's day!

### Added

Supported in this initial release:

- [Listing tests](https://nexte.st/book/listing.md)
- [Running tests in parallel](https://nexte.st/book/running.md) for faster results
- [Partitioning tests](https://nexte.st/book/partitioning.md) across multiple CI jobs
- [Test retries](https://nexte.st/book/retries.md) and flaky test detection
- [JUnit support](https://nexte.st/book/junit.md) for integration with other test tooling

[0.9.85-rc.1]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.85-rc.1
[0.9.84]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.84
[0.9.83]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.83
[0.9.82]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.82
[0.9.81]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.81
[0.9.80]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.80
[0.9.79]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.79
[0.9.78]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.78
[0.9.77]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.77
[0.9.76]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.76
[0.9.75]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.75
[0.9.74]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.74
[0.9.73]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.73
[0.9.72]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.72
[0.9.71]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.71
[0.9.70]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.70
[0.9.69]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.69
[0.9.68]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.68
[0.9.67]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.67
[0.9.66]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.66
[0.9.65]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.65
[0.9.64]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.64
[0.9.63]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.63
[0.9.62]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.62
[0.9.61]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.61
[0.9.60]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.60
[0.9.59]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.59
[0.9.58]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.58
[0.9.57]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.57
[0.9.56]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.56
[0.9.55]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.55
[0.9.54]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.54
[0.9.53]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.53
[0.9.52]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.52
[0.9.51]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.51
[0.9.50]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.50
[0.9.49]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.49
[0.9.48]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.48
[0.9.47]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.47
[0.9.46]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.46
[0.9.45]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.45
[0.9.44]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.44
[0.9.43]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.43
[0.9.42]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.42
[0.9.41]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.41
[0.9.40]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.40
[0.9.39]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.39
[0.9.38]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.38
[0.9.37]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.37
[0.9.36]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.36
[0.9.35]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.35
[0.9.34]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.34
[0.9.33]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.33
[0.9.32]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.32
[0.9.31]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.31
[0.9.30]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.30
[0.9.29]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.29
[0.9.29-rc.1]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.29-rc.1
[0.9.28]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.28
[0.9.27]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.27
[0.9.26]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.26
[0.9.25]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.25
[0.9.24]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.24
[0.9.23]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.23
[0.9.22]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.22
[0.9.21]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.21
[0.9.20]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.20
[0.9.19]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.19
[0.9.18]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.18
[0.9.17]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.17
[0.9.16]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.16
[0.9.15]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.15
[0.9.14]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.14
[0.9.13]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.13
[0.9.12]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.12
[0.9.11]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.11
[0.9.10]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.10
[0.9.9]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.9
[0.9.8]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.8
[0.9.7]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.7
[0.9.6]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.6
[0.9.5]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.5
[0.9.4]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.4
[0.9.3]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.3
[0.9.2]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.2
[0.9.1]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.1
[0.9.0]: https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.0
