# Benchmarks

Nextest's [execution model](how-it-works.md) generally leads to faster test runs than Cargo. How much faster depends on the specifics, but here are some general guidelines:

* *Larger workspaces will see a greater benefit.* This is because larger workspaces have more crates, more test binaries, and more potential spots for bottlenecks. 
* *Test bottlenecks.* Nextest excels in situations where there are bottlenecks in multiple test binaries: cargo test can only run them serially, while nextest can run those tests in parallel.
* *Build caching.* Test runs are one component of end-to-end execution times. Speeding up the build by using [sccache](https://github.com/mozilla/sccache), the [Rust Cache GitHub Action](https://github.com/marketplace/actions/rust-cache), or similar, will make test run times be a proportionally greater part of overall times.

Even if nextest doesn't result in faster test builds, you may find doing occasional nextest runs useful for identifying test bottlenecks, for its user interface, or for its other features like test retries.

## Results

| Project         | Revision     | Test count | cargo test (s) | nextest (s) | Difference |
| --------------- | ------------ | ---------: | -------------: | ----------: | ---------: |
| [cargo-guppy]   | [`c135447a`] | 252        | 34.70          | 22.14       | \-36.2%    |
| [diem][^diem1]  | [`6025888b`] | 1476       | 1058.46        | 400.53      | \-62.1%    |
| [penumbra]      | [`44ab43f6`] | 32         | 54.66          | 24.90       | \-54.4%    |
| [ring]          | [`c14c355f`] | 179        | 17.64          | 11.60       | \-34.2%    |
| [rust-analyzer] | [`4449a336`] | 3746       | 6.76           | 5.23        | \-22.6%    |
| [tokio]         | [`e7a0da60`] | 1014       | 27.16          | 11.72       | \-56.8%    |

[cargo-guppy]: https://github.com/facebookincubator/cargo-guppy/
[`c135447a`]: https://github.com/facebookincubator/cargo-guppy/commit/c135447af716d0f985557b40042b2b6df53fa653

[diem]: https://github.com/diem/diem
[`6025888b`]: https://github.com/diem/diem/commit/6025888b264793bc2112d2ad3a6ef40f0861ee08

[^diem1]: Diem ships its own in-tree tool on top of [nextest-runner], so the commands were slightly different:
* the command for cargo test is `cargo xtest --unit`
* the command for running nextest is `cargo nextest --unit`

[penumbra]: https://github.com/penumbra-zone/penumbra
[`44ab43f6`]: https://github.com/penumbra-zone/penumbra/commit/44ab43f62bafa861608ac3f2e6deabb456c43983

[ring]: https://github.com/briansmith/ring
[`c14c355f`]: https://github.com/briansmith/ring/commit/c14c355f51c537c99ff43935c88c22c2e04980a3

[rust-analyzer]: https://github.com/rust-analyzer/rust-analyzer
[`4449a336`]: https://github.com/rust-analyzer/rust-analyzer/commit/4449a336f6965ebdfa9b7408e6ff40a6a990a43d

[tokio]: https://github.com/tokio-rs/tokio
[`e7a0da60`]: https://github.com/tokio-rs/tokio/commit/e7a0da60cd997f10b33f32c4763c8ecef01144f8

[nextest-runner]: https://crates.io/crates/nextest-runner

## Specifications

All measurements were done on:
* **Processor:** AMD Ryzen 9 3900x x86_64
* **Operating system:** Pop_OS! 21.04 running Linux kernel 5.15.15
* **RAM:** 64GB
* **Rust:** version 1.58.1

Lines of code were measured by `loc`, while the number of tests was recorded by nextest.

The commands run were:

* **cargo test:** `cargo test --workspace --bins --lib --tests` (to exclude doctests since they're not supported by nextest)
* **nextest:** `cargo nextest run --workspace`

The measurements do not include time taken to build the tests. To ensure that, each command was run 5 times in succession. The measurement recorded is the minimum of runs 3, 4 and 5.
