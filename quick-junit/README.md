# quick-junit

[![quick-junit on crates.io](https://img.shields.io/crates/v/quick-junit)](https://crates.io/crates/quick-junit)
[![Documentation (latest release)](https://img.shields.io/badge/docs-latest-brightgreen.svg)](https://docs.rs/quick-junit/)
[![Documentation (main)](https://img.shields.io/badge/docs-main-purple)](https://nexte.st/rustdoc/quick_junit/)
[![Changelog](https://img.shields.io/badge/changelog-latest-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-Apache-green.svg)](LICENSE-APACHE)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE-MIT)

`quick-junit` is a JUnit/XUnit XML data model and serializer for Rust. This crate allows users
to create a JUnit report as an XML file. JUnit XML files are widely supported by test tooling.

 This crate is built to serve the needs of [cargo-nextest](https://nexte.st).

## Overview

The root element of a JUnit report is a `Report`. A `Report` consists of one or more
`TestSuite` instances. A `TestSuite` instance consists of one or more `TestCase`s.

The status (success, failure, error, or skipped) of a `TestCase` is represented by `TestCaseStatus`.

## Features

- ✅ Serializing JUnit/XUnit to the [Jenkins format](https://llg.cubic.org/docs/junit/).
- ✅ Including test reruns using `TestRerun`
- ✅ Including flaky tests
- ✅ Including standard output and error
  - ✅ Filtering out [invalid XML
    characters](https://en.wikipedia.org/wiki/Valid_characters_in_XML) (eg ANSI escape codes)
    from the output
- ✅ Automatically keeping track of success, failure and error counts
- ✅ Arbitrary properties and extra attributes

This crate does not currently support deserializing JUnit XML. (PRs are welcome!)

## Examples

```rust
use quick_junit::*;

let mut report = Report::new("my-test-run");
let mut test_suite = TestSuite::new("my-test-suite");
let success_case = TestCase::new("success-case", TestCaseStatus::success());
let failure_case = TestCase::new("failure-case", TestCaseStatus::non_success(NonSuccessKind::Failure));
test_suite.add_test_cases([success_case, failure_case]);
report.add_test_suite(test_suite);

const EXPECTED_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="my-test-run" tests="2" failures="1" errors="0">
    <testsuite name="my-test-suite" tests="2" disabled="0" errors="0" failures="1">
        <testcase name="success-case">
        </testcase>
        <testcase name="failure-case">
            <failure/>
        </testcase>
    </testsuite>
</testsuites>
"#;

assert_eq!(report.to_string().unwrap(), EXPECTED_XML);
```

For a more comprehensive example, including reruns and flaky tests, see
[`fixture_tests.rs`](https://github.com/nextest-rs/nextest/blob/main/quick-junit/tests/fixture_tests.rs).

## Minimum supported Rust version (MSRV)

The minimum supported Rust version is **Rust 1.54.**

While this crate is a pre-release (0.x.x) it may have its MSRV bumped in a patch release.
Once a crate has reached 1.x, any MSRV bump will be accompanied with a new minor version.

## Alternatives

* [**junit-report**](https://crates.io/crates/junit-report): Older, more mature project. Doesn't
  appear to support flaky tests or arbitrary properties as of version 0.7.0.

## Contributing

See the [CONTRIBUTING](../CONTRIBUTING.md) file for how to help out.

## License

This project is available under the terms of either the [Apache 2.0 license](../LICENSE-APACHE) or
the [MIT license](../LICENSE-MIT).

<!--
README.md is generated from README.tpl by cargo readme. To regenerate, run from the repository root:

./scripts/regenerate-readmes.sh
-->
