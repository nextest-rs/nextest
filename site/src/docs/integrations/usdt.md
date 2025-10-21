---
icon: material/radar
description: Dynamically trace nextest events with DTrace and bpftrace.
---

# USDT probes

<!-- md:version 0.9.108 -->

Nextest defines [USDT](https://docs.rs/usdt/) probes which can be used for dynamic tracing. USDT probes are supported:

* On x86_64: Linux, via [bpftrace](https://bpftrace.org/)
* On aarch64: macOS, via [DTrace](https://dtrace.org/)
* On x86_64 and aarch64: illumos and other Solaris derivatives, and FreeBSD, via [DTrace](https://dtrace.org/)

## List of probes

Nextest's USDT probes, as well as the arguments available to them, are listed in the nextest-runner documentation:

* [Latest release](https://docs.rs/nextest-runner/latest/nextest_runner/usdt)
* [On main](https://nexte.st/rustdoc/nextest_runner/usdt/)

For all probes:

* The first argument (`arg0`) contains JSON-encoded data describing the event. With DTrace, this data can be extracted via the [`json`](https://sysmgr.org/blog/2012/11/29/dtrace_and_json_together_at_last/) function. (bpftrace does not appear to have a facility for JSON extraction.)

* The second argument (`arg1`) contains a globally unique string identifier for this kind of event, suitable for indexing into arrays.

  * For `run-*` events, `arg1` is the nextest run ID (a UUID).

  * For `test-attempt-*` events, `arg1` is a test attempt ID, comprised of:
    * the nextest run ID
    * the [binary ID](../running.md#binary-ids)
    * the test name
    * the stress index if this is a [stress run](../features/stress-tests.md)
    * and the current attempt, if the test is being retried.

  * For `setup-script-*` events, `arg1` is comprised of:
    * the nextest run ID
    * the name of the setup script
    * the stress index if this is a [stress run](../features/stress-tests.md)

Most probes also have commonly-used arguments available as `arg2`, `arg3`, and so on. For example, for the [`test-attempt-start` probe](https://nexte.st/rustdoc/nextest_runner/usdt/struct.UsdtTestAttemptStart):

* `arg2` is the [binary ID](../running.md#binary-ids) (a string)
* `arg3` is the test name (a string)
* `arg4` is the process ID of the test process

For more information on these arguments, consult each probe's documentation.

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

    Here, `arg2` is the binary ID, and `arg5` is how long the test took in
    nanoseconds. Dividing by 1 000 000 provides the time in milliseconds.

    ```sh
    bpftrace -e 'usdt:/opt/cargo/bin/cargo-nextest:nextest:test-attempt-done { @times[str(arg2)] = hist(arg5 / 1000000); }'
    ```

=== "DTrace"

    Here, `arg2` is the binary ID, and `arg5` is how long the test took in
    nanoseconds. Dividing by 1 000 000 provides the time in milliseconds.

    ```sh
    dtrace -Zn '*:cargo-nextest::test-attempt-done { @times[copyinstr(arg2)] = quantize(arg5 / 1000000); }'
    ```

## More information

* For more about bpftrace, see its [One-Liner Tutorial](https://bpftrace.org/tutorial-one-liners) and other documentation.
* For more about DTrace, check out the [Dynamic Tracing Guide](https://illumos.org/books/dtrace/preface.html#preface).

!!! tip "Consider using an LLM"

    If you're new to bpftrace and/or DTrace, your favorite LLM may be able to help you get started. We've had pretty good results with Claude Sonnet 4.5. (It does make mistakes, though, so consult the respective guides if something goes wrong.)
