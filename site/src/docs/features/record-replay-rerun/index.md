---
icon: material/history
status: experimental
description: Recording test runs, replaying them later, and rerunning test failures.
---

# Record, replay and rerun

<!-- md:version 0.9.123 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `[experimental]` with `record = true` to [`~/.config/nextest/config.toml`](../../user-config/index.md), or set `NEXTEST_EXPERIMENTAL_RECORD=1` in the environment
    - **Tracking issue:** TBD

Nextest supports recording test runs to rerun failing tests and to replay them later. Recorded runs are stored locally in the system cache.

Recorded test runs capture:

* Test statuses (pass, fail, etc) and durations.
* Outputs for all tests, both failing and successful. (If `--no-capture` is passed in at the time the run is recorded, test output cannot be captured.)

## Use cases

* Rerunning tests that failed or were not run in the past, with the goal being to iteratively converge towards a successful test run.
* Replaying test runs, including those that might have aged past terminal scrollback.

In the future, it will be possible to export replayed test runs in various formats such as JUnit and libtest-json output.

## Usage

To enable recording in [user configuration](../../user-config/index.md):

```toml title="Enabling recording in <code>~/.config/nextest/config.toml</code>"
[experimental]
record = true

[record]
enabled = true
```

Now, all future `cargo nextest run` instances will automatically be recorded.

## Learn more

- [Rerunning failed tests](rerun.md) — iteratively converge towards a successful test run.
- [Replaying test runs](replay.md) — replay recorded runs, including with different reporter options.
- [Managing recorded runs](managing-runs.md) — list, prune, export, and configure the record store.

## Configuration options

For a full list, see [_Record configuration_](../../user-config/reference.md#record-configuration).
