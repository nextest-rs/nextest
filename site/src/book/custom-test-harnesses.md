# Custom test harnesses

A custom test harness is defined in `Cargo.toml` as:

```toml
[[test]]
name = "my-test"
harness = false
```

As mentioned in [_How nextest works_](how-it-works.md), cargo-nextest has a much thicker interface with the test harness than cargo test does. If you don't use any custom harnesses, cargo-nextest will run out of the box. However, custom test harnesses need to follow certain rules in order to work with nextest.

## libtest-mimic (recommended)

Nextest is compatible with custom test harnesses based on [libtest-mimic](https://github.com/LukasKalbertodt/libtest-mimic), version 0.4.0, or 0.5.2 or above (note that 0.5.0 and 0.5.1 have [a regression](https://github.com/nextest-rs/datatest-stable/pull/5)). Using this crate is recommended.

### Example: datatest-stable

For a test harness based on libtest-mimic, see [datatest-stable](https://github.com/nextest-rs/datatest-stable). This harness implements _data-driven tests_.

- datatest-stable can be used out of the box if each test is specified by a file within a particular directory on disk. For example, it is used by the [Move smart contract language](https://github.com/move-language/move) for many of its internal tests. With [these tests](https://github.com/move-language/move/tree/dfd7cf14a32f8182ddd9f39e9da086c29cb20b7b/language/move-ir-compiler/transactional-tests/tests/bytecode-generation/declarations), the harness is used to verify that each `.mvir` input results in the `.exp` output.
- datatest-stable also serves as an example for how to write your own custom test harness, if you need to.

> **Note:** Versions of libtest-mimic prior to 0.4.0 are not compatible with nextest.

## Manually implementing a test harness

For your test harness to work with nextest, follow these rules (keywords are as in [RFC 2119]):

[RFC 2119]: https://datatracker.ietf.org/doc/html/rfc2119

- **The test harness MUST support being run with `--list --format terse`.** This command MUST print to stdout all tests in _exactly_ the format

  ```
  my-test-1: test
  my-test-2: test
  ```

  Other output MUST NOT be written to stdout.

  Custom test harnesses that are meant to be run as a single unit MUST produce just one line in the output.

- **The test harness MUST support being run with `--list --format terse --ignored`**. This command MUST print to stdout exactly the set of ignored tests (however the harness defines them) in the same format as above. If there are no ignored tests or if the test harness doesn't support ignored tests, the output MUST be empty. The set of ignored tests MUST be either of the following two options:
  - A subset of the tests printed out without `--ignored`; this is what libtest does.
  - A completely disjoint set of tests from those printed out without `--ignored`.
- **Test names that are not at the top level (however the harness defines this) SHOULD be returned as `path::to::test::test_name`.** This is recommended because the cargo-nextest UI uses `::` as a separator to format test names nicely.
- **The test harness MUST support being run with `<test-name> --nocapture --exact`**. This command will be called with every test name provided by the harness in `--list` above.
