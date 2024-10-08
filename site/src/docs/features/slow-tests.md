---
icon: material/speedometer-slow
---

# Slow tests and timeouts

Slow tests can bottleneck your test run. Nextest identifies tests that take more than a certain amount of time, and optionally lets you terminate tests that take too long to run.

## Slow tests

For tests that take more than a certain amount of time (by default 60 seconds), nextest prints out a **SLOW** status. For example, in the output below, `test_slow_timeout` takes 90 seconds to execute and is marked as a slow test.

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/slow-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/slow-output.ansi | ../scripts/strip-ansi.sh
    ```

## Configuring timeouts

To customize how long it takes before a test is marked slow, use the `slow-timeout` [configuration parameter](../configuration/index.md). For example, to set a timeout of 2 minutes before a test is marked slow, add this to `.config/nextest.toml`:

```toml title="Slow tests in <code>.config/nextest.toml</code>"
[profile.default]
slow-timeout = "2m"
```

Nextest uses the `humantime` parser: see [its documentation](https://docs.rs/humantime/latest/humantime/fn.parse_duration.html) for the full supported syntax.

## Terminating tests after a timeout

Nextest lets you optionally specify a number of `slow-timeout` periods after which a test is terminated. For example, to configure a slow timeout of 30 seconds and for tests to be terminated after 120 seconds (4 periods of 30 seconds), add this to `.config/nextest.toml`:

```toml title="Slow tests with termination"
[profile.default]
slow-timeout = { period = "30s", terminate-after = 4 }
```

### Example

The run below is configured with:

```toml
slow-timeout = { period = "1s", terminate-after = 2 }
```

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/timeout-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/timeout-output.ansi | ../scripts/strip-ansi.sh
    ```

### How nextest terminates tests

On Unix platforms, nextest creates a [process group] for each test. On timing out, nextest attempts a graceful shutdown: it first sends the [SIGTERM](https://www.gnu.org/software/libc/manual/html_node/Termination-Signals.html) signal to the process group, then waits for a grace period (by default 10 seconds) for the test to shut down. If the test doesn't shut itself down within that time, nextest sends SIGKILL (`kill -9`) to the process group to terminate it immediately.

To customize the grace period, use the `slow-timeout.grace-period` configuration setting. For example, with the `ci` profile, to terminate tests after 5 minutes with a grace period of 30 seconds:

```toml title="Termination grace period"
[profile.ci]
slow-timeout = { period = "60s", terminate-after = 5, grace-period = "30s" }
```

To send SIGKILL to a process immediately, without a grace period, set `slow-timeout.grace-period` to zero:

```toml title="Termination without a grace period"
[profile.ci]
slow-timeout = { period = "60s", terminate-after = 5, grace-period = "0s" }
```

<!-- md:version 0.9.61 --> The `slow-timeout.grace-period` setting is also applied to terminations due to Ctrl-C or other signals. With older versions, nextest always waits 10 seconds before sending SIGKILL.

#### Termination on Windows

On Windows, nextest terminates the test immediately in a manner akin to SIGKILL. (On Windows, nextest uses [job objects] to kill the test process and all its descendants.) The `slow-timeout.grace-period` configuration setting is ignored.

[process group]: https://en.wikipedia.org/wiki/Process_group
[job objects]: https://docs.microsoft.com/en-us/windows/win32/procthread/job-objects

## Per-test settings

Nextest supports [per-test settings](../configuration/per-test-overrides.md) for `slow-timeout` and `terminate-after`.

For example, some end-to-end tests might take longer to run and sometimes get stuck. For tests containing the substring `test_e2e`, to configure a slow timeout of 120 seconds, and to terminate tests after 10 minutes:

```toml title="Per-test slow timeouts"
[[profile.default.overrides]]
filter = 'test(test_e2e)'
slow-timeout = { period = "120s", terminate-after = 5 }
```

See [_Override precedence_](../configuration/per-test-overrides.md#override-precedence) for more about the order in which overrides are evaluated.
