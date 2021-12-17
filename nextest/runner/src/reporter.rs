// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::WriteEventError,
    metadata::MetadataReporter,
    runner::{RunDescribe, RunStats, RunStatuses, TestRunStatus, TestStatus},
    test_filter::MismatchReason,
    test_list::{TestInstance, TestList},
};
use debug_ignore::DebugIgnore;
use nextest_config::{FailureOutput, NextestProfile, StatusLevel};
use owo_colors::{OwoColorize, Style};
use std::{
    fmt, io,
    io::Write,
    time::{Duration, SystemTime},
};
use structopt::StructOpt;

#[derive(Debug, Default, StructOpt)]
#[structopt(rename_all = "kebab-case")]
pub struct ReporterOpts {
    /// Output stdout and stderr on failures
    #[structopt(long, possible_values = &FailureOutput::variants(), case_insensitive = true)]
    failure_output: Option<FailureOutput>,
    /// Test statuses to output
    #[structopt(long, possible_values = StatusLevel::variants(), case_insensitive = true)]
    status_level: Option<StatusLevel>,
}

/// Functionality to report test results to stderr and JUnit
pub struct TestReporter<'a> {
    status_level: StatusLevel,
    failure_output: FailureOutput,
    binary_id_width: usize,
    styles: Box<Styles>,

    // TODO: too many concerns mixed up here. Should have a better model, probably in conjunction
    // with factoring out the different reporters below.
    failing_tests: DebugIgnore<Vec<(TestInstance<'a>, TestRunStatus)>>,

    metadata_reporter: MetadataReporter<'a>,
}

impl<'a> TestReporter<'a> {
    /// Creates a new instance with the given profile.
    pub fn new(test_list: &TestList, profile: &'a NextestProfile<'a>, opts: &ReporterOpts) -> Self {
        let styles = Box::new(Styles::default());
        let binary_id_width = test_list
            .iter()
            .map(|(_, info)| info.binary_id.len())
            .max()
            .unwrap_or_default();
        let metadata_reporter = MetadataReporter::new(profile);
        Self {
            status_level: opts.status_level.unwrap_or_else(|| profile.status_level()),
            failure_output: opts
                .failure_output
                .unwrap_or_else(|| profile.failure_output()),
            failing_tests: DebugIgnore(vec![]),
            styles,
            binary_id_width,
            metadata_reporter,
        }
    }

    pub fn colorize(&mut self) {
        self.styles.colorize();
    }

    /// Report a test event.
    pub fn report_event(
        &mut self,
        event: TestEvent<'a>,
        writer: impl Write,
    ) -> Result<(), WriteEventError> {
        self.write_event(event, writer)
    }

    // ---
    // Helper methods
    // ---

    /// Report this test event to the given writer.
    fn write_event(
        &mut self,
        event: TestEvent<'a>,
        writer: impl Write,
    ) -> Result<(), WriteEventError> {
        self.write_event_impl(&event, writer)
            .map_err(WriteEventError::Io)?;
        self.metadata_reporter.write_event(event)?;
        Ok(())
    }

    fn write_event_impl(
        &mut self,
        event: &TestEvent<'a>,
        mut writer: impl Write,
    ) -> io::Result<()> {
        match event {
            TestEvent::RunStarted { test_list } => {
                write!(writer, "{:>12} ", "Starting".style(self.styles.pass))?;

                let count_style = self.styles.count;

                write!(
                    writer,
                    "{} tests across {} binaries",
                    test_list.run_count().style(count_style),
                    test_list.binary_count().style(count_style),
                )?;

                let skip_count = test_list.skip_count();
                if skip_count > 0 {
                    write!(writer, " ({} skipped)", skip_count.style(count_style))?;
                }

                writeln!(writer)?;
            }
            TestEvent::TestStarted { .. } => {
                // TODO
            }
            TestEvent::TestSlow {
                test_instance,
                elapsed,
            } => {
                if self.status_level >= StatusLevel::Slow {
                    write!(writer, "{:>12} ", "SLOW".style(self.styles.skip))?;
                    self.write_slow_duration(*elapsed, &mut writer)?;
                    self.write_instance(*test_instance, &mut writer)?;
                    writeln!(writer)?;
                }
            }
            TestEvent::TestRetry {
                test_instance,
                run_status,
            } => {
                if self.status_level >= StatusLevel::Retry {
                    let retry_string =
                        format!("{}/{} RETRY", run_status.attempt, run_status.total_attempts);
                    write!(writer, "{:>12} ", retry_string.style(self.styles.retry))?;

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
            }
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
            } => {
                let describe = run_statuses.describe();

                if self.status_level >= describe.status_level() {
                    // First, print the status.
                    let last_status = match describe {
                        RunDescribe::Success { run_status } => {
                            write!(writer, "{:>12} ", "PASS".style(self.styles.pass))?;
                            run_status
                        }
                        RunDescribe::Flaky { last_status, .. } => {
                            // Use the skip color to also represent a flaky test.
                            write!(
                                writer,
                                "{:>12} ",
                                format!("TRY {} PASS", last_status.attempt).style(self.styles.skip)
                            )?;
                            last_status
                        }
                        RunDescribe::Failure { last_status, .. } => {
                            let status_str = match last_status.status {
                                TestStatus::Fail => "FAIL",
                                TestStatus::ExecFail => "XFAIL",
                                TestStatus::Pass => unreachable!("this is a failing test"),
                            };

                            if last_status.attempt == 1 {
                                write!(writer, "{:>12} ", status_str.style(self.styles.fail))?;
                            } else {
                                write!(
                                    writer,
                                    "{:>12} ",
                                    format!("TRY {} {}", last_status.attempt, status_str)
                                        .style(self.styles.fail)
                                )?;
                            }
                            last_status
                        }
                    };

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
            }
            TestEvent::TestSkipped {
                test_instance,
                reason: _reason,
            } => {
                if self.status_level >= StatusLevel::Skip {
                    write!(writer, "{:>12} ", "SKIP".style(self.styles.skip))?;
                    // same spacing [   0.034s]
                    write!(writer, "[         ] ")?;

                    self.write_instance(*test_instance, &mut writer)?;
                    writeln!(writer)?;
                }
            }
            TestEvent::RunBeginCancel { running, reason } => {
                write!(writer, "{:>12} ", "Canceling".style(self.styles.fail))?;
                let reason_str = match reason {
                    CancelReason::Signal => "signal",
                    // TODO: differentiate between control errors (e.g. fail-fast) and report errors
                    CancelReason::ReportError => "error",
                };

                writeln!(
                    writer,
                    "due to {}: {} tests still running",
                    reason_str.style(self.styles.count),
                    running.style(self.styles.count)
                )?;
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
                let summary_style = if *failed > 0 || *exec_failed > 0 {
                    self.styles.fail
                } else {
                    self.styles.pass
                };
                write!(writer, "{:>12} ", "Summary".style(summary_style))?;

                // Next, print the total time taken.
                // * > means right-align.
                // * 8 is the number of characters to pad to.
                // * .3 means print two digits after the decimal point.
                // TODO: better time printing mechanism than this
                write!(writer, "[{:>8.3?}s] ", elapsed.as_secs_f64())?;

                write!(writer, "{}", final_run_count.style(self.styles.count))?;
                if final_run_count != initial_run_count {
                    write!(writer, "/{}", initial_run_count.style(self.styles.count))?;
                }
                write!(
                    writer,
                    " tests run: {} passed",
                    passed.style(self.styles.pass)
                )?;

                if *flaky > 0 {
                    write!(
                        writer,
                        " ({} {})",
                        flaky.style(self.styles.count),
                        "flaky".style(self.styles.skip),
                    )?;
                }
                write!(writer, ", ")?;

                if *failed > 0 {
                    write!(
                        writer,
                        "{} {}, ",
                        failed.style(self.styles.count),
                        "failed".style(self.styles.fail),
                    )?;
                }

                if *exec_failed > 0 {
                    write!(
                        writer,
                        "{} {}, ",
                        exec_failed.style(self.styles.count),
                        "exec failed".style(self.styles.fail),
                    )?;
                }

                write!(
                    writer,
                    "{} {}",
                    skipped.style(self.styles.count),
                    "skipped".style(self.styles.skip),
                )?;

                writeln!(writer)?;

                if self.status_level >= StatusLevel::Fail {
                    for (test_instance, run_status) in &*self.failing_tests {
                        self.write_run_status(test_instance, run_status, false, &mut writer)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn write_instance(&self, instance: TestInstance<'a>, mut writer: impl Write) -> io::Result<()> {
        write!(
            writer,
            "{:>width$} ",
            instance.binary_id.style(self.styles.test_list.test_bin),
            width = self.binary_id_width
        )?;

        // Now look for the part of the test after the last ::, if any.
        let mut splits = instance.name.rsplitn(2, "::");
        let trailing = splits.next().expect("test should have at least 1 element");
        if let Some(rest) = splits.next() {
            write!(writer, "{}::", rest)?;
        }
        write!(
            writer,
            "{}",
            trailing.style(self.styles.test_list.test_name)
        )?;

        Ok(())
    }

    fn write_duration(&self, duration: Duration, mut writer: impl Write) -> io::Result<()> {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "[{:>8.3?}s] ", duration.as_secs_f64())
    }

    fn write_slow_duration(&self, duration: Duration, mut writer: impl Write) -> io::Result<()> {
        // Inside the curly braces:
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        // TODO: better time printing mechanism than this
        write!(writer, "[>{:>7.3?}s] ", duration.as_secs_f64())
    }

    fn write_run_status(
        &self,
        test_instance: &TestInstance<'a>,
        run_status: &TestRunStatus,
        is_retry: bool,
        mut writer: impl Write,
    ) -> io::Result<()> {
        let (header_style, _output_style) = if is_retry {
            (self.styles.retry, self.styles.retry_output)
        } else {
            (self.styles.fail, self.styles.fail_output)
        };

        write!(writer, "\n{}", "--- ".style(header_style))?;
        self.write_attempt(run_status, header_style, &mut writer)?;
        write!(writer, "{}", " STDOUT: ".style(header_style))?;

        {
            let no_color = strip_ansi_escapes::Writer::new(&mut writer);
            self.write_instance(*test_instance, no_color)?;
        }
        writeln!(writer, "{}", " ---".style(header_style))?;

        {
            // Strip ANSI escapes from the output in case some test framework doesn't check for
            // ttys before producing color output.
            // TODO: apply output style once https://github.com/jam1garner/owo-colors/issues/41 is
            // fixed
            let mut no_color = strip_ansi_escapes::Writer::new(&mut writer);
            no_color.write_all(run_status.stdout())?;
        }

        write!(writer, "\n{}", "--- ".style(header_style))?;
        self.write_attempt(run_status, header_style, &mut writer)?;
        write!(writer, "{}", " STDERR: ".style(header_style))?;

        {
            let no_color = strip_ansi_escapes::Writer::new(&mut writer);
            self.write_instance(*test_instance, no_color)?;
        }
        writeln!(writer, "{}", " ---".style(header_style))?;

        {
            // Strip ANSI escapes from the output in case some test framework doesn't check for
            // ttys before producing color output.
            // TODO: apply output style once https://github.com/jam1garner/owo-colors/issues/41 is
            // fixed
            let mut no_color = strip_ansi_escapes::Writer::new(&mut writer);
            no_color.write_all(run_status.stderr())?;
        }

        writeln!(writer)
    }

    fn write_attempt(
        &self,
        run_status: &TestRunStatus,
        style: Style,
        mut writer: impl Write,
    ) -> io::Result<()> {
        if run_status.total_attempts > 1 {
            write!(
                writer,
                "{} {}",
                "TRY".style(style),
                run_status.attempt.style(style)
            )?;
        }
        Ok(())
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

    /// A test was slower than a configured soft timeout.
    TestSlow {
        /// The test instance that was slow.
        test_instance: TestInstance<'a>,

        /// The amount of time that has elapsed since the beginning of the test.
        elapsed: Duration,
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

#[derive(Debug, Default)]
struct Styles {
    count: Style,
    pass: Style,
    retry: Style,
    fail: Style,
    retry_output: Style,
    fail_output: Style,
    skip: Style,
    test_list: crate::test_list::Styles,
}

impl Styles {
    fn colorize(&mut self) {
        self.count = Style::new().bold();
        self.pass = Style::new().green().bold();
        self.retry = Style::new().magenta().bold();
        self.fail = Style::new().red().bold();
        self.retry_output = Style::new().magenta();
        self.fail_output = Style::new().magenta();
        self.skip = Style::new().yellow().bold();
        self.test_list.colorize();
    }
}
