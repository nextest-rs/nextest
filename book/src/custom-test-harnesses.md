# Custom test harnesses

A custom test harness is defined in `Cargo.toml` as:

```toml
[[test]]
name = "my-test"
harness = false
```

As mentioned in [How nextest works](how-it-works.md), cargo-nextest has a much thicker interface with the test harness than cargo test does. If you don't use any custom harnesses, cargo-nextest will run out of the box. However, users of custom test harnesses may need to be changed to work with cargo nextest. In general, the changes make the test harness look more like the default Rust test harness, and are pretty small overall.

* **Custom test harnesses MUST support being run with `--list --format terse`.** This command MUST print to stdout all tests in *exactly* the format

    ```
    my-test-1: test
    my-test-2: test
    ```
    Other output MUST NOT be written to stdout.

    Custom test harnesses that are meant to be run as a single unit MUST produce just one line in the output.
* **Custom test harnesses MUST support being run with `--list --format terse --ignored`**. This command MUST print to stdout exactly the set of ignored tests (however the harness defines them) in the same format as above. If there are no ignored tests or if the test harness doesn't support ignored tests, the output MUST be empty.
* **Test names that are not at the top level (however the harness defines this) SHOULD be returned as `path::to::test::test_name`.** This is recommended because the cargo-nextest UI uses `::` as a separator to format test names nicely.
* **Custom test harnesses MUST support being run with `<test-name> --nocapture --exact`**. This command will be called with every test name provided by the harness in `--list` above.
