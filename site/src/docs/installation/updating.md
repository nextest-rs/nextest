---
icon: material/update
---

# Updating nextest

Nextest includes a self-update feature that will fetch and write the latest version of the binary to your system.

## Pre-built binaries

For pre-built binaries installed via `cargo-binstall` or a release URL, run:

```sh
cargo nextest self update
```

The nextest updater downloads and installs the latest version of the cargo-nextest binary from [get.nexte.st](https://get.nexte.st).

To request a specific version, add `--version <version>` to the command. For example, to update to nextest 0.9.72, run:

```
cargo nextest self update --version 0.9.72
```

## From source

Nextest can also be updated from source, by running:

```
cargo install cargo-nextest --locked
```

## With distro packages

For versions of nextest packaged by a distributor, follow the instructions for the respective package manager.

### Note for distributors

The `cargo-nextest` crate has a `default-no-update` feature which consists of all default features except for self-update. The recommended, forward-compatible way to build cargo-nextest is with `--locked --no-default-features --features default-no-update`.
