---
icon: material/chart-scatter-plot
sidebar_icon: false
description: Running Criterion.rs benchmarks in test mode with nextest.
---

# Criterion benchmarks

Nextest supports running benchmarks in "test mode" with [Criterion.rs](https://bheisler.github.io/criterion.rs/book/index.html).

!!! note "Running benchmarks to measure performance"

    <!-- md:version 0.9.117 --> This page is about running benchmarks as regular tests via `cargo nextest run`. Nextest also has an experimental `cargo nextest bench` command which runs benchmarks to measure performance. For more, see [_Running benchmarks_](../features/benchmarks.md).

## What is test mode?

Many benchmarks depend on the system that's running them being [quiescent](https://en.wiktionary.org/wiki/quiescent). In other words, while benchmarks are being run there shouldn't be any other user or system activity. This can make benchmarks hard or even unsuitable to run in CI systems like GitHub Actions, where the capabilities of individual runners vary or are too noisy to produce useful results.

However, it can still be good to verify in CI that benchmarks compile correctly, and don't panic when run. To support this use case, libraries like Criterion support running benchmarks in "test mode".

For criterion and nextest, benchmarks are run with the following settings:

- With the `test` Cargo profile. This is typically the same as the `dev` profile, and can be overridden with `--cargo-profile`.
- With one iteration of the benchmark.

## Requirements

- Criterion 0.5.0 or above; previous versions are not compatible with nextest.
- Any recent version of cargo-nextest.

## Running benchmarks

By default, `cargo nextest run` does not include benchmarks as part of the test run. (This matches `cargo test`.)

To include benchmarks in your test run, use `cargo nextest run --all-targets`.

This will produce output that looks like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/criterion-output.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/criterion-output.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

To run just benchmarks in test mode, use `cargo nextest run --benches`.
