// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    metadata::MetadataReporter,
    runner::{RunDescribe, RunStats, RunStatuses, TestRunStatus, TestStatus},
    test_filter::MismatchReason,
    test_list::{test_bin_spec, test_name_spec, TestInstance, TestList},
};
use anyhow::{Context, Result};
use nextest_config::{FailureOutput, NextestProfile};
use std::{
    fmt, io,
    io::Write,
    time::{Duration, SystemTime},
};
use structopt::clap::arg_enum;
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

/// Functionality to report test results to stdout and JUnit
pub struct TestReporter<'a> {
    stdout: BufferWriter,
    #[allow(dead_code)]
    stderr: BufferWriter,
    failure_output: FailureOutput,
    binary_id_width: usize,

    // TODO: too many concerns mixed up here. Should have a better model, probably in conjunction
    // with factoring out the different reporters below.
    failing_tests: Vec<(TestInstance<'a>, TestRunStatus)>,

    metadata_reporter: MetadataReporter<'a>,
}

impl<'a> TestReporter<'a> {
    /// Creates a new instance with the given color choice.
    pub fn new(test_list: &TestList, color: Color, profile: &'a NextestProfile<'a>) -> Self {
        let stdout = BufferWriter::stdout(color.color_choice(atty::Stream::Stdout));
        let stderr = BufferWriter::stderr(color.color_choice(atty::Stream::Stderr));
        let binary_id_width = test_list
            .iter()
            .map(|(_, info)| info.binary_id.len())
            .max()
            .unwrap_or_default();
        let metadata_reporter = MetadataReporter::new(profile);
        Self {
            stdout,
            stderr,
            failure_output: profile.failure_output(),
            failing_tests: vec![],
            binary_id_width,
            metadata_reporter,
        }
    }

    /// Report a test event.
    pub fn report_event(&mut self, event: TestEvent<'a>) -> Result<()> {
        let mut buffer = self.stdout.buffer();
        self.write_event(event, &mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }

    // ---
    // Helper methods
    // ---

    /// Report this test event to the given writer.
    fn write_event(&mut self, event: TestEvent<'a>, mut writer: impl WriteColor) -> Result<()> {
        match &event {
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
            TestEvent::TestRetry {
                test_instance,
                run_status,
            } => {
                writer.set_color(&Self::retry_spec())?;
                let retry_string =
                    format!("{}/{} RETRY", run_status.attempt, run_status.total_attempts);
                write!(writer, "{:>12} ", retry_string)?;
                writer.reset()?;

                // Next, print the time taken.
                self.write_duration(run_status.time_taken, &mut writer)?;

                // Print the name of the test.
                self.write_instance(*test_instance, &mut writer)?;
                writeln!(writer)?;

                // This test is guaranteed to have failed.
                assert!(
                    !run_status.status.is_success(),
                    "only failing tests are retried"
                );
                if self.failure_output.is_immediate() {
                    self.write_run_status(test_instance, run_status, true, &mut writer)?;
                }

                // The final output doesn't show retries.
            }
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
            } => {
                // First, print the status.
                let last_status = match run_statuses.describe() {
                    RunDescribe::Success { run_status } => {
                        writer.set_color(&Self::pass_spec())?;
                        write!(writer, "{:>12} ", "PASS")?;
                        run_status
                    }
                    RunDescribe::Flaky { last_status, .. } => {
                        // Use the skip color to also represent a flaky test.
                        writer.set_color(&Self::skip_spec())?;
                        write!(
                            writer,
                            "{:>12} ",
                            format!("TRY {} PASS", last_status.attempt)
                        )?;
                        last_status
                    }
                    RunDescribe::Failure { last_status, .. } => {
                        writer.set_color(&Self::fail_spec())?;
                        let status_str = match last_status.status {
                            TestStatus::Fail => "FAIL",
                            TestStatus::ExecFail => "XFAIL",
                            TestStatus::Pass => unreachable!("this is a failing test"),
                        };
                        if last_status.attempt == 1 {
                            write!(writer, "{:>12} ", status_str)?;
                        } else {
                            write!(
                                writer,
                                "{:>12} ",
                                format!("TRY {} {}", last_status.attempt, status_str)
                            )?;
                        }
                        last_status
                    }
                };

                writer.reset()?;

                // Next, print the time taken.
                self.write_duration(last_status.time_taken, &mut writer)?;

                // Print the name of the test.
                self.write_instance(*test_instance, &mut writer)?;
                writeln!(writer)?;

                // If the test failed to execute, print its output and error status.
                if !last_status.status.is_success() {
                    if self.failure_output.is_immediate() {
                        self.write_run_status(test_instance, last_status, false, &mut writer)?;
                    }
                    if self.failure_output.is_final() {
                        self.failing_tests
                            .push((*test_instance, last_status.clone()));
                    }
                }
            }
            TestEvent::TestSkipped {
                test_instance,
                reason: _reason,
            } => {
                writer.set_color(&Self::skip_spec())?;
                write!(writer, "{:>12} ", "SKIP")?;
                writer.reset()?;
                // same spacing [   0.034s]
                write!(writer, "[         ] ")?;

                self.write_instance(*test_instance, &mut writer)?;
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
                start_time: _start_time,
                elapsed,
                run_stats:
                    RunStats {
                        initial_run_count,
                        final_run_count,
                        passed,
                        flaky,
                        failed,
                        exec_failed,
                        skipped,
                    },
            } => {
                let summary_spec = if *failed > 0 || *exec_failed > 0 {
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
                write!(writer, "[{:>8.3?}s] ", elapsed.as_secs_f64())?;

                let count_spec = Self::count_spec();

                writer.set_color(&count_spec)?;
                write!(writer, "{}", final_run_count)?;
                if final_run_count != initial_run_count {
                    write!(writer, "/{}", initial_run_count)?;
                }
                writer.reset()?;
                write!(writer, " tests run: ")?;

                writer.set_color(&count_spec)?;
                write!(writer, "{}", passed)?;
                writer.set_color(&Self::pass_spec())?;
                write!(writer, " passed")?;
                writer.reset()?;
                if *flaky > 0 {
                    write!(writer, " (")?;
                    writer.set_color(&count_spec)?;
                    write!(writer, "{}", flaky)?;
                    writer.set_color(&Self::skip_spec())?;
                    write!(writer, " flaky")?;
                    writer.reset()?;
                    write!(writer, ")")?;
                }
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

                for (test_instance, run_status) in &self.failing_tests {
                    self.write_run_status(test_instance, run_status, false, &mut writer)?;
                }
            }
        }

        self.metadata_reporter.write_event(event)?;
        Ok(())
    }

    fn write_instance(
        &self,
        instance: TestInstance<'a>,
        mut writer: impl WriteColor,
    ) -> io::Result<()> {
        writer.set_color(&test_bin_spec())?;
        write!(
            writer,
            "{:>width$}",
            instance.binary_id,
            width = self.binary_id_width
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

    fn write_duration(&self, duration: Duration, mut writer: impl WriteColor) -> io::Result<()> {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print two digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "[{:>8.3?}s] ", duration.as_secs_f64())
    }

    fn write_run_status(
        &self,
        test_instance: &TestInstance<'a>,
        run_status: &TestRunStatus,
        is_retry: bool,
        mut writer: impl WriteColor,
    ) -> io::Result<()> {
        let (header_spec, output_spec) = if is_retry {
            (Self::retry_spec(), Self::retry_output_spec())
        } else {
            (Self::fail_spec(), Self::fail_output_spec())
        };

        writer.set_color(&header_spec)?;
        write!(writer, "\n--- ")?;
        self.write_attempt(run_status, &mut writer)?;
        write!(writer, " STDOUT: ")?;
        self.write_instance(*test_instance, NoColor::new(&mut writer))?;
        writeln!(writer, " ---")?;

        writer.set_color(&output_spec)?;
        NoColor::new(&mut writer).write_all(run_status.stdout())?;

        writer.set_color(&header_spec)?;
        write!(writer, "--- ")?;
        self.write_attempt(run_status, &mut writer)?;
        write!(writer, " STDERR: ")?;
        self.write_instance(*test_instance, NoColor::new(&mut writer))?;
        writeln!(writer, " ---")?;

        writer.set_color(&output_spec)?;
        NoColor::new(&mut writer).write_all(run_status.stderr())?;

        writer.reset()?;
        writeln!(writer)
    }

    fn write_attempt(&self, run_status: &TestRunStatus, mut writer: impl Write) -> io::Result<()> {
        if run_status.total_attempts > 1 {
            write!(writer, "TRY {}", run_status.attempt)?;
        }
        Ok(())
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

    fn retry_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec
            .set_fg(Some(termcolor::Color::Magenta))
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

    fn retry_output_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec.set_fg(Some(termcolor::Color::Magenta));
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

impl<'a> fmt::Debug for TestReporter<'a> {
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

    // TODO: add events for BinaryStarted and BinaryFinished? May want a slightly different way to
    // do things, maybe a couple of reporter traits (one for the run as a whole and one for each
    // binary).
    /// A test started running.
    TestStarted {
        /// The test instance that was started.
        test_instance: TestInstance<'a>,
    },

    /// A test failed and is being retried.
    ///
    /// This event does not occur on the final run of a failing test.
    TestRetry {
        /// The test instance that is being retried.
        test_instance: TestInstance<'a>,

        /// The status of this attempt to run the test. Will never be success.
        run_status: TestRunStatus,
    },

    /// A test finished running.
    TestFinished {
        /// The test instance that finished running.
        test_instance: TestInstance<'a>,

        /// Information about all the runs for this test.
        run_statuses: RunStatuses,
    },

    /// A test was skipped.
    TestSkipped {
        /// The test instance that was skipped.
        test_instance: TestInstance<'a>,

        /// The reason this test was skipped.
        reason: MismatchReason,
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
        start_time: SystemTime,

        /// The amount of time it took for the tests to run.
        elapsed: Duration,

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
