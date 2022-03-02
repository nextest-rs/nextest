# Reusing build

**This is an experimental feature: set the environment variable `NEXTEST_EXPERIMENTAL_REUSE_BUILD=1` to use it.**

In some cases, it can be useful to split tests building and tests running in two steps. This includes:

- Cross compilation: the build machine is not the same as the target machine
- Partitioned execution: build once and partition the execution
- Saving execution time on a machine: do not build on a GPU machine, ...

Requirement:

- the project source must be checkout on the running machine: this might be needed for tests assets and we do apply the right working directory relative to the workspace root when executing the tests.
- cargo does not need to be install on the running machine (in which case replace `cargo nextest` by `cargo-nextest nextest` in the following commands)

## Simple build/run split

1. Build the tests and save the binaries metadata: `cargo nextest list --no-query --message-format json > target/binaries-metadata.json`
2. List the tests: `cargo nextest list --binaries-metadata target/binaries-metadata.json`
3. Run the tests: `cargo nextest run --binaries-metadata target/binaries-metadata.json`

## Cross-compilation

Some tests still needs to be run on the host machine, this include proc-macro tests.

1. On the build machine
    1. Save the project cargo metadata: `cargo metadata --format-version=1 --all-features --no-deps > target/cargo-metadata.json`
    2. Build the tests and save the binaries metadata: `cargo nextest list --no-query --target <TARGET> --message-format json > target/binaries-metadata.json`
    3. List host-only tests: `cargo nextest list --platform-filter host --binaries-metadata target/binaries-metadata.json --cargo-metadata target/cargo-metadata.json`
    3. Run host-only tests: `cargo nextest run --platform-filter host --binaries-metadata target/binaries-metadata.json --cargo-metadata target/cargo-metadata.json`
    4. Archive artifacts: both json files and the tests binaries (listing `cat target/binaries-metadata.json | jq '."rust-binaries" | .[] . "binary-path"`)
2. On the target machine
    1. Clone the project repo
    2. Extract artifacts
    3. List target-only tests: `cargo nextest list --platform-filter target --binaries-metadata <PATH>/binaries-metadata.json --cargo-metadata <PATH>/cargo-metadata.json --workspace-remap <REPO-PATH> --binaries-directory-remap <TESTS-FOLDER-PATH>`
    4. Run target-only tests: `cargo nextest run --platform-filter target --binaries-metadata <PATH>/binaries-metadata.json --cargo-metadata <PATH>/cargo-metadata.json --workspace-remap <REPO-PATH> --binaries-directory-remap <TESTS-FOLDER-PATH>`
