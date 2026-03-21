---
icon: material/history
status: experimental
description: Recording test runs, replaying them later, and rerunning test failures.
render_macros: false
---

# Record, replay and rerun

<!-- md:version 0.9.123 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `[experimental]` with `record = true` to [`~/.config/nextest/config.toml`](../../user-config/index.md), or set `NEXTEST_EXPERIMENTAL_RECORD=1` in the environment
    - **Tracking issue:** TBD

Nextest supports recording test runs to rerun failing tests and to replay them later. Recorded runs are stored locally in the system cache.

Recordings can also be exported from CI as portable archives, and visualized as Perfetto traces. For a full list of what you can do with recordings, see [_Learn more_](#learn-more) below.

Recorded test runs capture:

* Test statuses (pass, fail, etc) and durations.
* Outputs for all tests, both failing and successful. (If `--no-capture` is passed in at the time the run is recorded, test output cannot be captured.)

## Setting up run recording

Run recording can be enabled locally or in CI.

### Enabling run recording locally

To enable recording in [user configuration](../../user-config/index.md):

```toml title="Enabling recording in <code>~/.config/nextest/config.toml</code>"
[experimental]
record = true

[record]
enabled = true
```

Now, all future `cargo nextest run` instances will automatically be recorded.

### Enabling run recording in GitHub Actions

For GitHub Actions, the following recipe sets up recording, then uploads the resulting [portable recording](portable-recordings.md) as a [workflow artifact](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts):

```yaml
# Set up recording for the local nextest run.
- name: Create recording user config
  shell: bash
  run: |
    mkdir -p "$RUNNER_TEMP/nextest-config"
    printf '[experimental]\nrecord = true\n\n[record]\nenabled = true\n' \
      > "$RUNNER_TEMP/nextest-config/config.toml"
    cat "$RUNNER_TEMP/nextest-config/config.toml"

- name: Run tests
  shell: bash
  env:
    NEXTEST_STATE_DIR: ${{ runner.temp }}/nextest-state
  run: cargo nextest run --profile ci --user-config-file "$RUNNER_TEMP/nextest-config/config.toml"
  
- name: Create portable archive from recorded run
  # Run this step even if the test step fails.
  if: "!cancelled()"
  env:
    NEXTEST_STATE_DIR: ${{ runner.temp }}/nextest-state
  shell: bash
  run: |
    cargo nextest store export latest \
      --user-config-file "$RUNNER_TEMP/nextest-config/config.toml" \
      --archive-file "$RUNNER_TEMP/nextest-run-archive.zip"

- name: Upload portable recording
  # Run this step even if the test step fails.
  if: "!cancelled()"
  uses: actions/upload-artifact@v7.0.0
  with:
    path: ${{ runner.temp }}/nextest-run-archive.zip
    archive: false
```

### Enabling run recording in other CI systems

Follow this general pattern:

```bash
# Create a user config which enables recording.
mkdir -p "$TMPDIR/nextest-config"
printf '[experimental]\nrecord = true\n\n[record]\nenabled = true\n' \
  > "$TMPDIR/nextest-config/config.toml"
cat "$TMPDIR/nextest-config/config.toml"

# Run tests with this user config file.
cargo nextest run --profile ci --user-config-file "$TMPDIR/nextest-config/config.toml"

# Export the recording.
cargo nextest store export latest \
  --user-config-file "$TMPDIR/nextest-config/config.toml" \
  --archive-file "$TMPDIR/nextest-run-archive.zip"
```

Then, post-process or upload `$TMPDIR/nextest-run-archive.zip` as supported by your CI system.

## Learn more

- [_Rerunning failed tests_](rerun.md) — iteratively converge towards a successful test run.
- [_Replaying test runs_](replay.md) — replay recorded runs, including with different reporter options.
- [_Portable recordings_](portable-recordings.md) — export and share recordings across machines.
- [_Perfetto traces_](perfetto-chrome-traces.md) — visualize and analyze test runs.
- [_Managing recorded runs_](managing-runs.md) — list, prune, and configure the record store.

## Configuration options

For a full list, see [_Record configuration_](../../user-config/reference.md#record-configuration).
