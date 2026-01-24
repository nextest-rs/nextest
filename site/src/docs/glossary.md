---
icon: material/book-outline
description: Definitions of key terms and identifiers used throughout nextest.
---

# Glossary

This page defines key terms and identifiers used throughout nextest.

## Run ID

A run ID is a [UUID](https://en.wikipedia.org/wiki/Universally_Unique_Identifier) that uniquely identifies a single invocation of `cargo nextest run`. Every test and setup script executed within that run shares the same run ID.

Run IDs are useful for:

- Correlating logs and events across tests within the same run.
- Distinguishing between different test runs in external systems.

The run ID is available:

- During [`cargo nextest run`](running.md), at the beginning of the test run. For example:

  ```bash exec="true" result="ansi"
  cat src/outputs/run-id-example.ansi | ../scripts/strip-hyperlinks.sh
  ```

- Via the [`NEXTEST_RUN_ID`](configuration/env-vars.md#environment-variables-nextest-sets) environment variable.
- In [JUnit output](machine-readable/junit.md), via the `uuid` attribute on the root `testsuites` element.
- <!-- md:version 0.9.108 --> In [USDT probes](integrations/usdt.md), through `arg1` for `run-*` events, and also JSON-encoded in `arg0` as the `run_id` field.

## Binary ID

A binary ID uniquely identifies a test binary within a Cargo workspace. The format depends on the type of binary:

| Binary type                    | Format                       | Example                    |
|--------------------------------|------------------------------|----------------------------|
| Unit test (from `lib.rs`)      | `crate-name`                 | `my-crate`                 |
| Integration test               | `crate-name::bin-name`       | `my-crate::integration`    |
| Other (benchmark, example, etc.) | `crate-name::kind/bin-name` | `my-crate::bench/perf`     |

For more about unit and integration tests, see [the documentation for `cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html).

The binary ID is available:

- During [`cargo nextest run`](running.md), as the first part of each test execution line. For example:

  ```bash exec="true" result="ansi"
  cat src/outputs/binary-id-run-example.ansi | ../scripts/strip-hyperlinks.sh
  ```

- In [`cargo nextest list`](listing.md) output, as section headings. For example:

  ```bash exec="true" result="ansi"
  cat src/outputs/binary-id-list-example.ansi | ../scripts/strip-hyperlinks.sh
  ```

- <!-- md:version 0.9.116 --> Via the [`NEXTEST_BINARY_ID`](configuration/env-vars.md#environment-variables-nextest-sets) environment variable.

- In [JUnit output](machine-readable/junit.md), via the `name` attribute on each `testsuite` element.

- <!-- md:version 0.9.108 --> In [USDT probes](integrations/usdt.md), JSON-encoded in `arg0` as the `binary_id` field.

Within [filtersets](filtersets/index.md), binary IDs can be selected via the [`binary_id` predicate](filtersets/reference.md#basic-predicates).

## Attempt ID

An attempt ID uniquely identifies a single execution attempt of a test. When tests are [retried](features/retries.md), each retry is a separate attempt with its own attempt ID. If you're collecting data by individual test execution, an attempt ID is a suitable, globally unique map key.

An attempt ID is comprised of:

- The [run ID](#run-id)
- The [binary ID](#binary-id)
- For [stress tests](features/stress-tests.md), the 0-indexed stress index (not included if this is not a stress run)
- The test name
- The current attempt number, if the test is being [retried](features/retries.md) (not included if this is the first attempt)

An example attempt ID is:

```
55459fda-13fe-406a-b4e3-0230fd52bb03:nextest_runner::integration@stress-3$basic::retry_overrides_ignored#2
```

Here:

- `55459fda-13fe-406a-b4e3-0230fd52bb03` is the run ID.
- `nextest_runner::integration` is the binary ID.
- `stress-3` is the stress index (the 4th stress iteration).
- `basic::retry_overrides_ignored` is the test name.
- `2` is the attempt number (indicating this is the 2nd attempt).

The attempt ID is available:

- <!-- md:version 0.9.116 --> Via the [`NEXTEST_ATTEMPT_ID`](configuration/env-vars.md#environment-variables-nextest-sets) environment variable.
- <!-- md:version 0.9.108 --> In [USDT probes](integrations/usdt.md), through `arg1` for `test-attempt-*` events, and also JSON-encoded in `arg0` as the `attempt_id` field.

## Slot numbers

<!-- md:version 0.9.90 -->

Nextest assigns each running test a *global slot number*. Additionally, if a test is in a [test group](configuration/test-groups.md), the test is also assigned a *group slot number*.

Slot numbers are non-negative integers starting from 0. They are useful for assigning resources such as blocks of port numbers to tests.

Slot numbers are:

- **Unique** for the lifetime of the test: no other concurrently running test will have the same global slot number, and no other concurrently running test in the same group will have the same group slot number.
- **Stable** across [retries](features/retries.md) within the same run (though not across runs).
- **Compact**: each test is assigned the smallest available slot number at the time it starts. For example, if a test group is limited to serial execution, the group slot number is always 0.

The global slot number is available:

- Via the [`NEXTEST_TEST_GLOBAL_SLOT`](configuration/env-vars.md#environment-variables-nextest-sets) environment variable.
- <!-- md:version 0.9.108 --> In [USDT probes](integrations/usdt.md), JSON-encoded in `arg0` as the `global_slot` field.

The group slot number is available:

- Via the [`NEXTEST_TEST_GROUP_SLOT`](configuration/env-vars.md#environment-variables-nextest-sets) environment variable. (If a test is not within a group, `NEXTEST_TEST_GROUP_SLOT` is set to `none`.)
- <!-- md:version 0.9.108 --> In [USDT probes](integrations/usdt.md), JSON-encoded in `arg0` as the `group_slot` field. (If a test is not within a group, this is `null`.)
