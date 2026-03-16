---
icon: material/database-cog-outline
description: Listing, pruning, and configuring recorded test runs.
---

# Managing recorded runs

## Listing recorded runs

To list recorded runs, run `cargo nextest store list`. This produces output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/store-list.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/store-list.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

As with reruns, highlighted prefixes can be used to uniquely identify a test run. For example, with the above output, to replay the run ID starting with `b0b38ba7`, run `cargo nextest replay -R b0b`.

Reruns are shown in a tree structure. Chains of reruns (e.g., with repeated `cargo nextest run -R latest` invocations) are shown linearly if possible.

### Detailed information about a run

For detailed information, run `cargo nextest store info <run-id>`. For example, `cargo nextest store info b0b`:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/store-info.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/store-info.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

For debugging purposes, runs capture all environment variables starting with `CARGO_` and `NEXTEST_`, other than ones ending with `_TOKEN` (since they may contain sensitive tokens).

## Record retention

Nextest applies limits to recorded runs to prevent the cache size from blowing up.

Retention limits are applied per-workspace. The default limits are:

- **Number**: A maximum of **100 runs** are stored.
- **Size**: Runs are stored until their compressed size exceeds **1 GB**.
- **Age**: Runs are stored for up to **30 days**.

These limits can be customized via the `[record]` section in [user configuration](../../user-config/index.md).

The cache is pruned in order of _last written at_, starting from the oldest runs. The last written at time is set at initial creation, and bumped on the original run when a recorded rerun is created.

### Example pruning configuration

```toml title="Custom pruning configuration in <code>~/.config/nextest/config.toml</code>"
[record]
enabled = true

# The maximum number of recorded runs.
max-records = 200
# The maximum total compressed size across recorded runs.
max-total-size = "2GB"
# The number of days to persist runs for.
max-age = "60d"
```

### Automatic pruning

The cache is automatically automatically pruned once a day at the end of test runs, if recording is enabled. The cache is also pruned if the number or size limits are exceeded by a factor of 1.5x or more.

### Manual pruning

To prune recorded runs manually, run `cargo nextest store prune`.

To see what would be pruned the next time, run `cargo nextest store prune --dry-run`.

### Store location

The store location is platform-dependent:

| Platform | Path |
|----------|------|
| Linux, macOS, and other Unix | `$XDG_STATE_HOME/nextest/` or `~/.local/state/nextest/` |
| Windows | `%LOCALAPPDATA%\nextest\` |

The store location can be overridden via the `NEXTEST_STATE_DIR` environment variable.

Within the store, recorded runs are indexed by canonicalized workspace path.

## Configuration options

For a full list, see [_Record configuration_](../../user-config/reference.md#record-configuration).

## Options and arguments

### `cargo nextest store list`

=== "Summarized output"

    The output of `cargo nextest store list -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store list -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store list -h | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest store list --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store list --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store list --help | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

### `cargo nextest store info`

=== "Summarized output"

    The output of `cargo nextest store info -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store info -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store info -h | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest store info --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store info --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store info --help | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

### `cargo nextest store prune`

=== "Summarized output"

    The output of `cargo nextest store prune -h`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store prune -h | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store prune -h | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

=== "Full output"

    The output of `cargo nextest store prune --help`:

    === "Colorized"

        ```bash exec="true" result="ansi"
        CLICOLOR_FORCE=1 cargo nextest store prune --help | ../scripts/strip-hyperlinks.sh
        ```

    === "Plaintext"

        ```bash exec="true" result="text"
        cargo nextest store prune --help | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
        ```

