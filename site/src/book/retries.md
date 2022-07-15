# Retries and flaky tests

Sometimes, tests fail nondeterministically, which can be quite annoying to developers locally and in CI. cargo-nextest supports *retrying* failed tests with the `--retries` option. If a test succeeds during a retry, the test is marked *flaky*. Here's an example:

![Output of cargo nextest run --retries 2](../static/nextest-retry.png)

`--retries 2` means that the test is retried twice, for a total of three attempts. In this case, the test fails on the first try but succeeds on the second try. The `TRY 2 PASS` text means that the test passed on the second try.

Flaky tests are treated as ultimately successful. If there are no other tests that failed, the exit code for the test run is 0.

Retries can also be:
* passed in via the environment variable `NEXTEST_RETRIES`.
* [configured in `.config/nextest.toml`](configuration.md).

For the order that configuration parameters are resolved in, see [Hierarchical configuration](configuration.md#hierarchical-configuration).

## Per-test overrides

Nextest supports [per-test overrides](per-test-overrides.md) for retries, letting you mark a subset of tests as needing retries. For example, to mark test names containing `"test_e2e"` as requiring retries:

```toml
[[profile.default.overrides]]
filter = 'test(test_e2e)'
retries = 2
```

> **Note:** The `--retries` command-line option and the `NEXTEST_RETRIES` environment variable both disable overrides.

## JUnit support

Flaky test detection is integrated with nextest's JUnit support. For more information, see [JUnit support](junit.md).
