# Miri and nextest

Nextest works with the [Miri interpreter](https://github.com/rust-lang/miri) for Rust. This interpreter can check for certain classes of undefined behavior.

## Benefits

The main benefit of using nextest with Miri is that each test runs in its own process. This means that it's easier to [identify memory leaks](https://github.com/rust-lang/miri/issues/1481).

Miri can be very taxing on most computers. If nextest is run under Miri, it configures itself to use 1 thread by default. This mirrors `cargo miri test`. You can customize this with the `--test-threads`/`-j` option.

## Requirements

- cargo-nextest 0.9.27 or above
- Miri from [master](https://github.com/rust-lang/miri) (a new Rust nightly with support for miri + nextest should come out soon)

## Usage

After [installing Miri](https://github.com/rust-lang/miri#using-miri), run:

```
cargo miri nextest run
```

You may need to specify the toolchain to run as, using `cargo +nightly-YYYY-MM-DD miri nextest run`.

## Configuring nextest running under Miri

If nextest detects a Miri environment, it uses the `default-miri` profile by default. Add repository-specific Miri configuration to this profile. For example, to [terminate tests](slow-tests.md#terminating-tests-after-a-timeout) after 2 minutes, add this to `.config/nextest.toml`:

```toml
[profile.default-miri]
slow-timeout = { period = "60s", terminate-after = 2 }
```
