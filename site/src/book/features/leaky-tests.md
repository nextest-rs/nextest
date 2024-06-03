---
icon: material/pipe-leak
---

# Leaky tests

Some tests create subprocesses but may not clean them up properly. Typical scenarios include:

- A test creates a server process to test against, but does not shut it down at the end of the test.
- A test starts a subprocess with the intent to shut it down, but panics, and does not use the [RAII pattern](https://doc.rust-lang.org/rust-by-example/scope/raii.html) to clean up subprocesses.
  - Note that `std::process::Child` [does **not** kill subprocesses](https://doc.rust-lang.org/std/process/struct.Child.html#warning) on being dropped. Some alternatives, such as `tokio::process::Command`, [can be configured](https://docs.rs/tokio/1/tokio/process/struct.Command.html#method.kill_on_drop) to do so.
- This can happen transitively as well: a test creates a process which creates its own subprocess, and so on.

Nextest can detect some, but not all, such situations. If nextest detects a subprocess leak, it marks the corresponding test as _leaky_.

## Leaky tests nextest detects

Currently, nextest is limited to detecting subprocesses that inherit standard output or standard error from the test. For example, here's a test that nextest will mark as leaky.

```rust
#[test]
fn test_subprocess_doesnt_exit() {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("120");
    cmd.spawn().unwrap();
}
```

For this test, nextest will output something like:

---

<pre><font color="#4E9A06"><b>    Starting</b></font> <b>1</b> tests across <b>8</b> binaries (<b>24</b> skipped)
<font color="#C4A000"><b>        LEAK</b></font> [   0.103s] <font color="#75507B"><b>nextest-tests::basic</b></font> <font color="#3465A4"><b>test_subprocess_doesnt_exit</b></font>
------------
<font color="#4E9A06"><b>     Summary</b></font> [   0.103s] <b>1</b> tests run: <b>1</b> <font color="#4E9A06"><b>passed</b></font> (<b>1</b> <font color="#C4A000"><b>leaky</b></font>), <b>24</b> <font color="#C4A000"><b>skipped</b></font>
</pre>

---

Leaky tests that are otherwise successful are considered to have passed.

## Leaky tests that nextest currently does not detect

Tests which spawn subprocesses that do not inherit either standard output or standard error are not currently detected by nextest. For example, the following test is not currently detected as leaky:

```rust
#[test]
fn test_subprocess_doesnt_exit_2() {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("120")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().unwrap();
}
```

Detecting such tests is a [very difficult problem to solve](https://github.com/oconnor663/duct.py/blob/master/gotchas.md#killing-grandchild-processes), particularly on Unix platforms.

> **Note:** This section is not part of nextest's [stability guarantees](../stability/index.md). In the future, these tests might get marked as leaky by nextest.

## Configuring the leak timeout

Nextest waits a specified amount of time (by default 100 milliseconds) after the test exits for standard output and standard error to be closed. In rare cases, you may need to configure the leak timeout.

To do so, use the `leak-timeout` [configuration parameter](../configuration/index.md). For example, to wait up to 500 milliseconds after the test exits, add this to `.config/nextest.toml`:

```toml
[profile.default]
leak-timeout = "500ms"
```

Nextest also supports [per-test overrides](../configuration/per-test-overrides.md) for the leak timeout.
