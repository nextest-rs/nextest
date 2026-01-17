// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Code to write out test and script outputs to the displayer.

use crate::{
    errors::DisplayErrorChain,
    indenter::indented,
    reporter::{
        ByteSubslice, TestOutputErrorSlice, UnitErrorDescription,
        events::*,
        helpers::{Styles, highlight_end},
    },
    test_output::ChildSingleOutput,
    write_str::WriteStr,
};
use owo_colors::Style;
use serde::Deserialize;
use std::{fmt, io};

/// When to display test output in the reporter.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, serde::Serialize)]
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
    force_success_output: Option<TestOutputDisplay>,
    force_failure_output: Option<TestOutputDisplay>,
    force_exec_fail_output: Option<TestOutputDisplay>,
    display_empty_outputs: bool,
}

impl UnitOutputReporter {
    pub(super) fn new(
        force_success_output: Option<TestOutputDisplay>,
        force_failure_output: Option<TestOutputDisplay>,
        force_exec_fail_output: Option<TestOutputDisplay>,
    ) -> Self {
        // Ordinarily, empty stdout and stderr are not displayed. This
        // environment variable is set in integration tests to ensure that they
        // are.
        let display_empty_outputs =
            std::env::var_os("__NEXTEST_DISPLAY_EMPTY_OUTPUTS").is_some_and(|v| v == "1");

        Self {
            force_success_output,
            force_failure_output,
            force_exec_fail_output,
            display_empty_outputs,
        }
    }

    pub(super) fn success_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_success_output.unwrap_or(test_setting)
    }

    pub(super) fn failure_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_failure_output.unwrap_or(test_setting)
    }

    pub(super) fn exec_fail_output(&self, test_setting: TestOutputDisplay) -> TestOutputDisplay {
        self.force_exec_fail_output.unwrap_or(test_setting)
    }

    // These are currently only used by tests, but there's no principled
    // objection to using these functions elsewhere in the displayer.
    #[cfg(test)]
    pub(super) fn force_success_output(&self) -> Option<TestOutputDisplay> {
        self.force_success_output
    }

    #[cfg(test)]
    pub(super) fn force_failure_output(&self) -> Option<TestOutputDisplay> {
        self.force_failure_output
    }

    pub(super) fn write_child_execution_output(
        &self,
        styles: &Styles,
        spec: &ChildOutputSpec,
        exec_output: &ChildExecutionOutputDescription<ChildSingleOutput>,
        mut writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        match exec_output {
            ChildExecutionOutputDescription::Output {
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
                    let mut indent_writer = indented(writer).with_str(spec.output_indent);
                    writeln!(indent_writer, "{error_chain}")?;
                    indent_writer.write_str_flush()?;
                    writer = indent_writer.into_inner();
                }

                let highlight_slice = if styles.is_colorized {
                    desc.output_slice()
                } else {
                    None
                };
                self.write_child_output(styles, spec, output, highlight_slice, writer)?;
            }

            ChildExecutionOutputDescription::StartError(error) => {
                writeln!(writer, "{}", spec.exec_fail_header)?;

                // Indent the displayed error chain.
                let error_chain = DisplayErrorChain::new(error);
                let mut indent_writer = indented(writer).with_str(spec.output_indent);
                writeln!(indent_writer, "{error_chain}")?;
                indent_writer.write_str_flush()?;
                writer = indent_writer.into_inner();
            }
        }

        writeln!(writer)
    }

    pub(super) fn write_child_output(
        &self,
        styles: &Styles,
        spec: &ChildOutputSpec,
        output: &ChildOutputDescription<ChildSingleOutput>,
        highlight_slice: Option<TestOutputErrorSlice<'_>>,
        mut writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        match output {
            ChildOutputDescription::Split { stdout, stderr } => {
                if let Some(stdout) = stdout
                    && (self.display_empty_outputs || !stdout.is_empty())
                {
                    writeln!(writer, "{}", spec.stdout_header)?;

                    // If there's no output indent, this is a no-op, though
                    // it will bear the perf cost of a vtable indirection +
                    // whatever internal state IndentWriter tracks. Doubt
                    // this will be an issue in practice though!
                    let mut indent_writer = indented(writer).with_str(spec.output_indent);
                    self.write_test_single_output_with_description(
                        styles,
                        stdout,
                        highlight_slice.and_then(|d| d.stdout_subslice()),
                        &mut indent_writer,
                    )?;
                    indent_writer.write_str_flush()?;
                    writer = indent_writer.into_inner();
                }

                if let Some(stderr) = stderr
                    && (self.display_empty_outputs || !stderr.is_empty())
                {
                    writeln!(writer, "{}", spec.stderr_header)?;

                    let mut indent_writer = indented(writer).with_str(spec.output_indent);
                    self.write_test_single_output_with_description(
                        styles,
                        stderr,
                        highlight_slice.and_then(|d| d.stderr_subslice()),
                        &mut indent_writer,
                    )?;
                    indent_writer.write_str_flush()?;
                }
            }
            ChildOutputDescription::Combined { output } => {
                if self.display_empty_outputs || !output.is_empty() {
                    writeln!(writer, "{}", spec.combined_header)?;

                    let mut indent_writer = indented(writer).with_str(spec.output_indent);
                    self.write_test_single_output_with_description(
                        styles,
                        output,
                        highlight_slice.and_then(|d| d.combined_subslice()),
                        &mut indent_writer,
                    )?;
                    indent_writer.write_str_flush()?;
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
        writer: &mut dyn WriteStr,
    ) -> io::Result<()> {
        let output_str = output.as_str_lossy();
        if styles.is_colorized {
            if let Some(subslice) = description {
                write_output_with_highlight(output_str, subslice, &styles.fail, writer)?;
            } else {
                // Output the text without stripping ANSI escapes, then reset the color afterwards
                // in case the output is malformed.
                write_output_with_trailing_newline(output_str, RESET_COLOR, writer)?;
            }
        } else {
            // Strip ANSI escapes from the output if nextest itself isn't colorized.
            let output_no_color = strip_ansi_escapes::strip_str(output_str);
            write_output_with_trailing_newline(&output_no_color, "", writer)?;
        }

        Ok(())
    }
}

const RESET_COLOR: &str = "\x1b[0m";

fn write_output_with_highlight(
    output: &str,
    ByteSubslice { slice, start }: ByteSubslice,
    highlight_style: &Style,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    let end = start + highlight_end(slice);

    // Output the start and end of the test without stripping ANSI escapes, then reset
    // the color afterwards in case the output is malformed.
    writer.write_str(&output[..start])?;
    writer.write_str(RESET_COLOR)?;

    // Some systems (e.g. GitHub Actions, Buildomat) don't handle multiline ANSI
    // coloring -- they reset colors after each line. To work around that,
    // we reset and re-apply colors for each line.
    for line in output[start..end].split_inclusive('\n') {
        write!(writer, "{}", FmtPrefix(highlight_style))?;

        // Write everything before the newline, stripping ANSI escapes.
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let stripped = strip_ansi_escapes::strip_str(trimmed);
        writer.write_str(&stripped)?;

        // End coloring.
        write!(writer, "{}", FmtSuffix(highlight_style))?;

        // Now write the newline, if present.
        writer.write_str(&line[trimmed.len()..])?;
    }

    // `end` is guaranteed to be within the bounds of `output`. (It is actually safe
    // for it to be equal to `output.len()` -- it gets treated as an empty string in
    // that case.)
    write_output_with_trailing_newline(&output[end..], RESET_COLOR, writer)?;

    Ok(())
}

/// Write output, always ensuring there's a trailing newline. (If there's no
/// newline, one will be inserted.)
///
/// `trailer` is written immediately before the trailing newline if any.
fn write_output_with_trailing_newline(
    mut output: &str,
    trailer: &str,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    // If there's a trailing newline in the output, insert the trailer right
    // before it.
    if output.ends_with('\n') {
        output = &output[..output.len() - 1];
    }

    writer.write_str(output)?;
    writer.write_str(trailer)?;
    writeln!(writer)
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
        let mut buf = String::new();
        let end = end.unwrap_or(output.len());

        let subslice = ByteSubslice {
            start,
            slice: &output.as_bytes()[start..end],
        };
        write_output_with_highlight(output, subslice, &Style::new().red().bold(), &mut buf)
            .unwrap();
        buf
    }
}
