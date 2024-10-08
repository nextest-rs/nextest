---
icon: octicons/shield-24
---

# Miri and nextest

<!-- md:version 0.9.29 -->

Nextest works with the [Miri interpreter](https://github.com/rust-lang/miri) for Rust. This interpreter can check for certain classes of [undefined behavior](https://doc.rust-lang.org/reference/behavior-considered-undefined.html).
It can also run your tests for (almost) arbitrary targets.

## Benefits

The main benefit of using nextest with Miri is that each test runs in its own process. This means that it's easier to [identify memory leaks](https://github.com/rust-lang/miri/issues/1481), for example.

Miri can be very taxing on most computers. If nextest is run under Miri, it configures itself to use 1 thread by default. This mirrors `cargo miri test`. You can customize this with the `--test-threads`/`-j` option.

## Usage

After [installing Miri](https://github.com/rust-lang/miri#using-miri), run:

```
cargo miri nextest run
```

You may need to specify the toolchain to run as, using `cargo +nightly-YYYY-MM-DD miri nextest run`.

Miri supports cross-interpretation, so e.g. to run your tests on a big-endian target, run:

```
cargo miri nextest run --target mips64-unknown-linux-gnuabi64
```

This does not require installing any special toolchain, and will work even if you are using macOS or Windows.

> **Note:** [Archiving and reusing builds](../ci-features/archiving.md) is not supported under Miri.

## Configuring nextest running under Miri

If nextest detects a Miri environment, it uses the `default-miri` profile by default. Add repository-specific Miri configuration to this profile. For example, to [terminate tests](../features/slow-tests.md#terminating-tests-after-a-timeout) after 2 minutes, add this to `.config/nextest.toml`:

```toml title="Miri configuration in <code>.config/nextest.toml</code>
[profile.default-miri]
slow-timeout = { period = "60s", terminate-after = 2 }
```
