---
icon: material/restart
description: Rerunning failed tests to iteratively converge towards a successful test run.
---

# Rerunning failed tests

When the recording feature is enabled, you can rerun failing tests with `cargo nextest run -R latest`. This command will run tests that, in the original run:

- failed;
- did not run because the test run was cancelled; or,
- were not previously seen, typically because they were newly added since the original run. (New tests are included so that they are validated while iterating on a failing run.)

The rerun feature works purely at the test level, and does not track code or build changes.

!!! tip "Rerun build scope"

    Without any further arguments, `cargo nextest run -R latest` will build the same targets that the original run did. If build scope arguments are specified, they will override the set of build targets from the original run.

    Build scope arguments include all arguments under the _Package selection_, _Target selection_, and _Feature selection_ headings of `cargo nextest run --help`.

## Prerequisites

To enable run recording, see [_Setting up run recording_](index.md#setting-up-run-recording).

## Example rerun flow

Let's say that `cargo nextest run --package nextest-filtering` was run, and it had two failing tests:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-original-run.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-original-run.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

---

With `cargo nextest run -R latest proptest_helpers`, the first test is selected:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

All selected tests passed, but some outstanding (previously-failing) tests still remain, so nextest exits with the advisory exit code 5 ([`RERUN_TESTS_OUTSTANDING`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.RERUN_TESTS_OUTSTANDING)).

---

A subsequent `cargo nextest run -R latest` will run the remaining test:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest-2.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest-2.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

!!! note "Exit code for no tests in a rerun"

    In regular runs, if there are no tests to run, nextest exits with the advisory exit code 4 ([`NO_TESTS_RUN`](https://docs.rs/nextest-metadata/latest/nextest_metadata/enum.NextestExitCode.html#associatedconstant.NO_TESTS_RUN)) by default.

    With reruns, if there are no tests to run, nextest exits with exit code 0 by default, indicating success. The difference in behavior is due to the goal of reruns being to converge to a successful test run.

---

It is possible to rewind the rerun logic to an earlier state by passing in a run ID to `-R`. In this case `b0b` forms an unambiguous prefix (highlighted in bold purple), so `cargo nextest run -R b0b` results in both tests being run:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/rerun-latest-3.ansi | ../scripts/strip-hyperlinks.sh
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/rerun-latest-3.ansi | ../scripts/strip-ansi.sh | ../scripts/strip-hyperlinks.sh
    ```

### Reruns and portable recordings

Reruns also work with [portable recordings](portable-recordings.md), which are self-contained archives that can be shared across machines. For example, `cargo nextest run -R my-run.zip`.

This is particularly useful for rerunning tests locally that failed in CI.

## Rerun heuristics

Picking the set of tests to run is tricky, particularly in the face of tests being removed and new ones being added. We have attempted to pick a strategy that aims to be conservative while covering the most common use cases, but it is always possible that tests are missed. Because of this, and because code changes might include regressions in previously passing tests, it is recommended that you perform a full test run once your iterations are complete.

As a best practice, it is also recommended that you use CI to gate changes making their way to production, and that you perform full runs in CI.

For more about the heuristics and considerations involved, see the [rerun decision table](../../design/architecture/recording-runs.md#rerun-decision-table) in the design document.

## Options and arguments

For the full list of options accepted by `cargo nextest run`, including rerun options, see [_Options and arguments_](../../running.md#options-and-arguments), or `cargo nextest run --help`.
