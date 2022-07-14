# Partitioning test runs in CI

For CI scenarios where test runs take too long on a single machine, nextest supports automatically *partitioning* or *sharding* tests into buckets, using the `--partition` option.

cargo-nextest supports two kinds of partitioning: *counted* and *hashed*.

## Counted partitioning

Counted partitioning is specified with `--partition count:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in count-based bucket m of n".

Here's an example of running tests in bucket 1 of 2:

![Output of cargo nextest run --partition count:1/2](../static/nextest-partition.png)

Tests not in the current bucket are marked skipped.

Counted partitioning is done *per test binary*. This means that the tests in one binary *do not* influence counting for other binaries.

Counted partitioning also applies after all other test filters. For example, if you specify `cargo nextest run --partition count:1/3 test_parsing`, nextest first selects tests that match the substring `test_parsing`, then buckets this subset of tests into 3 partitions and runs the tests in partition 1.

## Hashed sharding

Hashed sharding is specified with `--partition hash:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in hashed bucket m of n".

The hash is completely deterministic, and is based on a combination of the binary and test names. The hash algorithm is guaranteed never to change within a nextest version series.

For sufficiently large numbers of tests, hashed sharding produces roughly the same number of tests per bucket. However, smaller test runs may result in an uneven distribution.

## Example: Use in GitHub Actions

See [this working example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml) for how to [reuse builds](reusing-builds.md) and partition test runs on GitHub Actions.

## Example: Use in GitLab CI

GitLab can [parallelize the job](https://docs.gitlab.com/ee/ci/yaml/#parallel) between runners. This ability neatly plays with `--partition`.

```yaml
test:
  stage:                           test
  parallel: 3
  script:
    - echo "Node index - ${CI_NODE_INDEX}. Total amount - ${CI_NODE_TOTAL}"
    - time cargo nextest run --workspace --partition count:${CI_NODE_INDEX}/${CI_NODE_TOTAL}
```

This creates three jobs in a pipeline: `test 1/3`, `test 2/3` and `test 3/3`.
It's recommended to use caching or [archiving](reusing-builds.md) to avoid having all three jobs building the same code before actual testing.
