// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat,
    runner::{RunStats, TestRunStatus, TestStatus},
    test_list::{test_bin_spec, test_name_spec, TestInstance, TestList},
};
use anyhow::{Context, Result};
use camino::Utf8Path;
use std::{fmt, io, io::Write, time::Instant};
use structopt::{clap::arg_enum, StructOpt};
use termcolor::{BufferWriter, ColorChoice, ColorSpec, NoColor, WriteColor};

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

arg_enum! {
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum FailureOutput {
        Immediate,
        // TODO: report failures at the end of the process
        Never,
    }
}

impl Default for FailureOutput {
    fn default() -> Self {
        FailureOutput::Immediate
    }
}

#[derive(Debug, Default, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct ReporterOpts {
    /// Output stdout and stderr on failures
    #[structopt(long, default_value, possible_values = &FailureOutput::variants(), case_insensitive = true)]
    failure_output: FailureOutput,
}

/// Functionality to report test results to stdout, and in the future to other formats (e.g. JUnit).
pub struct TestReporter {
    stdout: BufferWriter,
    #[allow(dead_code)]
    stderr: BufferWriter,
    opts: ReporterOpts,
    friendly_name_width: usize,
}

impl TestReporter {
    /// Creates a new instance with the given color choice.
    pub fn new(test_list: &TestList, color: Color, opts: ReporterOpts) -> Self {
        let stdout = BufferWriter::stdout(color.color_choice(atty::Stream::Stdout));
        let stderr = BufferWriter::stderr(color.color_choice(atty::Stream::Stderr));
        let friendly_name_width = test_list
            .iter()
            .map(|(path, info)| Self::friendly_name(info.friendly_name.as_deref(), path).len())
            .max()
            .unwrap_or_default();
        Self {
            stdout,
            stderr,
            opts,
            friendly_name_width,
        }
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
        self.write_event(event, &mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }

    // ---
    // Helper methods
    // ---

    /// Report this test event to the given writer.
    fn write_event(&self, event: TestEvent<'_>, mut writer: impl WriteColor) -> io::Result<()> {
        match event {
            TestEvent::RunStarted { test_list } => {
                writer.set_color(&Self::pass_spec())?;
                write!(writer, "{:>12} ", "Starting")?;
                writer.reset()?;

                let count_spec = Self::count_spec();

                writer.set_color(&count_spec)?;
                write!(writer, "{}", test_list.run_count())?;
                writer.reset()?;
                write!(writer, " tests across ")?;
                writer.set_color(&count_spec)?;
                write!(writer, "{}", test_list.binary_count())?;
                writer.reset()?;
                write!(writer, " binaries")?;

                let skip_count = test_list.skip_count();
                if skip_count > 0 {
                    write!(writer, " (")?;
                    writer.set_color(&count_spec)?;
                    write!(writer, "{}", skip_count)?;
                    writer.reset()?;
                    write!(writer, " skipped)")?;
                }

                writeln!(writer)?;
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

                // Print the name of the test.
                self.write_instance(test_instance, &mut writer)?;
                writeln!(writer)?;

                // If the test failed to execute, print its output and error status.
                if !run_status.status.is_success()
                    && self.opts.failure_output == FailureOutput::Immediate
                {
                    writer.set_color(&Self::fail_spec())?;
                    write!(writer, "\n--- STDOUT: ")?;
                    self.write_instance(test_instance, NoColor::new(&mut writer))?;
                    writeln!(writer, " ---")?;

                    writer.set_color(&Self::fail_output_spec())?;
                    NoColor::new(&mut writer).write_all(&run_status.stdout)?;

                    writer.set_color(&Self::fail_spec())?;
                    write!(writer, "--- STDERR: ")?;
                    self.write_instance(test_instance, NoColor::new(&mut writer))?;
                    writeln!(writer, " ---")?;

                    writer.set_color(&Self::fail_output_spec())?;
                    NoColor::new(&mut writer).write_all(&run_status.stderr)?;

                    writer.reset()?;
                    writeln!(writer)?;
                }
            }
            TestEvent::TestSkipped { test_instance } => {
                writer.set_color(&Self::skip_spec())?;
                write!(writer, "{:>12} ", "SKIP")?;
                writer.reset()?;
                // same spacing [   0.034s]
                write!(writer, "[         ] ")?;

                self.write_instance(test_instance, &mut writer)?;
                writeln!(writer)?;
            }
            TestEvent::RunBeginCancel { running, reason } => {
                writer.set_color(&Self::fail_spec())?;
                write!(writer, "{:>12} ", "Canceling")?;
                writer.reset()?;
                write!(writer, "due to ")?;

                writer.set_color(&Self::count_spec())?;
                match reason {
                    CancelReason::Signal => write!(writer, "signal")?,
                    // TODO: differentiate between control errors (e.g. fail-fast) and report errors
                    CancelReason::ReportError => write!(writer, "error")?,
                }
                writer.reset()?;
                write!(writer, ", ")?;

                writer.set_color(&Self::count_spec())?;
                write!(writer, "{}", running)?;
                writer.reset()?;
                writeln!(writer, " tests still running")?;
            }

            TestEvent::RunFinished {
                start_time,
                initial_run_count,
                run_stats:
                    RunStats {
                        run_count,
                        passed,
                        failed,
                        exec_failed,
                        skipped,
                    },
            } => {
                let summary_spec = if failed > 0 || exec_failed > 0 {
                    Self::fail_spec()
                } else {
                    Self::pass_spec()
                };
                writer.set_color(&summary_spec)?;
                write!(writer, "{:>12} ", "Summary")?;
                writer.reset()?;

                // Next, print the total time taken.
                // * > means right-align.
                // * 8 is the number of characters to pad to.
                // * .3 means print two digits after the decimal point.
                // TODO: better time printing mechanism than this
                write!(writer, "[{:>8.3?}s] ", start_time.elapsed().as_secs_f64())?;

                let count_spec = Self::count_spec();

                writer.set_color(&count_spec)?;
                write!(writer, "{}", run_count)?;
                if run_count != initial_run_count {
                    write!(writer, "/{}", initial_run_count)?;
                }
                writer.reset()?;
                write!(writer, " tests run: ")?;

                writer.set_color(&count_spec)?;
                write!(writer, "{}", passed)?;
                writer.set_color(&Self::pass_spec())?;
                write!(writer, " passed")?;
                writer.reset()?;
                write!(writer, ", ")?;

                if failed > 0 {
                    writer.set_color(&count_spec)?;
                    write!(writer, "{}", failed)?;
                    writer.set_color(&Self::fail_spec())?;
                    write!(writer, " failed")?;
                    writer.reset()?;
                    write!(writer, ", ")?;
                }

                if exec_failed > 0 {
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

    fn write_instance(
        &self,
        instance: TestInstance<'_>,
        mut writer: impl WriteColor,
    ) -> io::Result<()> {
        let friendly_name = Self::friendly_name(instance.friendly_name, instance.binary);
        writer.set_color(&test_bin_spec())?;
        write!(
            writer,
            "{:>width$}",
            friendly_name,
            width = self.friendly_name_width
        )?;
        writer.reset()?;
        write!(writer, "  ")?;

        // Now look for the part of the test after the last ::, if any.
        let mut splits = instance.name.rsplitn(2, "::");
        let trailing = splits.next().expect("test should have at least 1 element");
        if let Some(rest) = splits.next() {
            write!(writer, "{}::", rest)?;
        }
        writer.set_color(&test_name_spec())?;
        write!(writer, "{}", trailing)?;
        writer.reset()?;

        Ok(())
    }

    fn friendly_name<'a>(friendly_name: Option<&'a str>, binary: &'a Utf8Path) -> &'a str {
        friendly_name.unwrap_or_else(|| {
            binary
                .file_name()
                .expect("test binaries always have file names")
        })
    }

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

    fn fail_output_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec.set_fg(Some(termcolor::Color::Red));
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
        /// The list of tests that will be run.
        ///
        /// The methods on the test list indicate the number of
        test_list: &'a TestList,
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

    /// A cancellation notice was received.
    RunBeginCancel {
        /// The number of tests still running.
        running: usize,

        /// The reason this run was canceled.
        reason: CancelReason,
    },

    /// The test run finished.
    RunFinished {
        /// The time at which the run was started.
        start_time: Instant,

        /// The total number of tests that were expected to be run.
        ///
        /// If the test run is canceled, this will be more than `run_stats.tests_run`.
        initial_run_count: usize,

        /// Statistics for the run.
        run_stats: RunStats,
    },
}

/// The reason why a test run is being cancelled.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CancelReason {
    /// An error occurred while reporting results.
    ReportError,

    /// A termination signal was received.
    Signal,
}
