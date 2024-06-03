---
icon: material/format-list-group
title: Listing tests
---

# Listing tests

To build and list all tests in a workspace[^doctest], cd into the workspace and run:

```
cargo nextest list
```

`cargo nextest list` takes most of the same options that `cargo nextest run` takes. For a full list of options accepted, see [Options and arguments](#options-and-arguments) below, or `cargo nextest list --help`.

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/list-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/list-output.ansi | ../scripts/strip-ansi.sh
    ```

??? info "Verbose output"

    With `--verbose`, information about binary paths and skipped tests is also printed.

    === "Colorized"

        ```bash exec="true" result="ansi"
        cat src/outputs/list-output-verbose.ansi
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cat src/outputs/list-output-verbose.ansi | ../scripts/strip-ansi.sh
        ```

[^doctest]: Doctests are currently [not supported](https://github.com/nextest-rs/nextest/issues/16) because of limitations in stable Rust. For now, run doctests in a separate step with `cargo test --doc`.

## Options and arguments

=== "Summarized output"

    The output of `cargo nextest list -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest list -h
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest list -h
        ```

=== "Full output"

    The output of `cargo nextest list --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest list --help
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest list --help
        ```
