# Machine-readable output

cargo-nextest can be configured to produce machine-readable JSON output, readable by other programs. The [nextest-metadata crate](https://crates.io/crates/nextest-metadata) provides a Rust interface to deserialize the output to. (The same crate is used by nextest to generate the output.)

## Listing tests

To produce a list of tests in the JSON output format `cargo nextest list --message-format json` (or `json-pretty` for nicely formatted output). Here's some example output for the [tokio repository](https://github.com/tokio-rs/tokio):

```json
% cargo nextest list -p tokio-util --features full --lib --message-format json-pretty
{
  "test-count": 4,
  "rust-build-meta": {
    "target-directory": "/home/rain/dev/tokio/target",
    "base-output-directories": [
      "debug"
    ],
    "linked-paths": []
  },
  "rust-suites": {
    "tokio-util": {
      "package-name": "tokio-util",
      "binary-name": "tokio-util",
      "package-id": "tokio-util 0.7.0 (path+file:///home/me/dev/tokio/tokio-util)",
      "binary-path": "/home/me/dev/tokio/target/debug/deps/tokio_util-def0ee51cb418fe8",
      "cwd": "/home/rain/dev/tokio/tokio-util",
      "build-platform": "target",
      "testcases": {
        "either::tests::either_is_async_read": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "either::tests::either_is_stream": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "time::wheel::level::test::test_slot_for": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "time::wheel::test::test_level_for": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        }
      }
    }
  }
}
```

The value of `"package-id"` can be matched up to the package IDs produced by running `cargo metadata`.

## Running tests

This is [currently not implemented](https://github.com/nextest-rs/nextest/issues/20), but will be implemented in the near future.
