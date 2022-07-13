# Machine-readable output

cargo-nextest can be configured to produce machine-readable JSON output, readable by other programs. The [nextest-metadata crate](https://crates.io/crates/nextest-metadata) provides a Rust interface to deserialize the output to. (The same crate is used by nextest to generate the output.)

## Listing tests

To produce a list of tests in the JSON output format `cargo nextest list --message-format json` (or `json-pretty` for nicely formatted output). Here's some example output for the [tokio repository](https://github.com/tokio-rs/tokio):

```json
% cargo nextest list -p tokio-util --features full --lib --message-format json-pretty
{
  "rust-build-meta": {
    "target-directory": "/home/rain/dev/tokio/target",
    "base-output-directories": [
      "debug"
    ],
    "non-test-binaries": {},
    "linked-paths": []
  },
  "test-count": 4,
  "rust-suites": {
    "tokio-util": {
      "package-name": "tokio-util",
      "binary-id": "tokio-util",
      "binary-name": "tokio-util",
      "package-id": "tokio-util 0.7.3 (path+file:///home/rain/dev/tokio/tokio-util)",
      "kind": "lib",
      "binary-path": "/home/me/dev/tokio/target/debug/deps/tokio_util-9dd5cbf268a3ffb4",
      "build-platform": "target",
      "cwd": "/home/me/dev/tokio/tokio-util",
      "status": "listed",
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
