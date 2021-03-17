// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat,
    runner::{TestRunStatus, TestStatus},
    test_list::{TestInstance, TestList},
};
use anyhow::{Context, Result};
use std::{fmt, io};
use structopt::clap::arg_enum;
use termcolor::{BufferWriter, ColorChoice, ColorSpec, WriteColor};

arg_enum! {
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum Color {
        Always,
        Auto,
        Never,
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::Auto
    }
}

impl Color {
    pub(crate) fn color_choice(self, stream: atty::Stream) -> ColorChoice {
        // https://docs.rs/termcolor/1.1.2/termcolor/index.html#detecting-presence-of-a-terminal
        match self {
            Color::Always => ColorChoice::Always,
            Color::Auto => {
                if atty::is(stream) {
                    ColorChoice::Auto
                } else {
                    ColorChoice::Never
                }
            }
            Color::Never => ColorChoice::Never,
        }
    }
}

/// Functionality to report test results to stdout, and in the future to other formats (e.g. JUnit).
pub struct TestReporter {
    stdout: BufferWriter,
    #[allow(dead_code)]
    stderr: BufferWriter,
}

impl TestReporter {
    /// Creates a new instance with the given color choice.
    pub fn new(color: Color) -> Self {
        let stdout = BufferWriter::stdout(color.color_choice(atty::Stream::Stdout));
        let stderr = BufferWriter::stderr(color.color_choice(atty::Stream::Stderr));
        Self { stdout, stderr }
    }

    /// Write a list of tests in the given format.
    pub fn write_list(&self, test_list: &TestList, output_format: OutputFormat) -> Result<()> {
        let mut buffer = self.stdout.buffer();
        test_list.write(output_format, &mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }

    /// Report a test event.
    pub fn report_event(&self, event: TestEvent<'_>) -> Result<()> {
        let mut buffer = self.stdout.buffer();
        event.report(&mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }
}

impl fmt::Debug for TestReporter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TestReporter")
            .field("stdout", &"BufferWriter { .. }")
            .field("stderr", &"BufferWriter { .. }")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum TestEvent<'a> {
    /// A test started running.
    TestStarted {
        /// The test instance that was started.
        test_instance: TestInstance<'a>,
    },

    /// A test finished running.
    TestFinished {
        /// The test instance that finished running.
        test_instance: TestInstance<'a>,

        /// Information about how this test was run.
        run_status: TestRunStatus,
    },

    /// A test was skipped.
    TestSkipped {
        /// The test instance that was skipped.
        test_instance: TestInstance<'a>,
        // TODO: add skip reason
    },
}

impl<'a> TestEvent<'a> {
    /// Report this test event to the given writer.
    pub fn report(&self, mut writer: impl WriteColor) -> io::Result<()> {
        match self {
            TestEvent::TestStarted { .. } => {
                // TODO
            }
            TestEvent::TestFinished {
                test_instance,
                run_status,
            } => {
                // First, print the status.
                match run_status.status {
                    TestStatus::Success => {
                        writer.set_color(&Self::pass_spec())?;
                        write!(writer, "        PASS ")?;
                        writer.reset()?;
                    }
                    TestStatus::Failure => {
                        writer.set_color(&Self::fail_spec())?;
                        write!(writer, "        FAIL ")?;
                        writer.reset()?;
                    }
                    TestStatus::ExecutionFailure => {
                        writer.set_color(&Self::fail_spec())?;
                        write!(writer, "    EXECFAIL ")?;
                        writer.reset()?;
                    }
                }

                // Next, print the time taken.
                // * > means right-align.
                // * 8 is the number of characters to pad to.
                // * .3 means print two digits after the decimal point.
                // TODO: better time printing mechanism than this
                write!(writer, "[{:>8.3?}s] ", run_status.time_taken.as_secs_f64())?;

                // Finally, print the name of the test.
                // TODO: should have a friendly name here
                writeln!(
                    writer,
                    "  {}::{}",
                    test_instance
                        .binary
                        .file_name()
                        .expect("binaries should always have a file name"),
                    test_instance.test_name
                )?;
            }
            TestEvent::TestSkipped { test_instance } => {
                writer.set_color(&Self::skip_spec())?;
                write!(writer, "        SKIP ")?;
                writer.reset()?;

                // TODO: should have a friendly name here
                writeln!(
                    writer,
                    "  {}::{}",
                    test_instance
                        .binary
                        .file_name()
                        .expect("binaries should always have a file name"),
                    test_instance.test_name
                )?;
            }
        }
        Ok(())
    }

    // ---
    // Helper methods
    // ---

    fn pass_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec
            .set_fg(Some(termcolor::Color::Green))
            .set_bold(true);
        color_spec
    }

    fn fail_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec
            .set_fg(Some(termcolor::Color::Red))
            .set_bold(true);
        color_spec
    }

    fn skip_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec
            .set_fg(Some(termcolor::Color::Yellow))
            .set_bold(true);
        color_spec
    }
}
