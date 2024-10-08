---
icon: material/message-processing-outline
---

# Machine-readable output formats

Nextest provides a number of ways to obtain output suitable for consumption by other tools or infrastructure.

## Test and binary lists

For test lists, nextest provides JSON output. See [_Machine-readable test lists_](list.md#machine-readable-test-lists).

In addition, nextest can also provide a list of binaries without running them to obtain the list of tests. See [_Machine-readable binary lists_](list.md#machine-readable-binary-lists) for more information.

## Test runs

For test runs, the main mechanism available is JUnit XML. For more information, see [_JUnit support_](junit.md).

Additionally, as an experimental feature, JSON libtest-like output is supported. This is primarily meant for compatibility with existing test infrastructure that consumes this output, and is not currently full-fidelity. For more information, see [_Libtest JSON output_](libtest-json.md).

## Future work

The overall aspiration is for all human-readable UI to also become machine-readable. Some features that are still missing:

1. A first-class newline-delimited JSON format for test runs, not necessarily attempting to retain compatibility with libtest JSON. See [#20](https://github.com/nextest-rs/nextest/issues/20).
2. Detected configuration: both [nextest-specific configuration](../configuration/index.md), and configuration detected by emulating Cargo. See [#1527](https://github.com/nextest-rs/nextest/issues/1527).

!!! note

    If you'd like to see any of these happen, contributions would be greatly appreciated!
