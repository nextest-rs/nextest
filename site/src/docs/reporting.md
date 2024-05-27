---
icon: material/account-voice
---

# Reporting test results

This section covers nextest's output format designed for humans. For output formats more suitable for consumption by other tools, see [_Machine-readable output_](machine-readable/index.md).

## Status levels

For non-output-related information (e.g. exit codes, slow tests), there are two options that control which test statuses (**PASS**, **FAIL** etc) are displayed during a test run:

`--status-level`
: Which test statuses to display during the run. The default is `pass`.

`--final-status-level`
: Which test statuses to display at the end of a test run. The default is `fail`.

There are 7 status levels: `none, fail, retry, slow, pass, skip, all`. Each status level causes all earlier status levels to be displayed as well, similar to log levels. For example, setting `--status-level` to `skip` will show failing, retried, slow and passing tests along with skipped tests.

## Standard output and standard error

For standard output and standard error produced by tests, nextest attempts to strike a balance between a clean user interface and providing information relevant for debugging. By default, nextest will hide test output for passing tests, and show them for failing tests.

### Displaying live test output

If you do not want to capture test output at all, run:

```
cargo nextest run --no-capture
```

In this mode, nextest will pass standard output and standard error through to the terminal. Nextest will also run tests _serially_ so that output from different tests isn't interspersed. This is different from `cargo test -- --nocapture`, which will run tests in parallel.

### Displaying captured test output

When `--no-capture` isn't used, nextest will capture standard output and standard error. There are
two options that control the situations in which test output (standard output and standard error) is
displayed:

`--success-output`
: When to display standard output and standard error for passing tests. The default is `never`.

`--failure-output`
: When to display standard output and standard error for failing tests. The default is `immediate`.

The possible values for these two are:

<div class="compact" markdown>

`immediate`
: Display output as soon as the test fails. Default for `--failure-output`.

`final`
: Display output at the end of the test run.

`immediate-final`
: Display output as soon as the test fails, and at the end of the run. This is most useful for CI jobs.

`never`
: Never display output. Default for `--success-output`.

</div>

These options can also be configured via [global configuration](configuration/index.md) and [per-test overrides](configuration/per-test-overrides.md). Specifying these options over the command line will override configuration settings.

## Options and arguments

For a full list of options, see the [options and arguments](running.md#options-and-arguments) for `cargo nextest run`.
