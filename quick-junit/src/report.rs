// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::serialize::serialize_report;
use chrono::{DateTime, FixedOffset};
use indexmap::map::IndexMap;
use std::{io, iter, time::Duration};

/// The root element of a JUnit report.
#[derive(Clone, Debug)]
pub struct Report {
    /// The name of this report.
    pub name: String,

    /// The time at which the first test in this report began execution.
    ///
    /// This is not part of the JUnit spec, but may be useful for some tools.
    pub timestamp: Option<DateTime<FixedOffset>>,

    /// The overall time taken by the test suite.
    ///
    /// This is serialized as the number of seconds.
    pub time: Option<Duration>,

    /// The total number of tests from all testsuites.
    pub tests: usize,

    /// The total number of failures from all testsuites.
    pub failures: usize,

    /// The total number of errors from all testsuites.
    pub errors: usize,

    /// The test suites contained in this report.
    pub testsuites: Vec<Testsuite>,
}

impl Report {
    /// Creates a new `Report` with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            timestamp: None,
            time: None,
            tests: 0,
            failures: 0,
            errors: 0,
            testsuites: vec![],
        }
    }

    /// Sets the start timestamp for the report.
    pub fn set_timestamp(&mut self, timestamp: impl Into<DateTime<FixedOffset>>) -> &mut Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Sets the time taken for overall execution.
    pub fn set_time(&mut self, time: Duration) -> &mut Self {
        self.time = Some(time);
        self
    }

    /// Adds a new testsuite and updates the `tests`, `failures` and `errors` counts.
    ///
    /// When generating a new report, use of this method is recommended over adding to
    /// `self.testsuites` directly.
    pub fn add_testsuite(&mut self, testsuite: Testsuite) -> &mut Self {
        self.tests += testsuite.tests;
        self.failures += testsuite.failures;
        self.errors += testsuite.errors;
        self.testsuites.push(testsuite);
        self
    }

    /// Adds several testsuites and updates the `tests`, `failures` and `errors` counts.
    ///
    /// When generating a new report, use of this method is recommended over adding to
    /// `self.testsuites` directly.
    pub fn add_testsuites(&mut self, testsuites: impl IntoIterator<Item = Testsuite>) -> &mut Self {
        for testsuite in testsuites {
            self.add_testsuite(testsuite);
        }
        self
    }

    /// Serialize this report to the given writer.
    pub fn serialize(&self, writer: impl io::Write) -> quick_xml::Result<()> {
        serialize_report(self, writer)
    }

    /// Serialize this report to a string.
    pub fn to_string(&self) -> quick_xml::Result<String> {
        let mut buf: Vec<u8> = vec![];
        self.serialize(&mut buf)?;
        String::from_utf8(buf).map_err(|utf8_err| quick_xml::Error::Utf8(utf8_err.utf8_error()))
    }
}

/// Represents a single testsuite.
///
/// A `Testsuite` groups together several `Testcase` instances.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Testsuite {
    /// The name of this testsuite.
    pub name: String,

    /// The total number of tests in this testsuite.
    pub tests: usize,

    /// The total number of disabled tests in this testsuite.
    pub disabled: usize,

    /// The total number of tests in this suite that errored.
    ///
    /// An "error" is usually some sort of *unexpected* issue in a test.
    pub errors: usize,

    /// The total number of tests in this suite that failed.
    ///
    /// A "failure" is usually some sort of *expected* issue in a test.
    pub failures: usize,

    /// The time at which the testsuite began execution.
    pub timestamp: Option<DateTime<FixedOffset>>,

    /// The overall time taken by the testsuite.
    pub time: Option<Duration>,

    /// The testcases that form this testsuite.
    pub testcases: Vec<Testcase>,

    /// Custom properties set during test execution, e.g. environment variables.
    pub properties: Vec<Property>,

    /// Data written to standard output while the testsuite was executed.
    pub system_out: Option<Output>,

    /// Data written to standard error while the testsuite was executed.
    pub system_err: Option<Output>,

    /// Other fields that may be set as attributes, such as "hostname" or "package".
    pub extra: IndexMap<String, String>,
}

impl Testsuite {
    /// Creates a new `Testsuite`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            time: None,
            timestamp: None,
            tests: 0,
            disabled: 0,
            errors: 0,
            failures: 0,
            testcases: vec![],
            properties: vec![],
            system_out: None,
            system_err: None,
            extra: IndexMap::new(),
        }
    }

    /// Sets the start timestamp for the testsuite.
    pub fn set_timestamp(&mut self, timestamp: impl Into<DateTime<FixedOffset>>) -> &mut Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Sets the time taken for the testsuite.
    pub fn set_time(&mut self, time: Duration) -> &mut Self {
        self.time = Some(time);
        self
    }

    /// Adds a property to this testsuite.
    pub fn add_property(&mut self, property: impl Into<Property>) -> &mut Self {
        self.properties.push(property.into());
        self
    }

    pub fn add_properties(
        &mut self,
        properties: impl IntoIterator<Item = impl Into<Property>>,
    ) -> &mut Self {
        for property in properties {
            self.add_property(property);
        }
        self
    }

    /// Adds a testcase to this testsuite and updates the counts.
    ///
    /// When generating a new report, use of this method is recommended over adding to
    /// `self.testcases` directly.
    pub fn add_testcase(&mut self, testcase: Testcase) -> &mut Self {
        self.tests += 1;
        match &testcase.status {
            TestcaseStatus::Success { .. } => {}
            TestcaseStatus::NonSuccess { kind, .. } => match kind {
                NonSuccessKind::Failure => self.failures += 1,
                NonSuccessKind::Error => self.errors += 1,
            },
            TestcaseStatus::Skipped { .. } => self.disabled += 1,
        }
        self.testcases.push(testcase);
        self
    }

    pub fn add_testcases(&mut self, testcases: impl IntoIterator<Item = Testcase>) -> &mut Self {
        for testcase in testcases {
            self.add_testcase(testcase);
        }
        self
    }

    /// Sets standard output.
    pub fn set_system_out(&mut self, system_out: impl AsRef<str>) -> &mut Self {
        self.system_out = Some(Output::new(system_out.as_ref()));
        self
    }

    /// Sets standard output from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_out_lossy(&mut self, system_out: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_out(String::from_utf8_lossy(system_out.as_ref()))
    }

    /// Sets standard error.
    pub fn set_system_err(&mut self, system_err: impl AsRef<str>) -> &mut Self {
        self.system_err = Some(Output::new(system_err.as_ref()));
        self
    }

    /// Sets standard error from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_err_lossy(&mut self, system_err: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_err(String::from_utf8_lossy(system_err.as_ref()))
    }
}

/// Represents a single testcase.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Testcase {
    /// The name of the testcase.
    pub name: String,

    /// The "classname" of the testcase.
    ///
    /// Typically, this represents the fully qualified path to the test. In other words,
    /// `classname` + `name` together should uniquely identify and locate a test.
    pub classname: Option<String>,

    /// The number of assertions in the testcase.
    pub assertions: Option<usize>,

    /// The time at which this testcase began execution.
    ///
    /// This is not part of the JUnit spec, but may be useful for some tools.
    pub timestamp: Option<DateTime<FixedOffset>>,

    /// The time it took to execute this testcase.
    pub time: Option<Duration>,

    /// The status of this test.
    pub status: TestcaseStatus,

    /// Data written to standard output while the testcase was executed.
    pub system_out: Option<Output>,

    /// Data written to standard error while the testcase was executed.
    pub system_err: Option<Output>,

    /// Other fields that may be set as attributes, such as "classname".
    pub extra: IndexMap<String, String>,
}

impl Testcase {
    /// Creates a new testcase.
    pub fn new(name: impl Into<String>, status: TestcaseStatus) -> Self {
        Self {
            name: name.into(),
            classname: None,
            assertions: None,
            timestamp: None,
            time: None,
            status,
            system_out: None,
            system_err: None,
            extra: IndexMap::new(),
        }
    }

    /// Sets the classname of the test.
    pub fn set_classname(&mut self, classname: impl Into<String>) -> &mut Self {
        self.classname = Some(classname.into());
        self
    }

    /// Sets the number of assertions in the testcase.
    pub fn set_assertions(&mut self, assertions: usize) -> &mut Self {
        self.assertions = Some(assertions);
        self
    }

    /// Sets the start timestamp for the testcase.
    pub fn set_timestamp(&mut self, timestamp: impl Into<DateTime<FixedOffset>>) -> &mut Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Sets the time taken for the testcase.
    pub fn set_time(&mut self, time: Duration) -> &mut Self {
        self.time = Some(time);
        self
    }

    /// Sets standard output.
    pub fn set_system_out(&mut self, system_out: impl AsRef<str>) -> &mut Self {
        self.system_out = Some(Output::new(system_out.as_ref()));
        self
    }

    /// Sets standard output from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_out_lossy(&mut self, system_out: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_out(String::from_utf8_lossy(system_out.as_ref()))
    }

    /// Sets standard error.
    pub fn set_system_err(&mut self, system_err: impl AsRef<str>) -> &mut Self {
        self.system_err = Some(Output::new(system_err.as_ref()));
        self
    }

    /// Sets standard error from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_err_lossy(&mut self, system_err: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_err(String::from_utf8_lossy(system_err.as_ref()))
    }
}

/// Represents the success or failure of a testcase.
#[derive(Clone, Debug)]
pub enum TestcaseStatus {
    /// This testcase passed.
    Success {
        /// Prior runs of the test. These are represented as `flakyFailure` or `flakyError` in the
        /// JUnit XML.
        flaky_runs: Vec<TestRerun>,
    },

    /// This testcase did not pass.
    NonSuccess {
        /// Whether this testcase failed in an expected way (failure) or an unexpected way (error).
        kind: NonSuccessKind,

        /// The failure message.
        message: Option<String>,

        /// The "type" of failure that occurred.
        ty: Option<String>,

        /// The description of the failure.
        ///
        /// This is serialized and deserialized from the text node of the element.
        description: Option<String>,

        /// Test reruns. These are represented as `rerunFailure` or `rerunError` in the JUnit XML.
        reruns: Vec<TestRerun>,
    },

    /// This testcase was not run.
    Skipped {
        /// The skip message.
        message: Option<String>,

        /// The "type" of skip that occurred.
        ty: Option<String>,

        /// The description of the skip.
        ///
        /// This is serialized and deserialized from the text node of the element.
        description: Option<String>,
    },
}

impl TestcaseStatus {
    /// Creates a new `TestcaseStatus` that represents a successful test.
    pub fn success() -> Self {
        TestcaseStatus::Success { flaky_runs: vec![] }
    }

    /// Creates a new `TestcaseStatus` that represents an unsuccessful test.
    pub fn non_success(kind: NonSuccessKind) -> Self {
        TestcaseStatus::NonSuccess {
            kind,
            message: None,
            ty: None,
            description: None,
            reruns: vec![],
        }
    }

    /// Creates a new `TestcaseStatus` that represents a skipped test.
    pub fn skipped() -> Self {
        TestcaseStatus::Skipped {
            message: None,
            ty: None,
            description: None,
        }
    }

    /// Sets the message. No-op if this is a success case.
    pub fn set_message(&mut self, message: impl Into<String>) -> &mut Self {
        let message_mut = match self {
            TestcaseStatus::Success { .. } => return self,
            TestcaseStatus::NonSuccess { message, .. } => message,
            TestcaseStatus::Skipped { message, .. } => message,
        };
        *message_mut = Some(message.into());
        self
    }

    /// Sets the type. No-op if this is a success case.
    pub fn set_type(&mut self, ty: impl Into<String>) -> &mut Self {
        let ty_mut = match self {
            TestcaseStatus::Success { .. } => return self,
            TestcaseStatus::NonSuccess { ty, .. } => ty,
            TestcaseStatus::Skipped { ty, .. } => ty,
        };
        *ty_mut = Some(ty.into());
        self
    }

    /// Sets the description (text node). No-op if this is a success case.
    pub fn set_description(&mut self, description: impl Into<String>) -> &mut Self {
        let description_mut = match self {
            TestcaseStatus::Success { .. } => return self,
            TestcaseStatus::NonSuccess { description, .. } => description,
            TestcaseStatus::Skipped { description, .. } => description,
        };
        *description_mut = Some(description.into());
        self
    }

    /// Adds a rerun or flaky run. No-op if this test was skipped.
    pub fn add_rerun(&mut self, rerun: TestRerun) -> &mut Self {
        self.add_reruns(iter::once(rerun))
    }

    /// Adds reruns or flaky runs. No-op if this test was skipped.
    pub fn add_reruns(&mut self, reruns: impl IntoIterator<Item = TestRerun>) -> &mut Self {
        let reruns_mut = match self {
            TestcaseStatus::Success { flaky_runs } => flaky_runs,
            TestcaseStatus::NonSuccess { reruns, .. } => reruns,
            TestcaseStatus::Skipped { .. } => return self,
        };
        reruns_mut.extend(reruns);
        self
    }
}

/// A rerun of a test.
///
/// This is serialized as `flakyFailure` or `flakyError` for successes, and as `rerunFailure` or
/// `rerunError` for failures/errors.
#[derive(Clone, Debug)]
pub struct TestRerun {
    /// The failure kind: error or failure.
    pub kind: NonSuccessKind,

    /// The time at which this rerun began execution.
    ///
    /// This is not part of the JUnit spec, but may be useful for some tools.
    pub timestamp: Option<DateTime<FixedOffset>>,

    /// The time it took to execute this rerun.
    ///
    /// This is not part of the JUnit spec, but may be useful for some tools.
    pub time: Option<Duration>,

    /// The failure message.
    pub message: Option<String>,

    /// The "type" of failure that occurred.
    pub ty: Option<String>,

    /// The stack trace, if any.
    pub stack_trace: Option<String>,

    /// Data written to standard output while the test rerun was executed.
    pub system_out: Option<Output>,

    /// Data written to standard error while the test rerun was executed.
    pub system_err: Option<Output>,

    /// The description of the failure.
    ///
    /// This is serialized and deserialized from the text node of the element.
    pub description: Option<String>,
}

impl TestRerun {
    /// Creates a new `TestRerun` of the given kind.
    pub fn new(kind: NonSuccessKind) -> Self {
        TestRerun {
            kind,
            timestamp: None,
            time: None,
            message: None,
            ty: None,
            stack_trace: None,
            system_out: None,
            system_err: None,
            description: None,
        }
    }

    /// Sets the start timestamp for this rerun.
    pub fn set_timestamp(&mut self, timestamp: impl Into<DateTime<FixedOffset>>) -> &mut Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Sets the time taken for this rerun.
    pub fn set_time(&mut self, time: Duration) -> &mut Self {
        self.time = Some(time);
        self
    }

    /// Sets the message.
    pub fn set_message(&mut self, message: impl Into<String>) -> &mut Self {
        self.message = Some(message.into());
        self
    }

    /// Sets the type.
    pub fn set_type(&mut self, ty: impl Into<String>) -> &mut Self {
        self.ty = Some(ty.into());
        self
    }

    /// Sets the stack trace.
    pub fn set_stack_trace(&mut self, stack_trace: impl Into<String>) -> &mut Self {
        self.stack_trace = Some(stack_trace.into());
        self
    }

    /// Sets standard output.
    pub fn set_system_out(&mut self, system_out: impl AsRef<str>) -> &mut Self {
        self.system_out = Some(Output::new(system_out.as_ref()));
        self
    }

    /// Sets standard output from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_out_lossy(&mut self, system_out: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_out(String::from_utf8_lossy(system_out.as_ref()))
    }

    /// Sets standard error.
    pub fn set_system_err(&mut self, system_err: impl AsRef<str>) -> &mut Self {
        self.system_err = Some(Output::new(system_err.as_ref()));
        self
    }

    /// Sets standard error from a `Vec<u8>`.
    ///
    /// The output is converted to a string, lossily.
    pub fn set_system_err_lossy(&mut self, system_err: impl AsRef<[u8]>) -> &mut Self {
        self.set_system_err(String::from_utf8_lossy(system_err.as_ref()))
    }

    /// Sets the description of the failure.
    pub fn set_description(&mut self, description: impl Into<String>) -> &mut Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NonSuccessKind {
    /// This is an expected failure. Serialized as `failure`, `flakyFailure` or `rerunFailure`
    /// depending on the context.
    Failure,

    /// This is an unexpected error. Serialized as `error`, `flakyError` or `rerunError` depending
    /// on the context.
    Error,
}

/// Custom properties set during test execution, e.g. environment variables.
#[derive(Clone, Debug)]
pub struct Property {
    /// The name of the property.
    pub name: String,

    /// The value of the property.
    pub value: String,
}

impl Property {
    /// Creates a new `Property` instance.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl<T> From<(T, T)> for Property
where
    T: Into<String>,
{
    fn from((k, v): (T, T)) -> Self {
        Property::new(k, v)
    }
}

/// Represents text that is written out to standard output or standard error during text execution.
///
/// # Encoding
///
/// On Unix platforms, standard output and standard error are typically bytestrings (`Vec<u8>`).
/// However, XUnit assumes that the output is valid Unicode, and this type definition reflects
/// that.
#[derive(Clone, Debug)]
pub struct Output {
    output: Box<str>,
}

impl Output {
    /// Creates a new output, removing any non-printable characters from it.
    pub fn new(output: impl AsRef<str>) -> Self {
        let output = output.as_ref();
        let output = output
            .replace(
                |c| matches!(c, '\x00'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f'),
                "",
            )
            .into_boxed_str();
        Self { output }
    }

    /// Returns the output.
    pub fn as_str(&self) -> &str {
        &self.output
    }

    /// Converts the output into a string.
    pub fn into_string(self) -> String {
        self.output.into_string()
    }
}

impl AsRef<str> for Output {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<Output> for String {
    fn from(output: Output) -> Self {
        output.into_string()
    }
}
