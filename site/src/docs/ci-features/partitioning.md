---
icon: material/set-split
description: Split up test runs across multiple runners through counted partitioning or hashed sharding.
---

# Partitioning test runs in CI

For CI scenarios where test runs take too long on a single machine, nextest supports automatically _partitioning_ or _sharding_ tests into buckets using the `--partition` option.

## Types of partitioning

cargo-nextest supports two kinds of partitioning: _sliced_ and _hashed_.

### Sliced partitioning

<!-- md:version 0.9.127 -->

With sliced partitioning, nextest lists all tests across all specified binaries, then assigns tests to each bucket in a round-robin fashion.

For example, in a situation with three binaries and two tests in each binary:

| binary   | test   | with 2 slices | with 3 slices |
| -------- | ------ | ------------- | ------------- |
| binary 1 | test 1 | slice 1       | slice 1       |
| binary 1 | test 2 | slice 2       | slice 2       |
| binary 2 | test 1 | slice 1       | slice 3       |
| binary 2 | test 2 | slice 2       | slice 1       |
| binary 3 | test 1 | slice 1       | slice 2       |
| binary 3 | test 2 | slice 2       | slice 3       |

Slices are specified with `--partition slice:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in slice m of n".

Sliced partitioning applies after all other test filters. For example, if you specify `cargo nextest run --partition slice:1/3 test_parsing`, nextest first selects tests that match the substring `test_parsing`, then buckets this subset of tests into 3 partitions and runs the tests in partition 1.

#### Example output for sliced partitioning

Example output for the command `cargo nextest run --partition slice:1/4`:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/slice-partitioning-output.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/slice-partitioning-output.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

Tests not in the current bucket are marked skipped.

### Hashed sharding

Hashed sharding is specified with `--partition hash:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in hashed bucket m of n".

The main benefit of hashed sharding is that it is completely deterministic (the hash is based on a combination of [binary ID](../glossary.md#binary-id) and test names). Unlike with sliced partitioning, adding or removing tests, or changing test filters, will never cause a test to fall into a different bucket. The hash algorithm is guaranteed never to change within a nextest version series.

For sufficiently large numbers of tests, hashed sharding produces roughly the same number of tests per bucket. However, smaller test runs may result in an uneven distribution.

!!! note "Counted partitioning is deprecated"

    Nextest also supports a _counted_ partitioning mode via `--partition count:m/n`. Counted partitioning is similar to sliced partitioning, except the buckets are made per test binary. Counted partitioning has neither the even nature of sliced partitioning nor the stability of hashed sharding, so it is worse than both.

    Nextest will continue to support counted partitioning, but it is recommended that you switch to `slice:`.

## Reusing builds

By default, each job has to do its own build before starting a test run. To save on the extra work, nextest supports [archiving builds](archiving.md) in one job for later reuse in other jobs. See [*Example: Use in GitHub Actions*](#example) below for how to do this.

## Example: Use in GitHub Actions { #example }

See [this working example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml) for how to [reuse builds](archiving.md) and partition test runs on GitHub Actions.

## Example: Use in GitLab CI

GitLab can [parallelize jobs](https://docs.gitlab.com/ee/ci/yaml/#parallel) across runners. This works neatly with `--partition`. For example:

```yaml
test:
  stage: test
  parallel: 3
  script:
    - echo "Node index - ${CI_NODE_INDEX}. Total amount - ${CI_NODE_TOTAL}"
    - time cargo nextest run --workspace --partition slice:${CI_NODE_INDEX}/${CI_NODE_TOTAL}
```

This creates three jobs that run in parallel: `test 1/3`, `test 2/3` and `test 3/3`.
