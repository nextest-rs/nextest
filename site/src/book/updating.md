# Updating nextest

Starting version 0.9.19, cargo-nextest has update functionality built-in. Simply run `cargo nextest self update` to check for and perform updates.

The nextest updater downloads and installs the latest version of the cargo-nextest binary from [get.nexte.st](https://get.nexte.st).

To request a specific version, run (e.g.) `cargo nextest self update --version 0.9.19`.

## For older versions

If you're on cargo-nextest 0.9.18 or below, update by redownloading and reinstalling the binary following the instructions at [Pre-built binaries](pre-built-binaries.md).

## For distributors

`cargo-nextest` 0.9.21 and above has a new `default-no-update` feature, which will contain all default features except for self-update. The recommended, forward-compatible way to build cargo-nextest is with `--no-default-features --features default-no-update`.
