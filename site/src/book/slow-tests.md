# Slow tests and timeouts

Slow tests can bottleneck your test run. Nextest identifies tests that take more than a certain amount of time, and optionally lets you terminate tests that take too long to run.

## Slow tests

For tests that take more than a certain amount of time (by default 60 seconds), nextest prints out a **SLOW** status. For example, in the output below, `test_slow_timeout` takes 90 seconds to execute and is marked as a slow test.

---

<pre><font color="#4E9A06"><b>    Starting</b></font> <b>6</b> tests across <b>8</b> binaries (<b>19</b> skipped)
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_success</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_success_should_panic</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::other</b></font> <font color="#3465A4"><b>other_test_success</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>       nextest-tests</b></font> <font color="#06989A">tests::</font><font color="#3465A4"><b>unit_test_success</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   1.501s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout_2</b></font>
<font color="#C4A000"><b>        SLOW</b></font> [&gt; 60.000s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [  90.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font>
------------
<font color="#4E9A06"><b>     Summary</b></font> [  90.002s] <b>6</b> tests run: <b>6</b> <font color="#4E9A06"><b>passed</b></font> (<b>1</b> <font color="#C4A000"><b>slow</b></font>), <b>19</b> <font color="#C4A000"><b>skipped</b></font>
</pre>

---

## Configuring timeouts

To customize how long it takes before a test is marked slow, you can use the `slow-timeout` [configuration parameter](configuration.md). For example, to set a timeout of 2 minutes before a test is marked slow, add this to `.config/nextest.toml`:

```toml
[profile.default]
slow-timeout = "2m"
```

Nextest uses the `humantime` parser: see [its documentation](https://docs.rs/humantime/latest/humantime/fn.parse_duration.html) for the full supported syntax.

## Terminating tests after a timeout

Nextest lets you optionally specify a timeout after which a test is terminated. For example, to configure a slow timeout of 60 seconds and for tests to be terminated after 3 minutes, add this to `.config/nextest.toml`:

```toml
[profile.default]
slow-timeout = { period = "60s", terminate-after = 3 }
```

`terminate-after` indicates the number of slow-timeout periods after which the test is terminated.

The run below is configured with:

```toml
slow-timeout = { period = "1s", terminate-after = 2 }
```

---

<pre><font color="#4E9A06"><b>    Starting</b></font> <b>5</b> tests across <b>8</b> binaries (<b>20</b> skipped)
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_success</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_success_should_panic</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>nextest-tests::other</b></font> <font color="#3465A4"><b>other_test_success</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.001s] <font color="#75507B"><b>       nextest-tests</b></font> <font color="#06989A">tests::</font><font color="#3465A4"><b>unit_test_success</b></font>
<font color="#C4A000"><b>        SLOW</b></font> [&gt;  1.000s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font>
<font color="#C4A000"><b>        SLOW</b></font> [&gt;  2.000s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font>
<font color="#CC0000"><b>     TIMEOUT</b></font> [   2.001s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font>

<font color="#CC0000"><b>--- STDOUT:              </b></font><font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_slow_timeout</b></font><font color="#CC0000"><b> ---</b></font>

running 1 test

------------
<font color="#CC0000"><b>     Summary</b></font> [   2.001s] <b>5</b> tests run: <b>4</b> <font color="#4E9A06"><b>passed</b></font>, <b>1</b> <font color="#CC0000"><b>timed out</b></font>, <b>20</b> <font color="#C4A000"><b>skipped</b></font>
</pre>

---

### How nextest terminates tests

On Unix platforms, nextest attempts a graceful shutdown: it first sends the [SIGTERM](https://www.gnu.org/software/libc/manual/html_node/Termination-Signals.html) signal to the test, then waits 10 seconds for it to shut down. If the test doesn't shut itself down within that time, nextest sends SIGKILL (`kill -9`) to the test to terminate it immediately.

On other platforms including Windows, nextest terminates the test immediately in a manner akin to SIGKILL.

> **Note:** The behavior described in this subsection is not part of the [stability guarantees](stability.md), and is subject to change.

## Per-test overrides

Nextest supports [per-test overrides](per-test-overrides.md) for the slow-timeout and terminate-after settings.

For example, some end-to-end tests might take longer to run and sometimes get stuck. For tests containing the substring `test_e2e`, to configure a slow timeout of 120 seconds, and to terminate tests after 10 minutes:

```toml
[[profile.default.overrides]]
filter = 'test(test_e2e)'
slow-timeout = { period = "120s", terminate-after = 5 }
```

See [Override precedence](per-test-overrides.md#override-precedence) for more about the order in which overrides are evaluated.
