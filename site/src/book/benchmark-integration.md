# Criterion benchmarks

Nextest supports running benchmarks in "test mode" with [Criterion.rs](https://bheisler.github.io/criterion.rs/book/index.html).

## What is test mode?

Many benchmarks depend on the system that's running them being [quiescent](https://en.wiktionary.org/wiki/quiescent). In other words, while benchmarks are being run there shouldn't be any other user or system activity. This can make benchmarks hard or even unsuitable to run in CI systems like GitHub Actions, where the capabilities of individual runners vary or are too noisy to produce useful results.

However, it can still be good to verify in CI that benchmarks compile correctly, and don't panic when run. To support this use case, libraries like Criterion support running benchmarks in "test mode".

For criterion and nextest, benchmarks are run with the following settings:

* With the `test` Cargo profile. This is typically the same as the `dev` profile, and can be overridden with `--cargo-profile`.
* With one iteration of the benchmark.

## Requirements

* Criterion 0.5.0 or above; previous versions are not compatible with nextest.
* Any recent version of cargo-nextest.

## Running benchmarks

By default, `cargo nextest run` does not include benchmarks as part of the test run. (This matches `cargo test`.)

To include benchmarks in your test run, use `cargo nextest run --all-targets`:

This will produce output that looks like:

<pre><font color="#D3D7CF">% </font><font color="#4E9A06">cargo</font> nextest run --all-targets
<font color="#4E9A06"><b>    Finished</b></font> test [unoptimized + debuginfo] target(s) in 0.05s
<font color="#4E9A06"><b>    Starting</b></font> <b>7</b> tests across <b>1</b> binaries
<font color="#4E9A06"><b>        PASS</b></font> [   0.368s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>depends_on_cache</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.404s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>depends_on</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.443s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>into_ids</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.520s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>make_graph</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.546s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>resolve_package</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.588s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>make_cycles</b></font>
<font color="#4E9A06"><b>        PASS</b></font> [   0.625s] <font color="#75507B"><b>my-benchmarks::bench/my_bench</b></font> <font color="#3465A4"><b>make_package_name</b></font>
------------
<font color="#4E9A06"><b>     Summary</b></font> [   0.626s] <b>7</b> tests run: <b>7</b> <font color="#4E9A06"><b>passed</b></font>, <b>0</b> <font color="#C4A000"><b>skipped</b></font>
</pre>

To run just benchmarks in test mode, use `cargo nextest run --benches`.
