---
icon: material/refresh-auto
---

# Stress tests

<!-- md:version 0.9.103 -->

Nextest can run one or more tests many times in a loop. To do so, use the `--stress-count` or `--stress-duration` options.

`--stress-count=N`
: Run each test `N` times.

`--stress-count=infinite`
: Run stress tests indefinitely.

`--stress-duration=DURATION`
: Run tests until `DURATION` (e.g. `24h` or `90m`) has elapsed.

## Stress test output

Stress tests are run in a loop, and each sub-run is annotated with a count.

For example, with `cargo nextest run --stress-count 3 test_expr_deps test_expr_package_regex`, you might see output like:

=== "Colorized"

    ```bash exec="true" result="ansi"
    cat src/outputs/stress-test-output.ansi
    ```

=== "Plaintext"

    ```bash exec="true" result="text"
    cat src/outputs/stress-test-output.ansi | ../scripts/strip-ansi.sh
    ```

(Nextest does not currently support running the same test multiple times in parallel. It may gain support for this style of stress test in the future.)

### JUnit output

With [JUnit output](../machine-readable/junit.md), each `<testsuite>` element's `name` attribute is annotated with `@stress-N`, where `N` is a zero-indexed counter.

For example:

```bash exec="true" result="xml"
cat ../fixtures/stress-test-junit.xml
```

### Libtest JSON output

With [libtest JSON output](../machine-readable/libtest-json.md), each iteration of a stress test is annotated with `@stress-N`, where `N` is a zero-indexed counter.

For example:

```json title="Example output with --message-format libtest-json"
{"type":"suite","event":"started","test_count":2}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-0$test_expr_package_regex"}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-0$test_expr_deps"}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-0$test_expr_deps","exec_time":0.005941609}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-0$test_expr_package_regex","exec_time":0.006209806}
{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0,"measured":0,"filtered_out":19,"exec_time":0.012151415}
{"type":"suite","event":"started","test_count":2}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-1$test_expr_deps"}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-1$test_expr_package_regex"}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-1$test_expr_deps","exec_time":0.00610633}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-1$test_expr_package_regex","exec_time":0.006165292}
{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0,"measured":0,"filtered_out":19,"exec_time":0.012271622}
```

With `libtest-json-plus`, the additional `nextest` object contains `stress_index` and `stress_total` (if available) fields.

```json title="Example output with --message-format libtest-json-plus"
{"type":"suite","event":"started","test_count":2,"nextest":{"crate":"nextest-filtering","test_binary":"match","kind":"test","stress_index":0,"stress_total":2}}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-0$test_expr_package_regex"}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-0$test_expr_deps"}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-0$test_expr_deps","exec_time":0.005379185}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-0$test_expr_package_regex","exec_time":0.005661841}
{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0,"measured":0,"filtered_out":19,"exec_time":0.011041026,"nextest":{"crate":"nextest-filtering","test_binary":"match","kind":"test","stress_index":0,"stress_total":2}}
{"type":"suite","event":"started","test_count":2,"nextest":{"crate":"nextest-filtering","test_binary":"match","kind":"test","stress_index":1,"stress_total":2}}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-1$test_expr_package_regex"}
{"type":"test","event":"started","name":"nextest-filtering::match@stress-1$test_expr_deps"}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-1$test_expr_deps","exec_time":0.004991222}
{"type":"test","event":"ok","name":"nextest-filtering::match@stress-1$test_expr_package_regex","exec_time":0.005451261}
{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0,"measured":0,"filtered_out":19,"exec_time":0.010442483,"nextest":{"crate":"nextest-filtering","test_binary":"match","kind":"test","stress_index":1,"stress_total":2}}
```
