// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![warn(missing_docs)]

//! `quick-junit` is a JUnit/XUnit XML serializer for Rust. This crate is built to serve the needs
//! of [cargo-nextest](docs.rs/cargo-nextest).
//!
//! # Overview
//!
//! The root element of a JUnit report is a [`Report`]. A [`Report`] consists of one or more
//! [`TestSuite`] instances. A [`TestSuite`] instance consists of one or more [`TestCase`]s.
//!
//! The status (success, failure, error, or skipped) of a [`TestCase`] is represented by [`TestCaseStatus`].
//! If a test was rerun, [`TestCaseStatus`] can manage [`TestRerun`] instances as well.
//!
//! # Examples
//!
//! ```rust
//! use quick_junit::*;
//!
//! let mut report = Report::new("my-test-run");
//! let mut test_suite = TestSuite::new("my-test-suite");
//! let success_case = TestCase::new("success-case", TestCaseStatus::success());
//! let failure_case = TestCase::new("failure-case", TestCaseStatus::non_success(NonSuccessKind::Failure));
//! test_suite.add_test_cases([success_case, failure_case]);
//! report.add_test_suite(test_suite);
//!
//! const EXPECTED_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
//! <testsuites name="my-test-run" tests="2" failures="1" errors="0">
//!     <testsuite name="my-test-suite" tests="2" disabled="0" errors="0" failures="1">
//!         <testcase name="success-case">
//!         </testcase>
//!         <testcase name="failure-case">
//!             <failure/>
//!         </testcase>
//!     </testsuite>
//! </testsuites>
//! "#;
//!
//! assert_eq!(report.to_string().unwrap(), EXPECTED_XML);
//! ```
//!
//! For a more comprehensive example, see
//! [`fixture_tests.rs`](https://github.com/diem/diem-devtools/blob/main/quick-junit/tests/fixture_tests.rs).

mod report;
mod serialize;

pub use report::*;

// Re-export `quick_xml::Error` and `Result` so it can be used by downstream consumers.
#[doc(no_inline)]
pub use quick_xml::{Error, Result};
