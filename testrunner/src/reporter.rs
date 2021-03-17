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
    /// The test run started.
    RunStarted {
        /// The number of binaries that will be run.
        binary_count: usize,

        /// The total number of tests that will be run.
        test_count: usize,
    },

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

    /// The test run finished.
    RunFinished {
        /// The total number of tests that were run.
        test_count: usize,

        /// The number of tests that passed.
        passed: usize,

        /// The number of tests that failed.
        failed: usize,

        /// The number of tests that encountered an execution failure.
        exec_failed: usize,

        /// The number of tests that were skipped.
        skipped: usize,
    },
}

impl<'a> TestEvent<'a> {
    /// Report this test event to the given writer.
    pub fn report(&self, mut writer: impl WriteColor) -> io::Result<()> {
        match self {
            TestEvent::RunStarted {
                test_count,
                binary_count,
            } => {
                writer.set_color(&Self::pass_spec())?;
                write!(writer, "{:>12} ", "Starting")?;
                writer.reset()?;

                let count_spec = Self::count_spec();

                writer.set_color(&count_spec)?;
                write!(writer, "{}", test_count)?;
                writer.reset()?;
                write!(writer, " tests across ")?;
                writer.set_color(&count_spec)?;
                write!(writer, "{}", binary_count)?;
                writer.reset()?;
                writeln!(writer, " binaries")?;
            }
            TestEvent::TestStarted { .. } => {
                // TODO
            }
            TestEvent::TestFinished {
                test_instance,
                run_status,
            } => {
                // First, print the status.
                match run_status.status {
                    TestStatus::Pass => {
                        writer.set_color(&Self::pass_spec())?;
                    }
                    TestStatus::Fail | TestStatus::ExecFail => {
                        writer.set_color(&Self::fail_spec())?;
                    }
                }

                write!(writer, "{:>12} ", run_status.status)?;
                writer.reset()?;

                // Next, print the time taken.
                // * > means right-align.
                // * 8 is the number of characters to pad to.
                // * .3 means print two digits after the decimal point.
                // TODO: better time printing mechanism than this
                write!(writer, "[{:>8.3?}s] ", run_status.time_taken.as_secs_f64())?;

                // Finally, print the name of the test.
                test_instance.write(&mut writer)?;
                writeln!(writer)?;
            }
            TestEvent::TestSkipped { test_instance } => {
                writer.set_color(&Self::skip_spec())?;
                write!(writer, "{:>12} ", &"SKIP")?;
                writer.reset()?;

                test_instance.write(&mut writer)?;
                writeln!(writer)?;
            }
            TestEvent::RunFinished {
                test_count,
                passed,
                failed,
                exec_failed,
                skipped,
            } => {
                let summary_spec = if *failed > 0 || *exec_failed > 0 {
                    Self::fail_spec()
                } else {
                    Self::pass_spec()
                };
                writer.set_color(&summary_spec)?;
                write!(writer, "{:>12} ", "Summary")?;
                writer.reset()?;

                let count_spec = Self::count_spec();

                writer.set_color(&count_spec)?;
                write!(writer, "{}", test_count)?;
                writer.reset()?;
                write!(writer, " tests run: ")?;

                writer.set_color(&count_spec)?;
                write!(writer, "{}", passed)?;
                writer.set_color(&Self::pass_spec())?;
                write!(writer, " passed")?;
                writer.reset()?;
                write!(writer, ", ")?;

                if *failed > 0 {
                    writer.set_color(&count_spec)?;
                    write!(writer, "{}", failed)?;
                    writer.set_color(&Self::fail_spec())?;
                    write!(writer, " failed")?;
                    writer.reset()?;
                    write!(writer, ", ")?;
                }

                if *exec_failed > 0 {
                    writer.set_color(&count_spec)?;
                    write!(writer, "{}", exec_failed)?;
                    writer.set_color(&Self::fail_spec())?;
                    write!(writer, " exec failed")?;
                    writer.reset()?;
                    write!(writer, ", ")?;
                }

                writer.set_color(&count_spec)?;
                write!(writer, "{}", skipped)?;
                writer.set_color(&Self::skip_spec())?;
                write!(writer, " skipped")?;
                writer.reset()?;

                writeln!(writer)?;

                // TODO: print information about failing tests
            }
        }
        Ok(())
    }

    // ---
    // Helper methods
    // ---

    fn count_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec.set_bold(true);
        color_spec
    }

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
