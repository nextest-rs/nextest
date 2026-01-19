---
icon: material/history
status: experimental
description: Recording test runs and replaying them later.
---

# Record and replay

<!-- md:version 0.9.123 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `[experimental]` with `record = true` to [`~/.config/nextest/config.toml`](../user-config/index.md), or set `NEXTEST_EXPERIMENTAL_RECORD=1` in the environment
    - **Tracking issue:** TBD

Nextest supports recording test runs to replay them later. Recorded runs are stored locally in the system cache.

Recorded test runs capture:

* Test statuses (pass, fail, etc) and durations.
* Outputs for all tests, both failing and successful. (If `--no-capture` is passed in at the time the run is recorded, test output cannot be captured.)

## Use cases

Currently, test runs can be replayed with `cargo nextest replay`. This is particularly useful for runs done in the past, including those that might have aged past terminal scrollback.

In the future, it will be possible to:

* publish archives in CI that can be replayed locally
* export replayed test runs in various formats such as JUnit and libtest-json output
* rerun tests that failed or were not run in the past, with the goal being to converge towards a successful test run

## Usage

Once the experimental `record` feature has been enabled in user config, you can enable recording in [user configuration](../user-config/index.md):

```toml title="Enabling recording in <code>~/.config/nextest/config.toml</code>"
[record]
enabled = true
```

Now, all future `cargo nextest run` instances will automatically be recorded.

### Replaying test runs

To replay the last test run, run `cargo nextest replay`. This will show output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/replay.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/replay.ansi | ../scripts/strip-ansi.sh
    ```

Earlier runs can be replayed by identifying them through their nextest run ID, with the `--run-id`/`-R` option to `cargo nextest replay`. Any unique prefix can be used; in colorized output, unique prefixes are highlighted in bold purple.

Replayed runs automatically use the [configured pager](../user-config/pager.md), such as `less`.

#### Reporter options for replay

The following [reporter options](../reporting.md) also apply to replays, allowing output to be displayed differently than the original run:

`--status-level <LEVEL>`
: Which test statuses to display during the replay. The default is `pass`. See [_Status levels_](../reporting.md#status-levels) for valid values.

`--final-status-level <LEVEL>`
: Which test statuses to display at the end of the replay. The default is `fail`. See [_Status levels_](../reporting.md#status-levels) for valid values.

`--failure-output <WHEN>`
: When to display output for failing tests. The default is `immediate`. Valid values: `immediate`, `final`, `immediate-final`, `never`.

`--success-output <WHEN>`
: When to display output for successful tests. The default is `never`. Valid values: `immediate`, `final`, `immediate-final`, `never`.

`--no-capture`
: Simulate no-capture mode. Since recorded output is already captured, this is a convenience option that sets `--success-output immediate`, `--failure-output immediate`, and `--no-output-indent`.

`--no-output-indent`
: Disable indentation for test output.

For example, outputs for successful tests are hidden by default. Use `cargo nextest replay --success-output immediate` to see those outputs.

### Listing recorded runs

To list recorded runs, run `cargo nextest store list`. This produces output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/store-list.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/store-list.ansi | ../scripts/strip-ansi.sh
    ```

The highlighted prefixes can be used to uniquely identify a test run. For example, with the above output, to replay the run ID starting with `9d032152`, run `cargo nextest replay -R 9d`.

## Record retention

Nextest applies limits to recorded runs to prevent the cache size from blowing up.

Retention limits are applied per-workspace. The default limits are:

- **Number**: A maximum of **100 runs** are stored.
- **Size**: Runs are stored until their compressed size exceeds **1 GB**.
- **Age**: Runs are stored for up to **30 days**.

These limits can be customized via the `[record]` section in [user configuration](../user-config/index.md).

The cache is pruned in least-recently-created (LRU) order, starting from the oldest runs.

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
| Linux and other Unix | `$XDG_CACHE_HOME/nextest/` or `~/.cache/nextest/` |
| macOS | `~/Library/Caches/nextest/` |
| Windows | `%LOCALAPPDATA%\nextest\` |

The store location can be overridden via the `NEXTEST_CACHE_DIR` environment variable.

Within the store, recorded runs are indexed by canonicalized workspace path.

## Configuration options

For a full list, see [_Record configuration_](../user-config/reference.md#record-configuration).

## Options and arguments

TODO
