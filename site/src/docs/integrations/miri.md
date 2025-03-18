---
icon: octicons/shield-24
---

# Miri and nextest

Nextest works with the [Miri interpreter](https://github.com/rust-lang/miri) for Rust. This interpreter can check for certain classes of [undefined behavior](https://doc.rust-lang.org/reference/behavior-considered-undefined.html).
It can also run your tests for (almost) arbitrary targets.

## Benefits

The main benefit of using nextest with Miri is that each test runs [in its own process](../design/why-process-per-test.md). This has several advantages:

* Miri itself is single-threaded, so `cargo miri test`, which runs several tests in the same process, is also single-threaded. But nextest can run Miri tests in parallel.
* Each test gets a separate Miri context, which can make it easier to perform operations like [identifying memory leaks](https://github.com/rust-lang/miri/issues/1481).

Note, however, that `cargo miri test` is able to detect data races where two tests race on a shared resource. Miri with nextest will not detect such races.

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

```toml title="Miri configuration in <code>.config/nextest.toml</code>"
[profile.default-miri]
slow-timeout = { period = "60s", terminate-after = 2 }
```
