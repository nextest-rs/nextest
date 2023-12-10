# Machine-readable output

cargo-nextest can be configured to produce machine-readable JSON output, readable by other programs. The [nextest-metadata crate](https://crates.io/crates/nextest-metadata) provides a Rust interface to deserialize the output to. (The same crate is used by nextest to generate the output.)

## Listing tests

To produce a list of tests using the JSON output, use `cargo nextest list --message-format json` (or `json-pretty` for nicely formatted output). Here's some example output for [camino](https://github.com/camino-rs/camino):

```json
% cargo nextest list --all-features --lib --message-format json-pretty
{
  "rust-build-meta": {
    "target-directory": "/home/rain/dev/camino/target",
    "base-output-directories": [
      "debug"
    ],
    "non-test-binaries": {},
    "build-script-out-dirs": {
      "camino 1.1.6 (path+file:///home/rain/dev/camino)": "debug/build/camino-02991de38c555ca1/out"
    },
    "linked-paths": [],
    "target-platforms": [
      {
        "triple": "x86_64-unknown-linux-gnu",
        "target-features": [
          "fxsr",
          "sse",
          "sse2"
        ]
      }
    ],
    "target-platform": null
  },
  "test-count": 5,
  "rust-suites": {
    "camino": {
      "package-name": "camino",
      "binary-id": "camino",
      "binary-name": "camino",
      "package-id": "camino 1.1.6 (path+file:///home/rain/dev/camino)",
      "kind": "lib",
      "binary-path": "/home/rain/dev/camino/target/debug/deps/camino-1bdca073ddd4474a",
      "build-platform": "target",
      "cwd": "/home/rain/dev/camino",
      "status": "listed",
      "testcases": {
        "serde_impls::tests::invalid_utf8": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "serde_impls::tests::valid_utf8": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "tests::test_borrowed_into": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "tests::test_deref_mut": {
          "ignored": false,
          "filter-match": {
            "status": "matches"
          }
        },
        "tests::test_owned_into": {
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

This is currently an experimental feature: see [Machine-readable output for test runs](run-machine-readable.md).

This is [currently not implemented](https://github.com/nextest-rs/nextest/issues/20). **Help wanted**: please post in the issue if you'd like to work on this!
