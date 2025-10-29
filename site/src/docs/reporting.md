---
icon: material/account-voice
---

# Reporting test results

This section covers nextest's output format designed for humans. For output formats more suitable for consumption by other tools, see [_Machine-readable output_](machine-readable/index.md).

## Test execution progress

<!-- md:version 0.9.108 -->

During test execution, the `--show-progress` command-line option (or `NEXTEST_SHOW_PROGRESS` in the environment) determines how progress is displayed.

`--show-progress=auto`
: Automatically determine how to show progress. This is the default. `auto` resolves to `bar` for interactive terminals, and `counter` for non-interactive ones.

`--show-progress=bar`
: Show a progress bar.

`--show-progress=counter`
: Show a progress counter next to each test, once it completes.

`--show-progress=none`
: Do not show a progress bar or counter.

`--show-progress=running` <!-- md:version 0.9.109 -->
: Display each running test on a separate line.

`--show-progress=only` <!-- md:version 0.9.109 -->
: Display each running test on a separate line, and hide successful test output; equivalent to `--show-progress=running --status-level=slow --final-status-level=none`.

Nextest versions prior to 0.9.108 show a progress bar in interactive terminals, and do not show any progress in non-interactive terminals. The `--hide-progress-bar` option, now deprecated, hides progress in interactive terminals.

## Status levels

For non-output-related information (e.g. exit codes, slow tests), there are two options that control which test statuses (**PASS**, **FAIL** etc) are displayed during a test run:

`--status-level`
: Which test statuses to display during the run. The default is `pass`.

`--final-status-level`
: Which test statuses to display at the end of a test run. The default is `fail`.

There are 7 status levels: `none, fail, retry, slow, pass, skip, all`. Each status level causes all earlier status levels to be displayed as well, similar to log levels. For example, setting `--status-level` to `skip` will show failing, retried, slow and passing tests along with skipped tests.

## Standard output and standard error

For standard output and standard error produced by tests, nextest attempts to
strike a balance between a clean user interface and providing information
relevant for debugging. By default, nextest will hide test output for passing
tests, and show them for failing tests.

## Displaying live test output

If you do not want to capture test output at all, run:

```
cargo nextest run --no-capture
```

In this mode, nextest will pass standard output and standard error through to
the terminal. Nextest will also run tests _serially_ so that output from
different tests isn't interspersed. This is different from `cargo test --
--nocapture`, which will run tests in parallel and potentially cause interleaved
output.

## Displaying captured test output

When `--no-capture` isn't used, nextest will capture standard output and
standard error, and buffer it internally.

### …while tests are running { #live-output }

<!-- md:version 0.9.86 -->

While tests are running, nextest can be interactively queried to display
current test status, including captured output. This is useful for debugging
tests that might be stuck, or are otherwise taking a long time to run.

To query current test status, do any of the following:

* In an interactive terminal, press `t`. This works on all Unix platforms, as
  well as on Windows, as long as nextest's output is being directly presented
  to the terminal (e.g. not piped to another program).

  Processing `t` requires that the terminal's processing is altered, which may
  cause issues in some circumstances. To disable this behavior, run nextest with
  `--no-input-handler`, or set `NEXTEST_NO_INPUT_HANDLER=1` in the environment.

* On Unix platforms where the `SIGINFO` signal is available (which includes macOS
  and other BSD-based platforms, as well as illumos, though not Linux), send that signal to nextest.

  In an interactive terminal, [press Ctrl-T]. Otherwise, run `kill -INFO <pid>`,
  where `<pid>` is the process ID of the running nextest process.

* On Unix platforms, send [the `SIGUSR1` signal][sigusr1] to nextest. This can
  be done by running `kill -USR1 <pid>`, where `<pid>` is the process ID of the
  running nextest process.

On being queried, nextest will display, for all running tests:

* The process ID and how long the test has been running for.
* The current status (running, terminating, etc).
* Standard output and standard error collected so far.

[press Ctrl-T]: https://blog.danielisz.org/2018/06/21/the-power-of-ctrlt/
[sigusr1]: https://www.gnu.org/software/libc/manual/html_node/Miscellaneous-Signals.html

### …after tests have finished { #completed-output }

Two options control the situations in which test output is displayed:

`--success-output`
: When to display standard output and standard error for passing tests. The default is `never`.

`--failure-output`
: When to display standard output and standard error for failing tests. The default is `immediate`.

The possible values are:

<div class="compact" markdown>

`immediate`
: Display output as soon as the test fails. Default for `--failure-output`.

`final`
: Display output at the end of the test run.

`immediate-final`
: Display output as soon as the test fails, and at the end of the run.

`never`
: Never display output. Default for `--success-output`.

</div>

These options can also be configured via [global configuration](configuration/index.md) and [per-test overrides](configuration/per-test-overrides.md). Specifying these options over the command line will override configuration settings.

#### Other options for completed tests { #other-options }

`--no-output-indent` <!-- md:version 0.9.95 -->
: By default, nextest indents captured output by 4 spaces for visual clarity. This flag disables that behavior. Can also be set through `NEXTEST_NO_OUTPUT_INDENT=1` in the environment.

## Options and arguments

For a full list of options, see the [options and arguments](running.md#options-and-arguments) for `cargo nextest run`.
