---
icon: material/update
description: Updating nextest with self update or from source.
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

### Beta and RC channels

<!-- md:version 0.9.120 -->

The nextest project occasionally publishes betas and release candidates (RCs) to test new features. To update to the latest prerelease, run:

```sh
# Update to the latest beta release:
cargo nextest self update --beta

# Update to the latest RC release:
cargo nextest self update --rc
```

If no newer betas or RCs are available, these commands update to the latest stable release.

You can also specify an exact prerelease version. This approach works with older versions of nextest as well:

```sh
# Update to a specific version:
cargo nextest self update --version 0.9.119-b.2
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

## Options and arguments

=== "Summarized output"

    The output of `cargo nextest self update -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest self update -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest self update -h | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest self update --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest self update --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest self update --help | ../scripts/strip-hyperlinks.sh
        ```
