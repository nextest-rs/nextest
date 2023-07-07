// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Serialize a `Report`.

use crate::{
    NonSuccessKind, Output, Property, Report, SerializeError, TestCase, TestCaseStatus, TestRerun,
    TestSuite,
};
use chrono::{DateTime, FixedOffset};
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use std::{io, time::Duration};

static TESTSUITES_TAG: &str = "testsuites";
static TESTSUITE_TAG: &str = "testsuite";
static TESTCASE_TAG: &str = "testcase";
static PROPERTIES_TAG: &str = "properties";
static PROPERTY_TAG: &str = "property";
static FAILURE_TAG: &str = "failure";
static ERROR_TAG: &str = "error";
static FLAKY_FAILURE_TAG: &str = "flakyFailure";
static FLAKY_ERROR_TAG: &str = "flakyError";
static RERUN_FAILURE_TAG: &str = "rerunFailure";
static RERUN_ERROR_TAG: &str = "rerunError";
static STACK_TRACE_TAG: &str = "stackTrace";
static SKIPPED_TAG: &str = "skipped";
static SYSTEM_OUT_TAG: &str = "system-out";
static SYSTEM_ERR_TAG: &str = "system-err";

pub(crate) fn serialize_report(
    report: &Report,
    writer: impl io::Write,
) -> Result<(), SerializeError> {
    let mut writer = Writer::new_with_indent(writer, b' ', 4);

    let decl = BytesDecl::new("1.0", Some("UTF-8"), None);
    writer.write_event(Event::Decl(decl))?;

    serialize_report_impl(report, &mut writer)?;

    // Add a trailing newline.
    Ok(writer.write_indent()?)
}

pub(crate) fn serialize_report_impl(
    report: &Report,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    // Use the destructuring syntax to ensure that all fields are handled.
    let Report {
        name,
        uuid,
        timestamp,
        time,
        tests,
        failures,
        errors,
        test_suites,
    } = report;

    let mut testsuites_tag = BytesStart::new(TESTSUITES_TAG);
    testsuites_tag.extend_attributes([
        ("name", name.as_str()),
        ("tests", tests.to_string().as_str()),
        ("failures", failures.to_string().as_str()),
        ("errors", errors.to_string().as_str()),
    ]);
    if let Some(uuid) = uuid {
        testsuites_tag.push_attribute(("uuid", uuid.to_string().as_str()));
    }
    if let Some(timestamp) = timestamp {
        serialize_timestamp(&mut testsuites_tag, timestamp);
    }
    if let Some(time) = time {
        serialize_time(&mut testsuites_tag, time);
    }
    writer.write_event(Event::Start(testsuites_tag))?;

    for test_suite in test_suites {
        serialize_test_suite(test_suite, writer)?;
    }

    serialize_end_tag(TESTSUITES_TAG, writer)?;
    writer.write_event(Event::Eof)?;

    Ok(())
}

pub(crate) fn serialize_test_suite(
    test_suite: &TestSuite,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    // Use the destructuring syntax to ensure that all fields are handled.
    let TestSuite {
        name,
        tests,
        disabled,
        errors,
        failures,
        time,
        timestamp,
        test_cases,
        properties,
        system_out,
        system_err,
        extra,
    } = test_suite;

    let mut test_suite_tag = BytesStart::new(TESTSUITE_TAG);
    test_suite_tag.extend_attributes([
        ("name", name.as_str()),
        ("tests", tests.to_string().as_str()),
        ("disabled", disabled.to_string().as_str()),
        ("errors", errors.to_string().as_str()),
        ("failures", failures.to_string().as_str()),
    ]);

    if let Some(timestamp) = timestamp {
        serialize_timestamp(&mut test_suite_tag, timestamp);
    }
    if let Some(time) = time {
        serialize_time(&mut test_suite_tag, time);
    }

    for (k, v) in extra {
        test_suite_tag.push_attribute((k.as_str(), v.as_str()));
    }

    writer.write_event(Event::Start(test_suite_tag))?;

    if !properties.is_empty() {
        serialize_empty_start_tag(PROPERTIES_TAG, writer)?;
        for property in properties {
            serialize_property(property, writer)?;
        }
        serialize_end_tag(PROPERTIES_TAG, writer)?;
    }

    for test_case in test_cases {
        serialize_test_case(test_case, writer)?;
    }

    if let Some(system_out) = system_out {
        serialize_output(system_out, SYSTEM_OUT_TAG, writer)?;
    }
    if let Some(system_err) = system_err {
        serialize_output(system_err, SYSTEM_ERR_TAG, writer)?;
    }

    serialize_end_tag(TESTSUITE_TAG, writer)?;
    Ok(())
}

fn serialize_property(
    property: &Property,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let mut property_tag = BytesStart::new(PROPERTY_TAG);
    property_tag.extend_attributes([
        ("name", property.name.as_str()),
        ("value", property.value.as_str()),
    ]);

    writer.write_event(Event::Empty(property_tag))
}

fn serialize_test_case(
    test_case: &TestCase,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let TestCase {
        name,
        classname,
        assertions,
        timestamp,
        time,
        status,
        system_out,
        system_err,
        extra,
        properties,
    } = test_case;

    let mut testcase_tag = BytesStart::new(TESTCASE_TAG);
    testcase_tag.extend_attributes([("name", name.as_str())]);
    if let Some(classname) = classname {
        testcase_tag.push_attribute(("classname", classname.as_str()));
    }
    if let Some(assertions) = assertions {
        testcase_tag.push_attribute(("assertions", format!("{assertions}").as_str()));
    }

    if let Some(timestamp) = timestamp {
        serialize_timestamp(&mut testcase_tag, timestamp);
    }
    if let Some(time) = time {
        serialize_time(&mut testcase_tag, time);
    }

    for (k, v) in extra {
        testcase_tag.push_attribute((k.as_str(), v.as_str()));
    }
    writer.write_event(Event::Start(testcase_tag))?;

    if !properties.is_empty() {
        serialize_empty_start_tag(PROPERTIES_TAG, writer)?;
        for property in properties {
            serialize_property(property, writer)?;
        }
        serialize_end_tag(PROPERTIES_TAG, writer)?;
    }

    match status {
        TestCaseStatus::Success { flaky_runs } => {
            for rerun in flaky_runs {
                serialize_rerun(rerun, FlakyOrRerun::Flaky, writer)?;
            }
        }
        TestCaseStatus::NonSuccess {
            kind,
            message,
            ty,
            description,
            reruns,
        } => {
            let tag_name = match kind {
                NonSuccessKind::Failure => FAILURE_TAG,
                NonSuccessKind::Error => ERROR_TAG,
            };
            serialize_status(
                message.as_deref(),
                ty.as_deref(),
                description.as_deref(),
                tag_name,
                writer,
            )?;
            for rerun in reruns {
                serialize_rerun(rerun, FlakyOrRerun::Rerun, writer)?;
            }
        }
        TestCaseStatus::Skipped {
            message,
            ty,
            description,
        } => {
            serialize_status(
                message.as_deref(),
                ty.as_deref(),
                description.as_deref(),
                SKIPPED_TAG,
                writer,
            )?;
        }
    }

    if let Some(system_out) = system_out {
        serialize_output(system_out, SYSTEM_OUT_TAG, writer)?;
    }
    if let Some(system_err) = system_err {
        serialize_output(system_err, SYSTEM_ERR_TAG, writer)?;
    }

    serialize_end_tag(TESTCASE_TAG, writer)?;

    Ok(())
}

fn serialize_status(
    message: Option<&str>,
    ty: Option<&str>,
    description: Option<&str>,
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let mut tag = BytesStart::new(tag_name);
    if let Some(message) = message {
        tag.push_attribute(("message", message));
    }
    if let Some(ty) = ty {
        tag.push_attribute(("type", ty));
    }

    match description {
        Some(description) => {
            writer.write_event(Event::Start(tag))?;
            writer.write_event(Event::Text(BytesText::new(description)))?;
            serialize_end_tag(tag_name, writer)?;
        }
        None => {
            writer.write_event(Event::Empty(tag))?;
        }
    }

    Ok(())
}

#[derive(Copy, Clone, Debug)]
enum FlakyOrRerun {
    Flaky,
    Rerun,
}

fn serialize_rerun(
    rerun: &TestRerun,
    flaky_or_rerun: FlakyOrRerun,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let TestRerun {
        timestamp,
        time,
        kind,
        message,
        ty,
        stack_trace,
        system_out,
        system_err,
        description,
    } = rerun;

    let tag_name = match (flaky_or_rerun, *kind) {
        (FlakyOrRerun::Flaky, NonSuccessKind::Failure) => FLAKY_FAILURE_TAG,
        (FlakyOrRerun::Flaky, NonSuccessKind::Error) => FLAKY_ERROR_TAG,
        (FlakyOrRerun::Rerun, NonSuccessKind::Failure) => RERUN_FAILURE_TAG,
        (FlakyOrRerun::Rerun, NonSuccessKind::Error) => RERUN_ERROR_TAG,
    };

    let mut tag = BytesStart::new(tag_name);
    if let Some(timestamp) = timestamp {
        serialize_timestamp(&mut tag, timestamp);
    }
    if let Some(time) = time {
        serialize_time(&mut tag, time);
    }
    if let Some(message) = message {
        tag.push_attribute(("message", message.as_str()));
    }
    if let Some(ty) = ty {
        tag.push_attribute(("type", ty.as_str()));
    }

    writer.write_event(Event::Start(tag))?;

    let mut needs_indent = false;
    if let Some(description) = description {
        writer.write_event(Event::Text(BytesText::new(description)))?;
        needs_indent = true;
    }

    // Note that the stack trace, system out and system err should occur in this order according
    // to the reference schema.
    if let Some(stack_trace) = stack_trace {
        if needs_indent {
            writer.write_indent()?;
            needs_indent = false;
        }
        serialize_empty_start_tag(STACK_TRACE_TAG, writer)?;
        writer.write_event(Event::Text(BytesText::new(stack_trace)))?;
        serialize_end_tag(STACK_TRACE_TAG, writer)?;
    }

    if let Some(system_out) = system_out {
        if needs_indent {
            writer.write_indent()?;
            needs_indent = false;
        }
        serialize_output(system_out, SYSTEM_OUT_TAG, writer)?;
    }
    if let Some(system_err) = system_err {
        if needs_indent {
            writer.write_indent()?;
            // needs_indent = false;
        }
        serialize_output(system_err, SYSTEM_ERR_TAG, writer)?;
    }

    serialize_end_tag(tag_name, writer)?;

    Ok(())
}

fn serialize_output(
    output: &Output,
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    serialize_empty_start_tag(tag_name, writer)?;

    let text = BytesText::new(output.as_str());
    writer.write_event(Event::Text(text))?;

    serialize_end_tag(tag_name, writer)?;

    Ok(())
}

fn serialize_empty_start_tag(
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let tag = BytesStart::new(tag_name);
    writer.write_event(Event::Start(tag))
}

fn serialize_end_tag(
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let end_tag = BytesEnd::new(tag_name);
    writer.write_event(Event::End(end_tag))
}

fn serialize_timestamp(tag: &mut BytesStart<'_>, timestamp: &DateTime<FixedOffset>) {
    // The format string is obtained from https://docs.rs/chrono/0.4.19/chrono/format/strftime/index.html#fn8.
    // The only change is that this only prints timestamps up to 3 decimal places (to match times).
    static RFC_3339_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f%:z";
    tag.push_attribute((
        "timestamp",
        format!("{}", timestamp.format(RFC_3339_FORMAT)).as_str(),
    ));
}

// Serialize time as seconds with 3 decimal points.
fn serialize_time(tag: &mut BytesStart<'_>, time: &Duration) {
    tag.push_attribute(("time", format!("{:.3}", time.as_secs_f64()).as_str()));
}
