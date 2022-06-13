# Updating nextest

Starting version 0.9.19, cargo-nextest has update functionality built-in. Simply run `cargo nextest self update` to check for and perform updates.

The nextest updater downloads and installs the latest version of the cargo-nextest binary from [get.nexte.st](https://get.nexte.st).

To request a specific version, run (e.g.) `cargo nextest self update --version 0.9.19`.

## For older versions

For cargo-nextest 0.9.18 or below, update by redownloading and reinstalling the binary following the instructions at [Pre-built binaries](pre-built-binaries.md).

## For distributors

To disable the self-update functionality, build cargo-nextest with `--no-default-features`.
