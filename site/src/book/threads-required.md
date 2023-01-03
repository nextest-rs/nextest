# Heavy tests and `threads-required`

Nextest achieves [its performance](benchmarks.md) through running [many tests in parallel](how-it-works.md). However, some projects have tests that consume a disproportionate amount of resources like CPU or memory. If too many of these *heavy tests* are run concurrently, your machine's CPU might be overwhelmed, or it might run out of memory.

With nextest, you can mark heavy tests as taking up multiple threads or "slots" out of the total amount of available parallelism. In other words, you can assign those tests a higher "weight". This is done by using the `threads-required` [per-test override](per-test-overrides.md).

For example, on a machine with 16 logical CPUs, nextest will run 16 tests concurrently by default. However, if you mark tests that begin with `tests::heavy::` as requiring 2 threads each:

```toml
[[profile.default.overrides]]
filter = 'test(/^tests::heavy::/)'
threads-required = 2
```

Then each test in the `tests::heavy` module will take up 2 of those 16 threads.

The `threads-required` configuration can also be set to one of two special values:

* `"num-cpus"` — The number of logical CPUs on the system.
* `"num-test-threads"` — The number of test threads nextest is currently running with.

> NOTE: `threads-required` is not meant to ensure mutual exclusion across sets of tests. To do so, see [Test groups and mutual exclusion](test-groups.md).

## Use cases

Some use cases that may benefit from limiting concurrency:

- Integration tests that spin up a network of services to run against.
- Tests that are multithreaded internally, possibly using a [custom test harness](custom-test-harnesses.md) where a single test is presented to nextest.
- Tests that consume large amounts of memory.

> **Tip:** Be sure to benchmark your test runs! `threads-required` will often cause test runs to become slower overall. However, setting it might still be desirable if it makes test runs more reliable.
