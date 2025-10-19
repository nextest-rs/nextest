---
icon: material/radar
description: Dynamically trace nextest events with DTrace and bpftrace.
---

# USDT probes

<!-- md:version 0.9.107 -->

Nextest defines [USDT](https://docs.rs/usdt/) probes which can be used for dynamic tracing. Probes are supported on:

* macOS, illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/)
* x86_64 Linux, via [bpftrace](https://bpftrace.org/) (aarch64 Linux might work as well)

## List of probes

The probes available in the current version of cargo-nextest, as well as the arguments available to them, are [listed in the nextest-runner documentation](https://docs.rs/nextest-runner/latest/nextest_runner/usdt).

For all probes, the first argument (`arg0`) contains JSON-encoded data. With DTrace, this data can be extracted via the [`json`](https://sysmgr.org/blog/2012/11/29/dtrace_and_json_together_at_last/) function. (bpftrace does not appear to have a facility for JSON extraction.)

Most probes also have commonly-used arguments available as `arg1`, `arg2`, and so on. For example, the `test-attempt-start` probe's `arg1` is the [binary ID](../running.md#binary-ids) (a string), and its `arg2` is the test name (a string).

The list of probes and their arguments are not part of nextest's [stability guarantees](../stability/index.md).

!!! note

    USDT probes and arguments are added as needed. If you'd like to add more probes to nextest, pull requests are welcome!

## Examples

To trace all test attempts as they finish, on Linux with bpftrace:

```sh
# Run this in another terminal before starting nextest.
# Replace /opt/cargo/bin/cargo-nextest with the output of `which cargo-nextest`.
sudo bpftrace -e 'usdt:/opt/cargo/bin/cargo-nextest:nextest:test-attempt-done { printf("%s\n", str(arg0)); }'
```

For more about bpftrace, see its [One-Liner Tutorial](https://bpftrace.org/tutorial-one-liners) and other documentation.

On macOS, illumos, or FreeBSD, with DTrace:

```sh
# Run this in another terminal before starting nextest.
# (On illumos, use pfexec instead of sudo.)
sudo dtrace -x strsize=512 -Z -n '*:cargo-nextest::test-attempt-done { printf("%s\n", copyinstr(arg0)); }'
```

For more about DTrace, check out the [Dynamic Tracing Guide](https://illumos.org/books/dtrace/preface.html#preface).

!!! tip "Consider using an LLM"

    If you're new to bpftrace and/or DTrace, your favorite LLM may be able to help you get started. We've had pretty good results with Claude Sonnet 4.5. (It does make mistakes, though, so consult the respective guides if something goes wrong.)
