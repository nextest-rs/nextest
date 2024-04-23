# Archiving and reusing builds

In some cases, it can be useful to separate out building tests from running them. Nextest supports archiving builds on one machine, and then extracting the archive to run tests on another machine.

## Terms

- **Build machine:** The computer that builds tests.
- **Target machine:** The computer that runs tests.

## Use cases

- **Cross-compilation.** The build machine has a different architecture, or runs a different operating system, from the target machine.
- **Test partitioning.** Build once on the build machine, then [partition test execution](partitioning.md) across multiple target machines.
- **Saving execution time on more valuable machines.** For example, build tests on a regular machine, then run them on a machine with a GPU attached to it.

## Requirements

- **The project source must be checked out to the same revision on the target machine.** This might be needed for test fixtures and other assets, and nextest sets the right working directory relative to the workspace root when executing tests.
- **It is your responsibility to transfer over the archive.** Use the examples below as a template.
- **Nextest must be installed on the target machine.** For best results, use the same version of nextest on both machines.

### Non-requirements

- **Cargo does not need to be installed on the target machine.** If `cargo` is unavailable, replace `cargo nextest` with `cargo-nextest nextest` in the following examples.

## Creating archives

`cargo nextest archive --archive-file <name-of-archive.tar.zst>` creates an archive with the following contents:

- Cargo-related metadata, at the location `target/nextest/cargo-metadata.json`.
- Metadata about test binaries, at the location `target/nextest/binaries-metadata.json`.
- All test binaries
- Other relevant files:

  - Dynamic libraries that test binaries might link to
  - Non-test binaries used by integration tests
  - Starting nextest 0.9.66, build script output directories for workspace packages that have associated test binaries

    > **NOTE:** Currently, `OUT_DIR`s are only archived one level deep to avoid bloating archives too much. In the future, we may add configuration to archive more or less of the output directory. If you have a use case that would benefit from this, please [file an issue](https://github.com/nextest-rs/nextest/issues/new).

**Note that archives do not include the source code for your project.** It is your responsibility to ensure that the source code for your workspace is transferred over to the target machine and has the same contents.

Currently, the only format supported is a Zstandard-compressed tarball (`.tar.zst`).

### Adding extra files to an archive

Starting nextest 0.9.69, you can include extra files within archives. This can be useful if your
build process creates artifacts outside of Cargo that are required for Rust tests.

Use the `profile.<profile-name>.archive.include` configuration option for this. For example:

```toml
[profile.default]
archive.include = [
    { path = "my-extra-path", relative-to = "target" },
    { path = "other-path", relative-to = "target", depth = 2, on-missing = "error" },
]
```

`archive.include` takes a list of tables, with the following parameters:

- `path` — The relative path to the archive.
- `relative-to` — The root directory that `path` is defined against. Currently, only `"target"` is
  supported, to indicate the target directory.
- `depth` — The recursion depth for directories; either a non-negative integer or `"infinite"`. The
  default is a depth of 16, which should cover most non-degenerate use cases.
- `on-missing` — What to do if the specified path was not found. One of `"warn"` (default),
  `"ignore"`, or `"error"`.

> NOTE: The following features are not currently supported:
>
> - Excluding subdirectories ([#1456]).
> - Paths relative to something other than the target directory ([#1457]).
> - Per-test-binary and per-platform overrides ([#1460]).
>
> Help on any of these would be greatly appreciated!

[#1456]: https://github.com/nextest-rs/nextest/issues/1456
[#1457]: https://github.com/nextest-rs/nextest/issues/1457
[#1460]: https://github.com/nextest-rs/nextest/issues/1460

## Running tests from archives

`cargo nextest list` and `run` support a new `--archive-file` option. This option accepts archives created by `cargo nextest archive` as above.

By default, archives are extracted to a temporary directory, and nextest remaps paths to use the new
target directory. To specify the directory archives should be extracted to, use the `--extract-to`
option.

### Specifying a new location for the source code

By default, nextest expects the workspace's source code to be in the same location on both the build and target machines. To specify a new location for the workspace, use the `--workspace-remap <path-to-workspace-root>` option with the `list` or `run` commands.

## Example: Simple build/run split

1. Build and archive tests:

   Here you should specify all the options you would normally use to build your tests.

   ```shell
   cargo nextest archive --workspace --all-features --archive-file my-archive.tar.zst
   ```

2. Run the tests:

   Archive keeps the options used to build the tests, so you **should not** specify them again.

   ```shell
   cargo nextest run --archive-file my-archive.tar.zst
   ```

## Example: Use in GitHub Actions

See [this working example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml) for how to reuse builds and [partition test runs](partitioning.md) on GitHub Actions.

## Example: Cross-compilation

While cross-compiling code, some tests may need to be run on the host platform. (See the note about [Filtering by build platform](running.md#filtering-by-build-platform) for more.)

### On the build machine

1. Build and run host-only tests:

   ```shell
   cargo nextest run --target <TARGET> -E 'platform(host)'
   ```

2. Archive tests:

   ```shell
   cargo nextest archive --target <TARGET> --archive-file my-archive.tar.zst
   ```

3. Copy `my-archive.tar.zst` to the target machine.

### On the target machine

1. Check out the project repository to a path `<REPO-PATH>`, to the same revision as the build machine.
2. List target-only tests:

   ```shell
   cargo nextest list -E 'platform(target)' \
       --archive-file my-archive.tar.zst \
       --workspace-remap <REPO-PATH>
   ```

3. Run target-only tests:

   ```shell
   cargo nextest run -E 'platform(target)' \
       --archive-file my-archive.tar.zst \
       --workspace-remap <REPO-PATH>
   ```

## Manually creating your own archives

You can also create and manage your own archives, with the following options to `cargo nextest list` and `run`:

- `--binaries-metadata`: The path to JSON metadata generated by `cargo nextest list --list-type binaries-only --message-format json`.
- `--target-dir-remap`: A possible new location for the target directory. Requires `--binaries-metadata`.
- `--cargo-metadata`: The path to JSON metadata generated by `cargo metadata --format-version 1`.

## Making tests relocatable

Some tests may need to be modified to handle changes in the workspace and target directories. Some common situations:

- To obtain the path to the source directory, Cargo provides the `CARGO_MANIFEST_DIR` option at both build time and runtime. For relocatable tests, use the value of `CARGO_MANIFEST_DIR` at runtime. This means `std::env::var("CARGO_MANIFEST_DIR")`, not `env!("CARGO_MANIFEST_DIR")`.

  If the workspace is remapped, nextest automatically sets `CARGO_MANIFEST_DIR` to the new location.

- To obtain the path to a crate's executables, Cargo provides the [`CARGO_BIN_EXE_<name>`] option to integration tests at build time. To handle target directory remapping, use the value of `NEXTEST_BIN_EXE_<name>` at runtime.

  To retain compatibility with `cargo test`, you can fall back to the value of `CARGO_BIN_EXE_<name>` at build time.

[`CARGO_BIN_EXE_<name>`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates

## Options and arguments for `cargo nextest archive`

```
{{#include ../../help-text/archive-help.txt}}
```
