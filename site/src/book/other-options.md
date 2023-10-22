# Other options

Some other options accepted by `cargo nextest run`. Many of these options are also accepted as [configuration settings](configuration.md) and [environment variables](env-vars.md).

## Runner options

- `--no-fail-fast`: do not exit the test run on the first failure. Most useful for CI scenarios.
- `-j, --test-threads`: number of tests to run simultaneously. Note that this is separate from the number of build jobs to run simultaneously, which is specified by `--build-jobs`.
- `--run-ignored ignored-only` runs ignored tests, while `--run-ignored all` runs both ignored and non-ignored tests.

## Reporter options

### `--success-output` and `--failure-output`

These options control when standard output and standard error are displayed for failing and passing tests, respectively. The possible values are:

- `immediate`: display output as soon as the test fails. Default for `--failure-output`.
- `final`: display output at the end of the test run.
- `immediate-final`: display output as soon as the test fails, and at the end of the run. This is most useful for CI jobs.
- `never`: never display output. Default for `--success-output`.

These options can also be configured via [global configuration](configuration.md) and [per-test overrides](per-test-overrides.md). Specifying these options over the command line will override configuration settings.

### `--status-level` and `--final-status-level`

- `--status-level`: which test statuses (**PASS**, **FAIL** etc) to display. There are 7 status levels: `none, fail, retry, slow, pass, skip, all`. Each status level causes all earlier status levels to be displayed as well (similar to log levels). (For example, setting `status-level` to `skip` will show failing, retried, slow and passing tests along with skipped tests.) The default is `pass`.
- `--final-status-level`: which test statuses to display at the end of a test run. For example, this can be set to `fail` to print out a list of failing tests at the end of a test run. The default is `none`.

For a full list of options, see [Options and arguments](running.md#options-and-arguments).
