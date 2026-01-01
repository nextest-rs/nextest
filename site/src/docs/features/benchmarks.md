---
icon: material/chart-bar
status: experimental
description: Running benchmarks with cargo nextest bench.
---

# Running benchmarks

<!-- md:version 0.9.117 -->

!!! experimental "Experimental: This feature is not yet stable"

    - **Enable with:** Add `experimental = ["benchmarks"]` to `.config/nextest.toml`, or set `NEXTEST_EXPERIMENTAL_BENCHMARKS=1` in the environment
    - **Tracking issue:** [#2874](https://github.com/nextest-rs/nextest/issues/2874)

Nextest supports running benchmarks with the `cargo nextest bench` command. Using `cargo nextest bench` can be helpful in case your benchmarks require [setup](../configuration/setup-scripts.md) or [wrapper](../configuration/wrapper-scripts.md) scripts.

Supported benchmark harnesses include:

* [Criterion.rs](https://bheisler.github.io/criterion.rs/book/index.html)
* The default libtest benchmark runner, in nightly Rust.
* Any other benchmark harnesses that follow the [custom test harness](../design/custom-test-harnesses.md) protocol.

!!! note

    This page is about running benchmarks to measure performance. Benchmarks can also be run in test mode as regular tests via `cargo nextest run`, without requiring an experimental feature flag. For more, see [_Criterion benchmarks_](../integrations/criterion.md).

## Benchmark-specific settings

Since benchmarks typically take longer than tests to run, nextest applies a different set of [slow](slow-tests.md) and [global](slow-tests.md#setting-a-global-timeout) timeouts for them. Access these via the `bench.slow-timeout` and `bench.global-timeout` settings, respectively.

```toml title="Setting timeouts for benchmarks"
[profile.default]
# Set a global timeout of 2 hours for benchmarks.
bench.global-timeout = "2h"

[profile.default.overrides]
# Terminate this benchmark after 10 minutes.
filter = "test(bench_commands)"
bench.slow-timeout = { period = "60s", terminate-after = 10 }
```

The regular `slow-timeout` and `global-timeout` settings are ignored for benchmarks.

## Options and arguments

TODO
