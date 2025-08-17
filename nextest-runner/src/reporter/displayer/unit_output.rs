// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Code to write out test and script outputs to the displayer.

use crate::{
    errors::DisplayErrorChain,
    reporter::{
        ByteSubslice, TestOutputErrorSlice, UnitErrorDescription,
        events::*,
        helpers::{Styles, highlight_end},
    },
    test_output::{ChildExecutionOutput, ChildOutput, ChildSingleOutput},
};
use bstr::ByteSlice;
use indent_write::io::IndentWriter;
use owo_colors::Style;
use serde::Deserialize;
use std::{
    fmt::{self, Formatter},
    io::{self, Write},
    str::FromStr,
};

/// When to display test output in the reporter.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct TestOutputDisplayStreams {
    /// When to display the stdout output of a test in the reporter
    pub stdout: Option<TestOutputDisplay>,
    /// When to display the stderr output of a test in the reporter
    pub stderr: Option<TestOutputDisplay>,
}

impl TestOutputDisplayStreams {
    /// Create a `TestOutputDisplayStreams` with both stdout and stderr set to `Immediate`
    pub fn create_immediate() -> Self {
        Self {
            stdout: Some(TestOutputDisplay::Immediate),
            stderr: Some(TestOutputDisplay::Immediate),
        }
    }
    /// Create a `TestOutputDisplayStreams` with both stdout and stderr set to `Final`
    pub fn create_final() -> Self {
        Self {
            stdout: Some(TestOutputDisplay::Final),
            stderr: Some(TestOutputDisplay::Final),
        }
    }
    /// Create a `TestOutputDisplayStreams` with both stdout and stderr set to `ImmediateFinal`
    pub fn create_immediate_final() -> Self {
        Self {
            stdout: Some(TestOutputDisplay::ImmediateFinal),
            stderr: Some(TestOutputDisplay::ImmediateFinal),
        }
    }
    /// Create a `TestOutputDisplayStreams` with both stdout and stderr set to `Never`
    pub fn create_never() -> Self {
        Self {
            stdout: Some(TestOutputDisplay::Never),
            stderr: Some(TestOutputDisplay::Never),
        }
    }

    /// Which output streams should be output immediately
    ///
    /// # Returns
    /// Returns `None` when no output streams should be output
    pub fn display_output_immediate(&self) -> Option<DisplayOutput> {
        let stdout = self.stdout.map_or(false, |t| t.is_immediate());
        let stderr = self.stderr.map_or(false, |t| t.is_immediate());
        if stdout || stderr {
            Some(DisplayOutput { stdout, stderr })
        } else {
            None
        }
    }

    /// Which output streams should be output at the end
    ///
    /// # Returns
    /// Returns `None` when no output streams should be output
    pub fn display_output_final(&self) -> Option<DisplayOutput> {
        let stdout = self.stdout.map_or(false, |t| t.is_final());
        let stderr = self.stderr.map_or(false, |t| t.is_final());
        if stdout || stderr {
            Some(DisplayOutput { stdout, stderr })
        } else {
            None
        }
    }
}

impl<'de> Deserialize<'de> for TestOutputDisplayStreams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        deserializer.deserialize_any(TestOutputDisplayStreamsVisitor)
    }
}

struct TestOutputDisplayStreamsVisitor;
impl<'de> serde::de::Visitor<'de> for TestOutputDisplayStreamsVisitor {
    type Value = TestOutputDisplayStreams;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        formatter.write_str(
            "a string with 'never', 'immediate', 'immediate-final' or 'final',\
or a map with 'stdout' and/or 'stderr' as keys and the preceding values",
        )
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // the input is a string so we expect a TestOutputDisplay value
        match v {
            "never" => Ok(TestOutputDisplayStreams::create_never()),
            "immediate" => Ok(TestOutputDisplayStreams::create_immediate()),
            "immediate-final" => Ok(TestOutputDisplayStreams::create_immediate_final()),
            "final" => Ok(TestOutputDisplayStreams::create_final()),
            _ => Err(E::invalid_value(
                serde::de::Unexpected::Str(v),
                &"unrecognized value, expected 'never', 'immediate', 'immediate-final' or 'final'\
or a map with 'stdout' and/or 'stderr' as keys and the preceding values",
            )),
        }
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        use serde::de::Error;

        // the input is a map, so we expect stdout and/or stderr as keys and TestOutputDisplay values
        // as the value
        let mut stdout = None;
        let mut stderr = None;
        while let Some((key, value)) = map.next_entry::<&str, &str>()? {
            match key {
                "stdout" => {
                    if stdout.is_some() {
                        return Err(A::Error::duplicate_field("stdout"));
                    }
                    stdout = Some(TestOutputDisplay::from_str(value).map_err(|_| {
                        A::Error::invalid_value(
                            serde::de::Unexpected::Str(value),
                            &"'never', 'immediate', 'immediate-final', or 'final'",
                        )
                    })?);
                }
                "stderr" => {
                    if stderr.is_some() {
                        return Err(A::Error::duplicate_field("stderr"));
                    }
                    stderr = Some(TestOutputDisplay::from_str(value).map_err(|_| {
                        A::Error::invalid_value(
                            serde::de::Unexpected::Str(value),
                            &"'never', 'immediate', 'immediate-final', or 'final'",
                        )
                    })?);
                }
                _ => return Err(A::Error::unknown_field(key, &["stdout", "stderr"])),
            }
        }
        Ok(TestOutputDisplayStreams { stdout, stderr })
    }
}

/// Simplified [`TestOutputDisplayStreams`] to tell which output streams should be output
///
/// Used after the Never/Immediate/Final distinction has been made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayOutput {
    /// Display the stdout output of the test
    pub stdout: bool,
    /// Display the stderr output of the test
    pub stderr: bool,
}

/// When to display test output in the reporter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
#[serde(rename_all = "kebab-case")]
pub enum TestOutputDisplay {
    /// Show output immediately on execution completion.
    ///
    /// This is the default for failing tests.
    Immediate,

    /// Show output immediately, and at the end of a test run.
    ImmediateFinal,

    /// Show output at the end of execution.
    Final,

    /// Never show output.
    Never,
}

impl TestOutputDisplay {
    /// Returns true if test output is shown immediately.
    pub fn is_immediate(self) -> bool {
        match self {
            TestOutputDisplay::Immediate | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Final | TestOutputDisplay::Never => false,
        }
    }

    /// Returns true if test output is shown at the end of the run.
    pub fn is_final(self) -> bool {
        match self {
            TestOutputDisplay::Final | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Immediate | TestOutputDisplay::Never => false,
        }
    }
}

impl FromStr for TestOutputDisplay {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "immediate" => Ok(TestOutputDisplay::Immediate),
            "immediate-final" => Ok(TestOutputDisplay::ImmediateFinal),
            "final" => Ok(TestOutputDisplay::Final),
            "never" => Ok(TestOutputDisplay::Never),
            _ => Err(
                "unrecognized value, expected 'never', 'immediate', 'immediate-final', or 'final'",
            ),
        }
    }
}

/// Formatting options for writing out child process output.
///
/// TODO: should these be lazily generated? Can't imagine this ever being
/// measurably slow.
#[derive(Debug)]
pub(super) struct ChildOutputSpec {
    pub(super) kind: UnitKind,
    pub(super) stdout_header: String,
    pub(super) stderr_header: String,
    pub(super) combined_header: String,
    pub(super) exec_fail_header: String,
    pub(super) output_indent: &'static str,
}

pub(super) struct UnitOutputReporter {
    force_success_output: TestOutputDisplayStreams,
    force_failure_output: TestOutputDisplayStreams,
    display_empty_outputs: bool,
}

impl UnitOutputReporter {
    pub(super) fn new(
        force_success_output: TestOutputDisplayStreams,
        force_failure_output: TestOutputDisplayStreams,
    ) -> Self {
        // Ordinarily, empty stdout and stderr are not displayed. This
        // environment variable is set in integration tests to ensure that they
        // are.
        let display_empty_outputs =
            std::env::var_os("__NEXTEST_DISPLAY_EMPTY_OUTPUTS").is_some_and(|v| v == "1");

        Self {
            force_success_output,
            force_failure_output,
            display_empty_outputs,
        }
    }

    pub(super) fn success_output(
        &self,
        test_setting: TestOutputDisplayStreams,
    ) -> TestOutputDisplayStreams {
        TestOutputDisplayStreams {
            stdout: self.force_success_output.stdout.or(test_setting.stdout),
            stderr: self.force_success_output.stderr.or(test_setting.stderr),
        }
    }

    pub(super) fn failure_output(
        &self,
        test_setting: TestOutputDisplayStreams,
    ) -> TestOutputDisplayStreams {
        TestOutputDisplayStreams {
            stdout: self.force_failure_output.stdout.or(test_setting.stdout),
            stderr: self.force_failure_output.stderr.or(test_setting.stderr),
        }
    }

    // These are currently only used by tests, but there's no principled
    // objection to using these functions elsewhere in the displayer.
    #[cfg(test)]
    pub(super) fn force_success_output(&self) -> TestOutputDisplayStreams {
        self.force_success_output
    }

    #[cfg(test)]
    pub(super) fn force_failure_output(&self) -> TestOutputDisplayStreams {
        self.force_failure_output
    }

    pub(super) fn write_child_execution_output(
        &self,
        styles: &Styles,
        spec: &ChildOutputSpec,
        exec_output: &ChildExecutionOutput,
        display_output: DisplayOutput,
        mut writer: &mut dyn Write,
    ) -> io::Result<()> {
        match exec_output {
            ChildExecutionOutput::Output {
                output,
                // result and errors are captured by desc.
                result: _,
                errors: _,
            } => {
                let desc = UnitErrorDescription::new(spec.kind, exec_output);

                // Show execution failures first so that they show up
                // immediately after the failure notification.
                if let Some(errors) = desc.exec_fail_error_list() {
                    writeln!(writer, "{}", spec.exec_fail_header)?;

                    // Indent the displayed error chain.
                    let error_chain = DisplayErrorChain::new(errors);
                    let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                    writeln!(indent_writer, "{error_chain}")?;
                    indent_writer.flush()?;
                    writer = indent_writer.into_inner();
                }

                let highlight_slice = if styles.is_colorized {
                    desc.output_slice()
                } else {
                    None
                };
                self.write_child_output(
                    styles,
                    spec,
                    output,
                    highlight_slice,
                    display_output,
                    writer,
                )?;
            }

            ChildExecutionOutput::StartError(error) => {
                writeln!(writer, "{}", spec.exec_fail_header)?;

                // Indent the displayed error chain.
                let error_chain = DisplayErrorChain::new(error);
                let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                writeln!(indent_writer, "{error_chain}")?;
                indent_writer.flush()?;
                writer = indent_writer.into_inner();
            }
        }

        writeln!(writer)
    }

    pub(super) fn write_child_output(
        &self,
        styles: &Styles,
        spec: &ChildOutputSpec,
        output: &ChildOutput,
        highlight_slice: Option<TestOutputErrorSlice<'_>>,
        display_output: DisplayOutput,
        mut writer: &mut dyn Write,
    ) -> io::Result<()> {
        match output {
            ChildOutput::Split(split) => {
                if let Some(stdout) = &split.stdout {
                    if self.display_empty_outputs || (!stdout.is_empty() && display_output.stdout) {
                        writeln!(writer, "{}", spec.stdout_header)?;

                        // If there's no output indent, this is a no-op, though
                        // it will bear the perf cost of a vtable indirection +
                        // whatever internal state IndentWriter tracks. Doubt
                        // this will be an issue in practice though!
                        let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                        self.write_test_single_output_with_description(
                            styles,
                            stdout,
                            highlight_slice.and_then(|d| d.stdout_subslice()),
                            &mut indent_writer,
                        )?;
                        indent_writer.flush()?;
                        writer = indent_writer.into_inner();
                    }
                }

                if let Some(stderr) = &split.stderr {
                    if self.display_empty_outputs || (!stderr.is_empty() && display_output.stderr) {
                        writeln!(writer, "{}", spec.stderr_header)?;

                        let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                        self.write_test_single_output_with_description(
                            styles,
                            stderr,
                            highlight_slice.and_then(|d| d.stderr_subslice()),
                            &mut indent_writer,
                        )?;
                        indent_writer.flush()?;
                    }
                }
            }
            ChildOutput::Combined { output } => {
                if self.display_empty_outputs || (!output.is_empty() && display_output.stdout) {
                    writeln!(writer, "{}", spec.combined_header)?;

                    let mut indent_writer = IndentWriter::new(spec.output_indent, writer);
                    self.write_test_single_output_with_description(
                        styles,
                        output,
                        highlight_slice.and_then(|d| d.combined_subslice()),
                        &mut indent_writer,
                    )?;
                    indent_writer.flush()?;
                }
            }
        }

        Ok(())
    }

    /// Writes a test output to the writer, along with optionally a subslice of the output to
    /// highlight.
    ///
    /// The description must be a subslice of the output.
    fn write_test_single_output_with_description(
        &self,
        styles: &Styles,
        output: &ChildSingleOutput,
        description: Option<ByteSubslice<'_>>,
        writer: &mut dyn Write,
    ) -> io::Result<()> {
        if styles.is_colorized {
            if let Some(subslice) = description {
                write_output_with_highlight(&output.buf, subslice, &styles.fail, writer)?;
            } else {
                // Output the text without stripping ANSI escapes, then reset the color afterwards
                // in case the output is malformed.
                write_output_with_trailing_newline(&output.buf, RESET_COLOR, writer)?;
            }
        } else {
            // Strip ANSI escapes from the output if nextest itself isn't colorized.
            let mut no_color = strip_ansi_escapes::Writer::new(writer);
            write_output_with_trailing_newline(&output.buf, b"", &mut no_color)?;
        }

        Ok(())
    }
}

const RESET_COLOR: &[u8] = b"\x1b[0m";

fn write_output_with_highlight(
    output: &[u8],
    ByteSubslice { slice, start }: ByteSubslice,
    highlight_style: &Style,
    mut writer: &mut dyn Write,
) -> io::Result<()> {
    let end = start + highlight_end(slice);

    // Output the start and end of the test without stripping ANSI escapes, then reset
    // the color afterwards in case the output is malformed.
    writer.write_all(&output[..start])?;
    writer.write_all(RESET_COLOR)?;

    // Some systems (e.g. GitHub Actions, Buildomat) don't handle multiline ANSI
    // coloring -- they reset colors after each line. To work around that,
    // we reset and re-apply colors for each line.
    for line in output[start..end].lines_with_terminator() {
        write!(writer, "{}", FmtPrefix(highlight_style))?;

        // Write everything before the newline, stripping ANSI escapes.
        let mut no_color = strip_ansi_escapes::Writer::new(writer);
        let trimmed = line.trim_end_with(|c| c == '\n' || c == '\r');
        no_color.write_all(trimmed.as_bytes())?;
        writer = no_color.into_inner()?;

        // End coloring.
        write!(writer, "{}", FmtSuffix(highlight_style))?;

        // Now write the newline, if present.
        writer.write_all(&line[trimmed.len()..])?;
    }

    // `end` is guaranteed to be within the bounds of `output.buf`. (It is actually safe
    // for it to be equal to `output.buf.len()` -- it gets treated as an empty list in
    // that case.)
    write_output_with_trailing_newline(&output[end..], RESET_COLOR, writer)?;

    Ok(())
}

/// Write output, always ensuring there's a trailing newline. (If there's no
/// newline, one will be inserted.)
///
/// `trailer` is written immediately before the trailing newline if any.
fn write_output_with_trailing_newline(
    mut output: &[u8],
    trailer: &[u8],
    writer: &mut dyn Write,
) -> io::Result<()> {
    // If there's a trailing newline in the output, insert the trailer right
    // before it.
    if output.last() == Some(&b'\n') {
        output = &output[..output.len() - 1];
    }

    writer.write_all(output)?;
    writer.write_all(trailer)?;
    writer.write_all(b"\n")
}

struct FmtPrefix<'a>(&'a Style);

impl fmt::Display for FmtPrefix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt_prefix(f)
    }
}

struct FmtSuffix<'a>(&'a Style);

impl fmt::Display for FmtSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt_suffix(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_output_with_highlight() {
        const RESET_COLOR: &str = "\u{1b}[0m";
        const BOLD_RED: &str = "\u{1b}[31;1m";

        assert_eq!(
            write_output_with_highlight_buf("output", 0, Some(6)),
            format!("{RESET_COLOR}{BOLD_RED}output{RESET_COLOR}{RESET_COLOR}\n")
        );

        assert_eq!(
            write_output_with_highlight_buf("output", 1, Some(5)),
            format!("o{RESET_COLOR}{BOLD_RED}utpu{RESET_COLOR}t{RESET_COLOR}\n")
        );

        assert_eq!(
            write_output_with_highlight_buf("output\nhighlight 1\nhighlight 2\n", 7, None),
            format!(
                "output\n{RESET_COLOR}\
                {BOLD_RED}highlight 1{RESET_COLOR}\n\
                {BOLD_RED}highlight 2{RESET_COLOR}{RESET_COLOR}\n"
            )
        );

        assert_eq!(
            write_output_with_highlight_buf(
                "output\nhighlight 1\nhighlight 2\nnot highlighted",
                7,
                None
            ),
            format!(
                "output\n{RESET_COLOR}\
                {BOLD_RED}highlight 1{RESET_COLOR}\n\
                {BOLD_RED}highlight 2{RESET_COLOR}\n\
                not highlighted{RESET_COLOR}\n"
            )
        );
    }

    fn write_output_with_highlight_buf(output: &str, start: usize, end: Option<usize>) -> String {
        // We're not really testing non-UTF-8 output here, and using strings results in much more
        // readable error messages.
        let mut buf = Vec::new();
        let end = end.unwrap_or(output.len());

        let subslice = ByteSubslice {
            start,
            slice: &output.as_bytes()[start..end],
        };
        write_output_with_highlight(
            output.as_bytes(),
            subslice,
            &Style::new().red().bold(),
            &mut buf,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }
}
