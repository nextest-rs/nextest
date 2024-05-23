# Machine-readable listings

Nextest provides machine-readable listings in two formats:

1. As lists of tests
2. As lists of test binaries

## Machine-readable test lists

To produce a list of tests as JSON, run:

```
cargo nextest list --message-format json
```

Specify `--message-format json-pretty` for formatted output.

### Parsing nextest's output

If parsing output in Rust, use [nextest-metadata's `TestListSummary`](https://docs.rs/nextest-metadata/latest/nextest_metadata/struct.TestListSummary.html). This is the library nextest itself uses to generate output, and will always be in sync.

A JSON schema is not currently available, but is planned to be.

## Machine-readable binary lists

In some cases, you may wish to avoid running test binaries. For example:

- You're cross-compiling tests and a [target runner](target-runners.md) is not available.
- You will perform operations on test binaries after building and before running tests.
- You have to roll your own version of nextest's [archive feature](reusing-builds.md).

In these cases, nextest can provide the list of test binaries as JSON, without executing them to find the list of test instances. Run:

```
cargo nextest list --list-type binaries-only --message-format json
```

Specify `--message-format json-pretty` for formatted output.

## Examples

Here's some example output for [camino](https://github.com/camino-rs/camino). Below, the value of `"package-id"` can be matched up to the package IDs produced by running `cargo metadata`.

### Example test list

```json
% cargo nextest list --all-features --lib --message-format json-pretty
{
  "rust-build-meta": {
    "target-directory": "/home/user/dev/camino/target",
    "base-output-directories": [
      "debug"
    ],
    "non-test-binaries": {},
    "build-script-out-dirs": {
      "path+file:///home/user/dev/camino#1.1.7": "debug/build/camino-3e59e0a4294df039/out"
    },
    "linked-paths": [],
    "platforms": {
      "host": {
        "platform": {
          "triple": "x86_64-unknown-linux-gnu",
          "target-features": [
            "fxsr",
            "sse",
            "sse2"
          ]
        },
        "libdir": {
          "status": "available",
          "path": "/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib"
        }
      },
      "targets": []
    },
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
      "package-id": "path+file:///home/user/dev/camino#1.1.7",
      "kind": "lib",
      "binary-path": "/home/user/dev/camino/target/debug/deps/camino-5be71433c290cfc5",
      "build-platform": "target",
      "cwd": "/home/user/dev/camino",
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

### Example binary list

```json
% cargo nextest list --all-features --lib --list-type binaries-only --message-format json-pretty
{
  "rust-build-meta": {
    "target-directory": "/home/rain/dev/camino/target",
    "base-output-directories": [
      "debug"
    ],
    "non-test-binaries": {},
    "build-script-out-dirs": {
      "path+file:///home/rain/dev/camino#1.1.7": "debug/build/camino-3e59e0a4294df039/out"
    },
    "linked-paths": [],
    "platforms": {
      "host": {
        "platform": {
          "triple": "x86_64-unknown-linux-gnu",
          "target-features": [
            "fxsr",
            "sse",
            "sse2"
          ]
        },
        "libdir": {
          "status": "available",
          "path": "/home/rain/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib"
        }
      },
      "targets": []
    },
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
  "rust-binaries": {
    "camino": {
      "binary-id": "camino",
      "binary-name": "camino",
      "package-id": "path+file:///home/rain/dev/camino#1.1.7",
      "kind": "lib",
      "binary-path": "/home/rain/dev/camino/target/debug/deps/camino-5be71433c290cfc5",
      "build-platform": "target"
    }
  }
}
```
