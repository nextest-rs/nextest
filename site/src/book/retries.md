# Retries and flaky tests

Sometimes, tests fail nondeterministically, which can be quite annoying to developers locally and in CI. cargo-nextest supports *retrying* failed tests with the `--retries` option. If a test succeeds during a retry, the test is marked *flaky*. Here's an example:

<img src="https://user-images.githubusercontent.com/180618/153311213-0fde9a74-f8ee-4701-a754-6866b80c476f.png"/>

`--retries 2` means that the test is retried twice, for a total of three attempts. In this case, the test fails on the first try but succeeds on the second try. The `TRY 2 PASS` text means that the test passed on the second try.

Flaky tests are treated as ultimately successful. If there are no other tests that failed, the exit code for the test run is 0.

Retries can also be [configured in `.config/nextest.toml`](configuration.md). The command-line `--retries` option overrides the configured value.

Flaky test detection is integrated with nextest's JUnit support. For more information, see [JUnit support](junit.md).
