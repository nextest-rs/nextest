// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat,
    runner::{RunStats, TestRunStatus, TestStatus},
    test_filter::MismatchReason,
    test_list::{test_bin_spec, test_name_spec, TestInstance, TestList},
};
use anyhow::{bail, Context, Result};
use camino::Utf8PathBuf;
use quick_junit::{Report, Testcase, TestcaseStatus, Testsuite};
use std::{
    collections::HashMap,
    fmt,
    fs::File,
    io,
    io::Write,
    str::FromStr,
    time::{Duration, Instant},
};
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

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FailureOutput {
    Immediate,
    ImmediateFinal,
    Final,
    Never,
}

impl FailureOutput {
    pub fn variants() -> [&'static str; 4] {
        ["immediate", "immediate-final", "final", "never"]
    }

    fn is_immediate(self) -> bool {
        match self {
            FailureOutput::Immediate | FailureOutput::ImmediateFinal => true,
            FailureOutput::Final | FailureOutput::Never => false,
        }
    }

    fn is_final(self) -> bool {
        match self {
            FailureOutput::Final | FailureOutput::ImmediateFinal => true,
            FailureOutput::Immediate | FailureOutput::Never => false,
        }
    }
}

impl fmt::Display for FailureOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureOutput::Immediate => write!(f, "immediate"),
            FailureOutput::ImmediateFinal => write!(f, "immediate-final"),
            FailureOutput::Final => write!(f, "final"),
            FailureOutput::Never => write!(f, "never"),
        }
    }
}

impl FromStr for FailureOutput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "immediate" => FailureOutput::Immediate,
            "immediate-final" => FailureOutput::ImmediateFinal,
            "final" => FailureOutput::Final,
            "never" => FailureOutput::Never,
            other => bail!("unrecognized value for failure-output: {}", other),
        };
        Ok(val)
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
    /// Output JUnit/XUnit to a file
    #[structopt(long)]
    junit: Option<Utf8PathBuf>,
}

/// Functionality to report test results to stdout and JUnit
pub struct TestReporter<'list> {
    stdout: BufferWriter,
    #[allow(dead_code)]
    stderr: BufferWriter,
    failure_output: FailureOutput,
    binary_id_width: usize,

    // TODO: too many concerns mixed up here. Should have a better model, probably in conjunction
    // with factoring out the different reporters below.
    failing_tests: Vec<(TestInstance<'list>, TestRunStatus)>,

    // TODO: Improve this design. A better design would be:
    // * add a ReportStore struct which receives test events and stores them
    // * provide a list of reporters to that struct, e.g. StdoutReporter and JUnitReporter
    // * that struct owns all the results (e.g. stdout and stderr) and calls the reporters
    // * TestEvent gains a lifetime param? need to figure this out in more detail
    junit_reporter: Option<JUnitReporter<'list>>,
}

impl<'list> TestReporter<'list> {
    /// Creates a new instance with the given color choice.
    pub fn new(test_list: &TestList, color: Color, opts: ReporterOpts) -> Self {
        let stdout = BufferWriter::stdout(color.color_choice(atty::Stream::Stdout));
        let stderr = BufferWriter::stderr(color.color_choice(atty::Stream::Stderr));
        let binary_id_width = test_list
            .iter()
            .map(|(_, info)| info.binary_id.len())
            .max()
            .unwrap_or_default();
        let junit_reporter = opts.junit.map(JUnitReporter::new);
        Self {
            stdout,
            stderr,
            failure_output: opts.failure_output,
            failing_tests: vec![],
            binary_id_width,
            junit_reporter,
        }
    }

    /// Write a list of tests in the given format.
    pub fn write_list(&self, test_list: &TestList, output_format: OutputFormat) -> Result<()> {
        let mut buffer = self.stdout.buffer();
        test_list.write(output_format, &mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }

    /// Report a test event.
    pub fn report_event(&mut self, event: TestEvent<'list>) -> Result<()> {
        let mut buffer = self.stdout.buffer();
        self.write_event(event, &mut buffer)?;
        self.stdout.print(&buffer).context("error writing output")
    }

    // ---
    // Helper methods
    // ---

    /// Report this test event to the given writer.
    fn write_event(&mut self, event: TestEvent<'list>, mut writer: impl WriteColor) -> Result<()> {
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
                self.write_instance(*test_instance, &mut writer)?;
                writeln!(writer)?;

                // If the test failed to execute, print its output and error status.
                if !run_status.status.is_success() && self.failure_output.is_immediate() {
                    self.write_run_status(test_instance, run_status, &mut writer)?;
                }

                if !run_status.status.is_success() && self.failure_output.is_final() {
                    // TODO this clone can probably be avoided with a refactoring
                    self.failing_tests
                        .push((*test_instance, run_status.clone()));
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
                    self.write_run_status(test_instance, run_status, &mut writer)?;
                }
            }
        }

        if let Some(junit_reporter) = &mut self.junit_reporter {
            junit_reporter.write_event(event)?;
        }
        Ok(())
    }

    fn write_instance(
        &self,
        instance: TestInstance<'list>,
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

    fn write_run_status(
        &self,
        test_instance: &TestInstance<'list>,
        run_status: &TestRunStatus,
        mut writer: impl WriteColor,
    ) -> io::Result<()> {
        writer.set_color(&Self::fail_spec())?;
        write!(writer, "\n--- STDOUT: ")?;
        self.write_instance(*test_instance, NoColor::new(&mut writer))?;
        writeln!(writer, " ---")?;

        writer.set_color(&Self::fail_output_spec())?;
        NoColor::new(&mut writer).write_all(&run_status.stdout)?;

        writer.set_color(&Self::fail_spec())?;
        write!(writer, "--- STDERR: ")?;
        self.write_instance(*test_instance, NoColor::new(&mut writer))?;
        writeln!(writer, " ---")?;

        writer.set_color(&Self::fail_output_spec())?;
        NoColor::new(&mut writer).write_all(&run_status.stderr)?;

        writer.reset()?;
        writeln!(writer)
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

impl<'list> fmt::Debug for TestReporter<'list> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TestReporter")
            .field("stdout", &"BufferWriter { .. }")
            .field("stderr", &"BufferWriter { .. }")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum TestEvent<'list> {
    /// The test run started.
    RunStarted {
        /// The list of tests that will be run.
        ///
        /// The methods on the test list indicate the number of
        test_list: &'list TestList,
    },

    // TODO: add events for BinaryStarted and BinaryFinished? May want a slightly different way to
    // do things, maybe a couple of reporter traits (one for the run as a whole and one for each
    // binary).
    /// A test started running.
    TestStarted {
        /// The test instance that was started.
        test_instance: TestInstance<'list>,
    },

    /// A test finished running.
    TestFinished {
        /// The test instance that finished running.
        test_instance: TestInstance<'list>,

        /// Information about how this test was run.
        run_status: TestRunStatus,
    },

    /// A test was skipped.
    TestSkipped {
        /// The test instance that was skipped.
        test_instance: TestInstance<'list>,

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
        start_time: Instant,

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

#[derive(Clone, Debug)]
struct JUnitReporter<'list> {
    path: Utf8PathBuf,
    testsuites: HashMap<&'list str, Testsuite>,
}

impl<'list> JUnitReporter<'list> {
    fn new(path: Utf8PathBuf) -> Self {
        Self {
            path,
            testsuites: HashMap::new(),
        }
    }

    fn write_event(&mut self, event: TestEvent<'list>) -> Result<()> {
        match event {
            TestEvent::RunStarted { .. } => {}
            TestEvent::TestStarted { .. } => {}
            TestEvent::TestFinished {
                test_instance,
                run_status,
            } => {
                let testsuite = self.testsuite_for(test_instance);

                let testcase_status = match run_status.status {
                    TestStatus::Pass => TestcaseStatus::success(),
                    TestStatus::Fail => {
                        let mut status = TestcaseStatus::failure();
                        status.set_type("test failure");
                        status
                    }
                    TestStatus::ExecFail => {
                        let mut status = TestcaseStatus::error();
                        status.set_type("execution failure");
                        status
                    }
                };
                // TODO: set message/description on testcase_status?

                let mut testcase = Testcase::new(test_instance.name, testcase_status);
                testcase.set_classname(test_instance.binary_id);
                testcase.set_time(run_status.time_taken);

                // TODO: also provide stdout and stderr for passing tests?
                // TODO: allure seems to want the output to be in a format where text files are
                // written out to disk:
                // https://github.com/allure-framework/allure2/blob/master/plugins/junit-xml-plugin/src/main/java/io/qameta/allure/junitxml/JunitXmlPlugin.java#L192-L196
                // we may have to update this format to handle that.
                if !run_status.status.is_success() {
                    testcase
                        .set_system_out_lossy(run_status.stdout)
                        .set_system_err_lossy(run_status.stderr);
                }

                testsuite.add_testcase(testcase);
            }
            TestEvent::TestSkipped { .. } => {
                // TODO: report skipped tests? causes issues if we want to aggregate runs across
                // skipped and non-skipped tests. Probably needs to be made configurable.

                // let testsuite = self.testsuite_for(test_instance);
                //
                // let mut testcase_status = TestcaseStatus::skipped();
                // testcase_status.set_message(format!("Skipped: {}", reason));
                // let testcase = Testcase::new(test_instance.name, testcase_status);
                //
                // testsuite.add_testcase(testcase);
            }
            TestEvent::RunBeginCancel { .. } => {}
            TestEvent::RunFinished { elapsed, .. } => {
                // Write out the report to the given file.
                // TODO: customize name
                // TODO: write a separate report for each binary?
                let mut report = Report::new("nextest-run");
                report
                    .set_time(elapsed)
                    .add_testsuites(self.testsuites.drain().map(|(_, testsuite)| testsuite));

                let f = File::create(&self.path).with_context(|| {
                    format!("failed to open junit file '{}' for writing", self.path)
                })?;
                report
                    .serialize(f)
                    .with_context(|| format!("failed to serialize junit to {}", self.path))?;
            }
        }

        Ok(())
    }

    fn testsuite_for(&mut self, test_instance: TestInstance<'list>) -> &mut Testsuite {
        self.testsuites
            .entry(test_instance.binary_id)
            .or_insert_with(|| Testsuite::new(test_instance.binary_id))
    }
}
