---
icon: material/radar
description: Dynamically trace nextest events with DTrace and bpftrace.
---

# USDT probes

<!-- md:version 0.9.107 -->

Nextest defines [USDT](https://docs.rs/usdt/) probes which can be used for dynamic tracing. Probes are supported on:

* macOS, illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/).
* x86_64 Linux, via [bpftrace](https://bpftrace.org/) (aarch64 Linux might work as well).

## List of probes

Nextest's USDT probes, as well as the arguments available to them, are listed in the nextest-runner documentation:

* [Latest release](https://docs.rs/nextest-runner/latest/nextest_runner/usdt)
* [On main](https://nexte.st/rustdoc/nextest_runner/usdt/)

For all probes, the first argument (`arg0`) contains JSON-encoded data. With DTrace, this data can be extracted via the [`json`](https://sysmgr.org/blog/2012/11/29/dtrace_and_json_together_at_last/) function. (bpftrace does not appear to have a facility for JSON extraction.)

Most probes also have commonly-used arguments available as `arg1`, `arg2`, and so on. For example, the [`test-attempt-start` probe](https://nexte.st/rustdoc/nextest_runner/usdt/struct.UsdtTestAttemptStart)'s `arg1` is the [binary ID](../running.md#binary-ids) (a string), and its `arg2` is the test name (a string). For more information on these arguments, consult each probe's documentation.

The probes and their arguments are not part of nextest's [stability guarantees](../stability/index.md).

!!! note

    USDT probes and arguments are added as needed. If you'd like to add more probes to nextest, pull requests are welcome!

## Examples

(Run each example as root, in another terminal before starting nextest).

Trace all test attempts as they start, printing a JSON blob with information for each test:

=== "bpftrace"

    Replace `/opt/cargo/bin/cargo-nextest` with the output of `which cargo-nextest`.

    ```sh
    bpftrace -e 'usdt:/opt/cargo/bin/cargo-nextest:nextest:test-attempt-start { printf("%s\n", str(arg0)); }'
    ```

=== "DTrace"

    ```sh
    dtrace -x strsize=512 -Zn '*:cargo-nextest::test-attempt-start { printf("%s\n", copyinstr(arg0)); }'
    ```

Make a per-test-binary powers-of-two histogram of test execution times, with figures in milliseconds:

=== "bpftrace"

    Replace `/opt/cargo/bin/cargo-nextest` with the output of `which cargo-nextest`.

    ```sh
    # Here, arg1 is the binary ID, and arg4 is how long the test took in
    # nanoseconds. Dividing by 1_000_000 provides the time in milliseconds.
    bpftrace -e 'usdt:/opt/cargo/bin/cargo-nextest:nextest:test-attempt-done { @times[str(arg1)] = hist(arg4 / 1000000); }'
    ```

=== "DTrace"

    ```sh
    # Here, arg1 is the binary ID, and arg4 is how long the test took in
    # nanoseconds. Dividing by 1_000_000 provides the time in milliseconds.
    dtrace -Zn '*:cargo-nextest::test-attempt-done { @times[copyinstr(arg1)] = quantize(arg4 / 1000000); }'
    ```

## More information

* For more about bpftrace, see its [One-Liner Tutorial](https://bpftrace.org/tutorial-one-liners) and other documentation.
* For more about DTrace, check out the [Dynamic Tracing Guide](https://illumos.org/books/dtrace/preface.html#preface).

!!! tip "Consider using an LLM"

    If you're new to bpftrace and/or DTrace, your favorite LLM may be able to help you get started. We've had pretty good results with Claude Sonnet 4.5. (It does make mistakes, though, so consult the respective guides if something goes wrong.)
