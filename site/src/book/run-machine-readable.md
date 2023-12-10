# Machine-readable output for test runs

- **Nextest version:** 0.9.66 and above
- **Enable with:** Set `NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1` in the environment
- **Tracking issue:** [#1152](https://github.com/nextest-rs/nextest/issues/1152)

Nextest has experimental support for producing machine-readable output for test runs, in a format similar to `cargo test`'s [libtest JSON output](https://github.com/rust-lang/rust/issues/49359).

The upstream libtest JSON format is currently unstable as of 2023-12. However, nextest's stabilization of the format is not gated on the upstream format being stabilized.

The implementation, and this documentation, is a work in progress.

## Usage

Pass in the `--message-format` option:

```
NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 cargo nextest run --message-format <format>
```

The `<format>` can be any of:

- `libtest-json`: Produce output similar to the unstable libtest JSON.
- `libtest-json-plus`: Produce libtest JSON output, along with an extra `nextest` field.

In addition, the version of the format can be specified via the `--message-format-version <version>` option. Supported values for `<version>` are:

- `0.1`: The unstable libtest JSON format as of 2023-12.

## Format specification

TODO

## Stability policy

**While this is an experimental feature:** The format may be changed to fix issues or track upstream changes. Changes will be documented in the [changelog](../CHANGELOG.md).

**After this format is stabilized in nextest:** If the unstable libtest JSON format changes, this will be accompanied with a bump in the version number. For example, the next unstable format will be called `0.2`. Once the libtest JSON format is stabilized, the corresponding format version in nextest will be `1` or `1.0`.
