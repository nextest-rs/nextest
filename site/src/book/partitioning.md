# Partitioning test runs in CI

For CI scenarios where test runs take too long on a single machine, nextest supports automatically _partitioning_ or _sharding_ tests into buckets, using the `--partition` option.

cargo-nextest supports two kinds of partitioning: _counted_ and _hashed_.

## Counted partitioning

Counted partitioning is specified with `--partition count:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in count-based bucket m of n".

Here's an example of running tests in bucket 1 of 2:

![Output of cargo nextest run --partition count:1/2](../static/nextest-partition.png)

Tests not in the current bucket are marked skipped.

Counted partitioning is done _per test binary_. This means that the tests in one binary _do not_ influence counting for other binaries.

Counted partitioning also applies after all other test filters. For example, if you specify `cargo nextest run --partition count:1/3 test_parsing`, nextest first selects tests that match the substring `test_parsing`, then buckets this subset of tests into 3 partitions and runs the tests in partition 1.

## Hashed sharding

Hashed sharding is specified with `--partition hash:m/n`, where m and n are both integers, and 1 ≤ m ≤ n. Specifying this operator means "run tests in hashed bucket m of n".

The main benefit of hashed sharding is that it is completely deterministic (the hash is based on a combination of the binary and test names). Unlike with counted partitioning, adding or removing tests, or changing test filters, will never cause a test to fall into a different bucket. The hash algorithm is guaranteed never to change within a nextest version series.

For sufficiently large numbers of tests, hashed sharding produces roughly the same number of tests per bucket. However, smaller test runs may result in an uneven distribution.

## Reusing builds

By default, each job has to do its own build before starting a test run. To save on the extra work, nextest supports [archiving builds](reusing-builds.md) in one job for later reuse in other jobs. See the example below for how to do this.

## Example: Use in GitHub Actions

See [this working example](https://github.com/nextest-rs/reuse-build-partition-example/blob/main/.github/workflows/ci.yml) for how to [reuse builds](reusing-builds.md) and partition test runs on GitHub Actions.

## Example: Use in GitLab CI

GitLab can [parallelize jobs](https://docs.gitlab.com/ee/ci/yaml/#parallel) across runners. This works neatly with `--partition`. For example:

```yaml
test:
  stage: test
  parallel: 3
  script:
    - echo "Node index - ${CI_NODE_INDEX}. Total amount - ${CI_NODE_TOTAL}"
    - time cargo nextest run --workspace --partition count:${CI_NODE_INDEX}/${CI_NODE_TOTAL}
```

This creates three jobs that run in parallel: `test 1/3`, `test 2/3` and `test 3/3`.
