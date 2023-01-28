# Benchmarks

Nextest's [execution model](how-it-works.md) generally leads to faster test runs than Cargo. How much faster depends on the specifics, but here are some general guidelines:

* *Larger workspaces will see a greater benefit.* This is because larger workspaces have more crates, more test binaries, and more potential spots for bottlenecks. 
* *Bottlenecks with "long pole" tests.* Nextest excels in situations where there are bottlenecks in multiple test binaries: cargo test can only run them serially, while nextest can run those tests in parallel.
* *Build caching.* Test runs are one component of end-to-end execution times. Speeding up the build by using [sccache](https://github.com/mozilla/sccache), the [Rust Cache GitHub Action](https://github.com/marketplace/actions/rust-cache), or similar, will make test run times be a proportionally greater part of overall times.

Even if nextest doesn't result in faster test runs, you may find it useful for identifying test bottlenecks, for its user interface, or for its [other features](../README.md#features).

## Results

| Project         | Revision     | Test count | cargo test (s) | nextest (s) | Improvement |
| --------------- | ------------ | ---------: | -------------: | ----------: | ----------: |
| [crucible]      | [`cb228c2b`] | 483        | 5.14           | 1.52        | 3.38×       |
| [guppy]         | [`2cc51b41`] | 271        | 6.42           | 2.80        | 2.29×       |
| [mdBook]        | [`0079184c`] | 199        | 3.85           | 1.66        | 2.31×       |
| [meilisearch]   | [`bfb1f927`] | 721        | 57.04          | 28.99       | 1.96×       |
| [omicron]       | [`e7949cd1`] | 619        | 444.08         | 202.50      | 2.19×       |
| [penumbra]      | [`4ecd94cc`] | 144        | 125.38         | 90.96       | 1.37×       |
| [reqwest]       | [`3459b894`] | 113        | 5.57           | 2.26        | 2.48×       |
| [ring]          | [`450ada28`] | 179        | 13.12          | 9.40        | 1.39×       |
| [tokio]         | [`1f50c571`] | 1138       | 24.27          | 11.60       | 2.09×       |

[crucible]: https://github.com/oxidecomputer/crucible
[`cb228c2b`]: https://github.com/oxidecomputer/crucible/commit/cb228c2b0c29ac2acdea730b149cc70d41effcbf

[guppy]: https://github.com/guppy-rs/guppy
[`2cc51b41`]: https://github.com/guppy-rs/guppy/commit/2cc51b411fe7fec9df6d5f459d5ebb51ba357b9a

[mdbook]: https://github.com/rust-lang/mdBook
[`0079184c`]: https://github.com/rust-lang/mdBook/commit/0079184c16de0916b82e5b3785963f3ef3f505ff

[meilisearch]: https://github.com/meilisearch/meilisearch
[`bfb1f927`]: https://github.com/meilisearch/meilisearch/commit/bfb1f9279bc5648bc9b90109f92e91cb259c288a

[omicron]: https://github.com/oxidecomputer/omicron
[`e7949cd1`]: https://github.com/oxidecomputer/omicron/commit/e7949cd15e775d326ada59c23c933c1714784a31

[penumbra]: https://github.com/penumbra-zone/penumbra
[`4ecd94cc`]: https://github.com/penumbra-zone/penumbra/commit/4ecd94cce2d41427cc8d89693d745448e5253265

[reqwest]: https://github.com/seanmonstar/reqwest
[`3459b894`]: https://github.com/seanmonstar/reqwest/commit/3459b89488e293eaed9f3c413155e2dff3018093

[ring]: https://github.com/briansmith/ring
[`450ada28`]: https://github.com/briansmith/ring/commit/450ada288f1805795140097ec96396b890bcf722

[tokio]: https://github.com/tokio-rs/tokio
[`1f50c571`]: https://github.com/tokio-rs/tokio/commit/e7a0da60cd997f10b33f32c4763c8ecef01144f8

## Specifications

All measurements were done on:
* **Processor:** AMD Ryzen 9 7950X x86_64, 16 cores/32 threads
* **Operating system:** Pop_OS! 22.04 running Linux kernel 6.0.12
* **RAM:** 64GB
* **Rust:** version 1.66.0

The commands run were:

* **cargo test:** `cargo test --workspace --bins --lib --tests --examples --no-fail-fast` (to exclude doctests since they're not supported by nextest)
* **nextest:** `cargo nextest run --workspace --bins --lib --tests --examples --no-fail-fast`

The measurements do not include time taken to build the tests. To ensure that, each command was run 5 times in succession. The measurement recorded is the minimum of runs 3, 4 and 5.
