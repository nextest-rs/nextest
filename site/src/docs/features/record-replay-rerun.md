---
icon: material/history
status: experimental
description: Recording test runs, replaying them later, and rerunning test failures.
---

# Record, replay and rerun

<!-- md:version 0.9.123 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `[experimental]` with `record = true` to [`~/.config/nextest/config.toml`](../user-config/index.md), or set `NEXTEST_EXPERIMENTAL_RECORD=1` in the environment
    - **Tracking issue:** TBD

Nextest supports recording test runs to rerun failing tests and to replay them later. Recorded runs are stored locally in the system cache.

Recorded test runs capture:

* Test statuses (pass, fail, etc) and durations.
* Outputs for all tests, both failing and successful. (If `--no-capture` is passed in at the time the run is recorded, test output cannot be captured.)

## Use cases

* Rerunning tests that failed or were not run in the past, with the goal being to iteratively converge towards a successful test run.
* Replaying test runs, including those that might have aged past terminal scrollback.

In the future, it will be possible to:

* Publish archives in CI that can be replayed locally.
* Export replayed test runs in various formats such as JUnit and libtest-json output.

## Usage

To enable recording in [user configuration](../user-config/index.md):

```toml title="Enabling recording in <code>~/.config/nextest/config.toml</code>"
[experimental]
record = true

[record]
enabled = true
```

Now, all future `cargo nextest run` instances will automatically be recorded.

## Rerunning failed tests

When the recording feature is enabled, you can rerun failing tests with `cargo nextest run -R latest`. This command will run tests that, in the original run:

- failed;
- did not run because the test run was cancelled; or,
- were not previously seen, typically because they were newly added since the original run.

!!! tip "Rerun build scope"

    Without any further arguments, `cargo nextest run -R latest` will build the same targets that the original run did. If build scope arguments are specified, they will override the set of build targets from the original run.
    
    Build scope arguments include all arguments under the _Package selection_, _Target selection_, and _Feature selection_ headings of `cargo nextest run --help`.

### Example rerun flow

Let's say that `cargo nextest run --package nextest-filtering` was run, and it had two failing tests:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-original-run.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-original-run.ansi | ../scripts/strip-ansi.sh
    ```

---

With `cargo nextest run -R latest proptest_helpers`, the first test is selected:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

All selected tests passed, but some outstanding (previously-failing) tests still remain, so nextest exits with the advisory exit code 5 ([`RERUN_TESTS_OUTSTANDING`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.RERUN_TESTS_OUTSTANDING)).

---

A subsequent `cargo nextest run -R latest` will run the remaining test:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest-2.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest-2.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

!!! note "Exit code for no tests in a rerun"

    In regular runs, if there are no tests to run, nextest exits with the advisory exit code 4 ([`NO_TESTS_RUN`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.NO_TESTS_RUN)) by default.
    
    With reruns, if there are no tests to run, nextest exits with exit code 0 by default, indicating success. The difference in behavior is due to the goal of reruns being to converge to a successful test run.

---

It is possible to rewind the rerun logic to an earlier state by passing in a run ID to `-R`. In this case `b0b` forms an unambiguous prefix (highlighted in bold purple), so `cargo nextest run -R b0b` results in both tests being run:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest-3.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest-3.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

### Rerun heuristics

Picking the set of tests to run is tricky, particularly in the face of tests being removed and new ones being added. We have attempted to pick a strategy that aims to be conservative while covering the most common use cases, but it is always possible that tests are missed. Because of this, and because code changes might include regressions in previously passing tests, it is recommended that you perform a full test run once your iterations are complete.

As a best practice, it is also recommended that you use CI to gate changes making their way to production, and that you perform full runs in CI.

A design document discussing the heuristics and considerations involved is forthcoming.

## Replaying test runs

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

### Reporter options for replay

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

## Listing recorded runs

To list recorded runs, run `cargo nextest store list`. This produces output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/store-list.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/store-list.ansi | ../scripts/strip-ansi.sh
    ```

As with reruns, highlighted prefixes can be used to uniquely identify a test run. For example, with the above output, to replay the run ID starting with `b0b38ba7`, run `cargo nextest replay -R b0b`.

Reruns are shown in a tree structure. Chains of reruns (e.g., with repeated `cargo nextest run -R latest` invocations) are shown linearly if possible.

### Detailed information about a run

For detailed information, run `cargo nextest store info <run-id>`. For example, `cargo nextest store info b0b`:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/store-info.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/store-info.ansi | ../scripts/strip-ansi.sh
    ```
    
For debugging purposes, runs capture all environment variables starting with `CARGO_` and `NEXTEST_`, other than ones ending with `_TOKEN` (since they may contain sensitive tokens).

## Record retention

Nextest applies limits to recorded runs to prevent the cache size from blowing up.

Retention limits are applied per-workspace. The default limits are:

- **Number**: A maximum of **100 runs** are stored.
- **Size**: Runs are stored until their compressed size exceeds **1 GB**.
- **Age**: Runs are stored for up to **30 days**.

These limits can be customized via the `[record]` section in [user configuration](../user-config/index.md).

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
| Linux and other Unix | `$XDG_CACHE_HOME/nextest/` or `~/.cache/nextest/` |
| macOS | `~/Library/Caches/nextest/` |
| Windows | `%LOCALAPPDATA%\nextest\` |

The store location can be overridden via the `NEXTEST_CACHE_DIR` environment variable.

Within the store, recorded runs are indexed by canonicalized workspace path.

## Configuration options

For a full list, see [_Record configuration_](../user-config/reference.md#record-configuration).

## Options and arguments

TODO
