# Environment variables

This section contains information about the environment variables nextest reads and sets.

## Environment variables nextest reads

Nextest reads the following environment variables to emulate the behavior of Cargo:
* `CARGO` — Path to the `cargo` binary to use for builds.
* `CARGO_TARGET_<triple>_RUNNER` — Support for [target runners](target-runners.md).

Currently, cargo-nextest does not read its own configuration as environment variables. [This will be supported in the future](https://github.com/nextest-rs/nextest/issues/14).

### Cargo-related environment variables nextest reads

cargo-nextest delegates to Cargo for the build, which recognizes a number of environment variables. See [Environment variables Cargo reads](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads) for a full list.

## Environment variables nextest sets

cargo-nextest exposes these environment variables to your tests *at runtime only*. They are not set at build time because cargo-nextest may reuse builds done outside of the nextest environment.

* `NEXTEST` — always set to `"1"`.

### Cargo-related environment variables nextest sets

cargo-nextest delegates to Cargo for the build, which controls the environment variables that are set. See [Environment variables Cargo sets for crates](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates) for a full list.

cargo-nextest also sets these environment variables at runtime, matching the behavior of cargo test:

* `CARGO` — Path to the `cargo` binary performing the build.
* `CARGO_MANIFEST_DIR` — The directory containing the manifest of your package.
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
