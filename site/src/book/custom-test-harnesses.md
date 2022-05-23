# Custom test harnesses

A custom test harness is defined in `Cargo.toml` as:

```toml
[[test]]
name = "my-test"
harness = false
```

As mentioned in [*How nextest works*](how-it-works.md), cargo-nextest has a much thicker interface with the test harness than cargo test does. If you don't use any custom harnesses, cargo-nextest will run out of the box. However, custom test harnesses need to follow certain rules in order to work with nextest.

## libtest-mimic (recommended)

Any custom test harness that uses [libtest-mimic](https://github.com/LukasKalbertodt/libtest-mimic) (version 0.4.0 or above) is compatible with nextest. Using this crate is recommended.

For an example test harness that uses libtest-mimic, see [datatest-stable](https://github.com/nextest-rs/datatest-stable).

> NOTE: Versions of libtest-mimic prior to 0.4.0 are not compatible with nextest.

## Manually implementing a test harness

For your test harness to work with nextest, follow these rules (keywords are as in [RFC2119]):

[RFC2119]: https://datatracker.ietf.org/doc/html/rfc2119

* **The test harness MUST support being run with `--list --format terse`.** This command MUST print to stdout all tests in *exactly* the format

    ```
    my-test-1: test
    my-test-2: test
    ```
    Other output MUST NOT be written to stdout.

    Custom test harnesses that are meant to be run as a single unit MUST produce just one line in the output.
* **The test harness MUST support being run with `--list --format terse --ignored`**. This command MUST print to stdout exactly the set of ignored tests (however the harness defines them) in the same format as above. If there are no ignored tests or if the test harness doesn't support ignored tests, the output MUST be empty. The set of ignored tests MUST be either of the following two options:
  * A subset of the tests printed out without `--ignored`; this is what libtest does.
  * A completely disjoint set of tests from those printed out without `--ignored`.
* **Test names that are not at the top level (however the harness defines this) SHOULD be returned as `path::to::test::test_name`.** This is recommended because the cargo-nextest UI uses `::` as a separator to format test names nicely.
* **The test harness MUST support being run with `<test-name> --nocapture --exact`**. This command will be called with every test name provided by the harness in `--list` above.
