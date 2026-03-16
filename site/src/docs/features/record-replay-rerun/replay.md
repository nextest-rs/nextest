---
icon: material/play-box-outline
description: Replaying recorded test runs, with options for different output formats.
---

# Replaying test runs

To replay the last test run, run `cargo nextest replay`. This will show output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/replay.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/replay.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

Earlier runs can be replayed by identifying them through their nextest run ID, with the `--run-id`/`-R` option to `cargo nextest replay`. Any unique prefix can be used; in colorized output, unique prefixes are highlighted in bold purple.

Replayed runs automatically use the [configured pager](../../user-config/pager.md), such as `less`.

## Reporter options for replay

The following [reporter options](../../reporting.md) also apply to replays, allowing output to be displayed differently than the original run:

`--status-level <LEVEL>`
: Which test statuses to display during the replay. The default is `pass`. See [_Status levels_](../../reporting.md#status-levels) for valid values.

`--final-status-level <LEVEL>`
: Which test statuses to display at the end of the replay. The default is `fail`. See [_Status levels_](../../reporting.md#status-levels) for valid values.

`--failure-output <WHEN>`
: When to display output for failing tests. The default is `immediate`. Valid values: `immediate`, `final`, `immediate-final`, `never`.

`--success-output <WHEN>`
: When to display output for successful tests. The default is `never`. Valid values: `immediate`, `final`, `immediate-final`, `never`.

`--no-capture`
: Simulate no-capture mode. Since recorded output is already captured, this is a convenience option that sets `--success-output immediate`, `--failure-output immediate`, and `--no-output-indent`.

`--no-output-indent`
: Disable indentation for test output.

For example, outputs for successful tests are hidden by default. Use `cargo nextest replay --success-output immediate` to see those outputs.

Replays also work with [portable recordings](portable-recordings.md), which are self-contained archives that can be shared across machines. For example, `cargo nextest replay -R my-run.zip`.

## Options and arguments

### `cargo nextest replay`

=== "Summarized output"

    The output of `cargo nextest replay -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest replay -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest replay -h | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest replay --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest replay --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest replay --help | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

