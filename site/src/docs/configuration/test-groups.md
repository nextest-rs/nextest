---
icon: material/set-none
---

# Test groups for mutual exclusion

<!-- md:version 0.9.48 -->

Nextest allows users to specify test _groups_ for sets of tests. This lets you configure groups of tests to run serially or with a limited amount of concurrency.

In other words, nextest lets you define logical _semaphores_ and _mutexes_ that apply to certain subsets of tests.

Tests that aren't part of a test group are not affected by these concurrency limits.

If the limit is set to 1, this is similar to `cargo test` with [the `serial_test` crate](https://crates.io/crates/serial_test), or a global mutex.

!!! warning "No support for in-process mutexes"

    With `cargo test`, some projects use in-process mutexes or semaphores for this purpose. Because nextest's execution model is process-per-test, does not support them directly. Instead, you can emulate them by using test groups.

## Use cases

- Your tests access a network service (perhaps running on the same system) that can only handle one, or a limited number of, tests being run at a time.
- Your tests run against a global system resource that may fail, or encounter race conditions, if accessed by more than one process at a time.
- Your tests start up a network service that listens on a fixed TCP or UDP port on localhost, and if several tests try to open up the same port concurrently, they'll collide with each other.

!!! info "Tests that open ports"

    While you can use test groups to make your existing network service tests work with nextest, **this is not the "correct" way to write such tests**. For example, your tests might collide with a network service already running on the system. The logical mutex will also make your test runs slower.

    Consider these two recommended approaches instead:

    - **Use a randomly assigned port.** On all platforms you can do this by binding to port 0. Once your test creates the service, you'll need a way to communicate the actual port assigned back to your test.
      - If your service is in the same process as your test, you can expose an API to retrieve the actual port assigned.
      - If your service is in another process, you'll need a way to communicate the port assigned back to the test. One approach is to pass in a temporary directory as an environment variable, then arrange for the service to write the port number in a file within the temporary directory.
    - **Rather than using TCP/IP, bind to a [Unix domain socket](https://en.wikipedia.org/wiki/Unix_domain_socket)** in a temporary directory. This approach also [works on Windows](https://devblogs.microsoft.com/commandline/windowswsl-interop-with-af_unix/).

## Configuring test groups

Test groups are specified in [nextest's configuration](index.md) by:

1. Declaring test group names along with concurrency limits, using the `max-threads` parameter.
2. Using the `test-groups` [per-test override](per-test-overrides.md).

For example:

```toml
[test-groups]
resource-limited = { max-threads = 4 }
serial-integration = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'test(resource_limited::)'
test-group = 'resource-limited'

[[profile.default.overrides]]
filter = 'package(integration-tests)'
platform = 'cfg(unix)'
test-group = 'serial-integration'
```

This configuration defines two test groups:

1. `resource-limited`, which is limited to 4 threads.
2. `serial-integration`, which is limited to 1 thread.

These test groups impact execution in the following ways:

1. Any tests whose name contains `resource_limited::` will be limited to running four at a time. In other words, there is a logical semaphore around all tests that contain `resource_limited::`, with four available permits.
2. On Unix platforms, tests in the `integration-tests` package will be limited to running one at a time, i.e. serially. In other words, on Unix platforms, there is a logical mutex around all tests in the `integration-tests` package.
3. Tests that are not in either of these groups will run with global concurrency limits.

Nextest will continue to schedule as many tests as possible, accounting for global and group concurrency limits.

## Showing test groups

You can show the test groups currently in effect with `cargo nextest show-config test-groups`.

With the above example, you might see the following output:

<pre>
<font color="#4E9A06"><b>    Finished</b></font> test [unoptimized + debuginfo] target(s) in 0.09s
group: <u style="text-decoration-style:single"><b>resource-limited</b></u> (max threads = <b>4</b>)
  * override for <b>default</b> profile with filter <font color="#C4A000">&apos;test(resource_limited::)&apos;</font>:
      <font color="#75507B"><b>resource-bindings</b></font>:
          <font color="#3465A4"><b>access::resource_limited::test_resource_access</b></font>
          <font color="#3465A4"><b>edit::resource_limited::test_resource_edit</b></font>
group: <u style="text-decoration-style:single"><b>serial-integration</b></u> (max threads = <b>1</b>)
  * override for <b>default</b> profile with filter <font color="#C4A000">&apos;package(integration-tests)&apos;</font>:
      <font color="#75507B"><b>integration-tests::integration</b></font>:
          <font color="#3465A4"><b>test_service_read</b></font>
          <font color="#3465A4"><b>test_service_write</b></font>
</pre>

This command accepts [all the same options](../listing.md#options-and-arguments) that `cargo nextest list` does.

## Comparison with `threads-required`

Test groups are similar to [heavy tests and `threads-required`](threads-required.md). The key difference is that test groups are meant to limit concurrency for subsets of tests, while `threads-required` sets global limits across the entire test run.

Both of these options can be combined. For example:

```toml
[test-groups]
my-group = { max-threads = 8 }

[[profile.default.overrides]]
filter = 'test(/^group::heavy::/)'
test-group = 'my-group'
threads-required = 2

[[profile.default.overrides]]
filter = 'test(/^group::light::/)'
test-group = 'my-group'
threads-required = 1  # this is the default, shown for clarity
```

With this configuration:

- Tests whose names start with `group::heavy::`, and tests that start with `group::light::`, are both part of `my-group`.
- The `group::heavy::` tests will take up two slots within _both_ global and group concurrency limits.
- The `group::light::` tests will take up one slot within both limits.

> **Note:** Setting `threads-required` to be greater than a test group's `max-threads` will not cause issues; a test that does so will take up all slots available.
