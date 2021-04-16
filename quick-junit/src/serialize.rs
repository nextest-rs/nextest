// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Serialize a `Report`.

use crate::{Output, Property, Report, Testcase, TestcaseStatus, Testsuite};
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use std::{array, io, time::Duration};

static TESTSUITES_TAG: &str = "testsuites";
static TESTSUITE_TAG: &str = "testsuite";
static TESTCASE_TAG: &str = "testcase";
static PROPERTIES_TAG: &str = "properties";
static PROPERTY_TAG: &str = "property";
static FAILURE_TAG: &str = "failure";
static ERROR_TAG: &str = "error";
static SKIPPED_TAG: &str = "skipped";
static SYSTEM_OUT_TAG: &str = "system-out";
static SYSTEM_ERR_TAG: &str = "system-err";

pub(crate) fn serialize_report(report: &Report, writer: impl io::Write) -> quick_xml::Result<()> {
    let mut writer = Writer::new_with_indent(writer, b' ', 4);

    let decl = BytesDecl::new(b"1.0", Some(b"UTF-8"), None);
    writer.write_event(Event::Decl(decl))?;

    serialize_report_impl(report, &mut writer)?;

    // Add a trailing newline.
    writer.write_indent()
}

pub(crate) fn serialize_report_impl(
    report: &Report,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    // Use the destructuring syntax to ensure that all fields are handled.
    let Report {
        name,
        time,
        tests,
        failures,
        errors,
        testsuites,
    } = report;

    let mut testsuites_tag = BytesStart::borrowed_name(TESTSUITES_TAG.as_bytes());
    testsuites_tag.extend_attributes(array::IntoIter::new([
        ("name", name.as_str()),
        ("tests", tests.to_string().as_str()),
        ("failures", failures.to_string().as_str()),
        ("errors", errors.to_string().as_str()),
    ]));
    if let Some(time) = time {
        testsuites_tag.push_attribute(("time", serialize_time(time).as_str()));
    }
    writer.write_event(Event::Start(testsuites_tag))?;

    for testsuite in testsuites {
        serialize_testsuite(testsuite, writer)?;
    }

    serialize_end_tag(TESTSUITES_TAG, writer)?;
    writer.write_event(Event::Eof)?;

    Ok(())
}

pub(crate) fn serialize_testsuite(
    testsuite: &Testsuite,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    // Use the destructuring syntax to ensure that all fields are handled.
    let Testsuite {
        name,
        tests,
        disabled,
        errors,
        failures,
        time,
        timestamp,
        testcases,
        properties,
        system_out,
        system_err,
        extra,
    } = testsuite;

    let mut testsuite_tag = BytesStart::borrowed_name(TESTSUITE_TAG.as_bytes());
    testsuite_tag.extend_attributes(array::IntoIter::new([
        ("name", name.as_str()),
        ("tests", tests.to_string().as_str()),
        ("disabled", disabled.to_string().as_str()),
        ("errors", errors.to_string().as_str()),
        ("failures", failures.to_string().as_str()),
    ]));
    if let Some(time) = time {
        testsuite_tag.push_attribute(("time", serialize_time(time).as_str()));
    }
    if let Some(timestamp) = timestamp {
        testsuite_tag.push_attribute(("timestamp", format!("{}", timestamp.format("%+")).as_str()));
    }

    for (k, v) in extra {
        testsuite_tag.push_attribute((k.as_str(), v.as_str()));
    }

    writer.write_event(Event::Start(testsuite_tag))?;

    if !properties.is_empty() {
        serialize_empty_start_tag(PROPERTIES_TAG, writer)?;
        for property in properties {
            serialize_property(property, writer)?;
        }
        serialize_end_tag(PROPERTIES_TAG, writer)?;
    }

    for testcase in testcases {
        serialize_testcase(testcase, writer)?;
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
    let mut property_tag = BytesStart::borrowed_name(PROPERTY_TAG.as_bytes());
    property_tag.extend_attributes(array::IntoIter::new([
        ("name", property.name.as_str()),
        ("value", property.value.as_str()),
    ]));

    writer.write_event(Event::Empty(property_tag))
}

fn serialize_testcase(
    testcase: &Testcase,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let Testcase {
        name,
        classname,
        assertions,
        time,
        status,
        system_out,
        system_err,
        extra,
    } = testcase;

    let mut testcase_tag = BytesStart::borrowed_name(TESTCASE_TAG.as_bytes());
    testcase_tag.extend_attributes(array::IntoIter::new([("name", name.as_str())]));
    if let Some(classname) = classname {
        testcase_tag.push_attribute(("classname", classname.as_str()));
    }
    if let Some(assertions) = assertions {
        testcase_tag.push_attribute(("assertions", format!("{}", assertions).as_str()));
    }
    if let Some(time) = time {
        testcase_tag.push_attribute(("time", serialize_time(time).as_str()));
    }
    for (k, v) in extra {
        testcase_tag.push_attribute((k.as_str(), v.as_str()));
    }
    writer.write_event(Event::Start(testcase_tag))?;

    match status {
        TestcaseStatus::Success => {}
        TestcaseStatus::Failure {
            message,
            ty,
            description,
        } => {
            serialize_status(
                message.as_deref(),
                ty.as_deref(),
                description.as_deref(),
                FAILURE_TAG,
                writer,
            )?;
        }
        TestcaseStatus::Error {
            message,
            ty,
            description,
        } => {
            serialize_status(
                message.as_deref(),
                ty.as_deref(),
                description.as_deref(),
                ERROR_TAG,
                writer,
            )?;
        }
        TestcaseStatus::Skipped {
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
    let mut tag = BytesStart::borrowed_name(tag_name.as_bytes());
    if let Some(message) = message {
        tag.push_attribute(("message", message));
    }
    if let Some(ty) = ty {
        tag.push_attribute(("type", ty));
    }

    match description {
        Some(description) => {
            writer.write_event(Event::Start(tag))?;
            writer.write_event(Event::Text(BytesText::from_plain_str(description)))?;
            serialize_end_tag(tag_name, writer)?;
        }
        None => {
            writer.write_event(Event::Empty(tag))?;
        }
    }

    Ok(())
}

fn serialize_output(
    output: &Output,
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    serialize_empty_start_tag(tag_name, writer)?;

    let text = BytesText::from_plain_str(&output.output);
    writer.write_event(Event::Text(text))?;

    serialize_end_tag(tag_name, writer)?;

    Ok(())
}

fn serialize_empty_start_tag(
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let tag = BytesStart::borrowed_name(tag_name.as_bytes());
    writer.write_event(Event::Start(tag))
}

fn serialize_end_tag(
    tag_name: &'static str,
    writer: &mut Writer<impl io::Write>,
) -> quick_xml::Result<()> {
    let end_tag = BytesEnd::borrowed(tag_name.as_bytes());
    writer.write_event(Event::End(end_tag))
}

// Serialize time as seconds with 3 decimal points.
fn serialize_time(time: &Duration) -> String {
    format!("{:.3}", time.as_secs_f64())
}
