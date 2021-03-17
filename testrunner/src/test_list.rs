// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputFormat, test_filter::TestFilter};
use anyhow::{anyhow, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use duct::cmd;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};
use termcolor::{ColorSpec, NoColor, WriteColor};

// TODO: capture ignored and not-ignored tests

/// Represents a test binary.
///
/// Accepted as input to `TestList::new`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinary {
    /// The test binary.
    pub binary: Utf8PathBuf,

    /// A friendly name for this binary. If provided, this name will be used instead of the binary.
    pub friendly_name: Option<String>,

    /// The working directory that this test should be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Option<Utf8PathBuf>,
}

/// List of tests, gotten by executing a test binary with the `--list` command.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestList {
    /// Number of tests across all binaries.
    test_count: usize,
    tests: BTreeMap<Utf8PathBuf, TestBinInfo>,
    // TODO: handle ignored tests
}

/// Information about a test binary.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinInfo {
    /// A friendly name for this binary. If provided, this name will be used instead of the binary.
    pub friendly_name: Option<String>,

    /// Test names.
    pub test_names: Vec<String>,

    /// The working directory that this test binary will be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Option<Utf8PathBuf>,
}

impl TestList {
    /// Creates a new test list by running the given command and applying the specified filter.
    pub fn new(
        test_binaries: impl IntoIterator<Item = TestBinary>,
        filter: &TestFilter,
    ) -> Result<Self> {
        let mut test_count = 0;

        let tests = test_binaries
            .into_iter()
            .map(|test_binary| {
                let mut cmd = cmd!(
                    AsRef::<Path>::as_ref(&test_binary.binary),
                    "--list",
                    "--format",
                    "terse"
                )
                .stdout_capture();
                if let Some(cwd) = &test_binary.cwd {
                    cmd = cmd.dir(cwd);
                };

                let output = cmd.read().with_context(|| {
                    format!(
                        "running '{} --list --format --terse' failed",
                        test_binary.binary
                    )
                })?;

                let (bin, info) = Self::process_output(test_binary, filter, output.as_str())?;
                test_count += info.test_names.len();
                Ok((bin, info))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self { tests, test_count })
    }

    /// Creates a new test list with the given binary names and outputs.
    pub fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<Item = (TestBinary, impl AsRef<str>)>,
        filter: &TestFilter,
    ) -> Result<Self> {
        let mut test_count = 0;

        let tests = test_bin_outputs
            .into_iter()
            .map(|(test_binary, output)| {
                let (bin, info) = Self::process_output(test_binary, filter, output.as_ref())?;
                test_count += info.test_names.len();
                Ok((bin, info))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self { tests, test_count })
    }

    /// Returns the total number of tests across all binaries.
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Returns the total number of binaries that contain tests.
    pub fn binary_count(&self) -> usize {
        self.tests.len()
    }

    /// Returns the tests for a given binary, or `None` if the binary wasn't in the list.
    pub fn get(&self, test_bin: impl AsRef<Utf8Path>) -> Option<&TestBinInfo> {
        self.tests.get(test_bin.as_ref())
    }

    /// Outputs this list to the given writer.
    pub fn write(&self, output_format: OutputFormat, writer: impl WriteColor) -> Result<()> {
        match output_format {
            OutputFormat::Plain => self.write_plain(writer).context("error writing test list"),
            OutputFormat::Serializable(format) => format.to_writer(self, writer),
        }
    }

    /// Iterates over the list of tests, returning the path and test name.
    pub fn iter(&self) -> impl Iterator<Item = TestInstance<'_>> + '_ {
        self.tests.iter().flat_map(|(test_bin, info)| {
            info.test_names.iter().map(move |test_name| {
                TestInstance::new(
                    test_bin,
                    info.friendly_name.as_deref(),
                    test_name,
                    info.cwd.as_deref(),
                )
            })
        })
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String> {
        // Ugh this sucks. String really should have an io::Write impl that errors on non-UTF8 text.
        let mut buf = NoColor::new(vec![]);
        self.write(output_format, &mut buf)?;
        Ok(String::from_utf8(buf.into_inner()).expect("buffer is valid UTF-8"))
    }

    // ---
    // Helper methods
    // ---

    fn process_output(
        test_binary: TestBinary,
        filter: &TestFilter,
        output: impl AsRef<str>,
    ) -> Result<(Utf8PathBuf, TestBinInfo)> {
        let TestBinary {
            binary,
            cwd,
            friendly_name,
        } = test_binary;

        let output = output.as_ref();
        let test_names = Self::parse(output, filter)?;

        Ok((
            binary,
            TestBinInfo {
                test_names,
                cwd,
                friendly_name,
            },
        ))
    }

    /// Parses the output of --list --format terse.
    fn parse(list_output: impl AsRef<str>, filter: &TestFilter) -> Result<Vec<String>> {
        Self::parse_impl(list_output.as_ref(), filter)
    }

    fn parse_impl(list_output: &str, filter: &TestFilter) -> Result<Vec<String>> {
        // The output is in the form:
        // <test name>: test
        // <test name>: test
        // ...

        let mut tests = vec![];
        for line in list_output.lines() {
            let test_name = line.strip_suffix(": test").ok_or_else(|| {
                anyhow!(
                    "line '{}' did not end with the string ': test', full output:\n{}",
                    line,
                    list_output
                )
            })?;
            if filter.is_match(test_name) {
                tests.push(test_name.into());
            }
        }
        Ok(tests)
    }

    fn write_plain(&self, mut writer: impl WriteColor) -> io::Result<()> {
        let test_bin_spec = test_bin_spec();
        let field_spec = Self::field_spec();
        let test_name_spec = test_name_spec();

        for (test_bin, info) in &self.tests {
            writer.set_color(&test_bin_spec)?;
            write!(writer, "{}", test_bin)?;
            writer.reset()?;
            writeln!(writer, ":")?;

            if let Some(cwd) = &info.cwd {
                writer.set_color(&field_spec)?;
                write!(writer, "  cwd: ")?;
                writer.reset()?;
                writeln!(writer, "{}", cwd)?;
            }

            writer.set_color(&test_name_spec)?;
            for test_name in &info.test_names {
                writeln!(writer, "    {}", test_name)?;
            }
            writer.reset()?;
        }
        Ok(())
    }

    fn field_spec() -> ColorSpec {
        let mut color_spec = ColorSpec::new();
        color_spec
            .set_fg(Some(termcolor::Color::Yellow))
            .set_bold(true);
        color_spec
    }
}

/// Represents a single test with its associated binary.
#[derive(Clone, Copy, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct TestInstance<'a> {
    /// The test binary.
    pub binary: &'a Utf8Path,

    /// The friendly name of the binary, if any.
    pub friendly_name: Option<&'a str>,

    /// The name of the test.
    pub test_name: &'a str,

    /// The working directory for this test. If None, the test will not be changed.
    pub cwd: Option<&'a Utf8Path>,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub(crate) fn new(
        binary: &'a (impl AsRef<Utf8Path> + ?Sized),
        friendly_name: Option<&'a str>,
        test_name: &'a (impl AsRef<str> + ?Sized),
        cwd: Option<&'a Utf8Path>,
    ) -> Self {
        Self {
            binary: binary.as_ref(),
            friendly_name,
            test_name: test_name.as_ref(),
            cwd,
        }
    }

    /// Formats this `TestInstance` and writes it to the given `WriteColor`.
    pub fn write(&self, mut writer: impl WriteColor) -> io::Result<()> {
        let friendly_name = self.friendly_name.unwrap_or_else(|| {
            self.binary
                .file_name()
                .expect("test binaries always have file names")
        });

        writer.set_color(&test_bin_spec())?;
        // TODO: don't hardcode the maximum width (probably need to look at all the friendly names
        // across all instances)
        write!(writer, "{:>20}", friendly_name)?;
        writer.reset()?;
        write!(writer, "  ")?;

        // Now look for the part of the test after the last ::, if any.
        let mut splits = self.test_name.rsplitn(2, "::");
        let trailing = splits.next().expect("test should have at least 1 element");
        if let Some(rest) = splits.next() {
            write!(writer, "{}::", rest)?;
        }
        writer.set_color(&test_name_spec())?;
        write!(writer, "{}", trailing)?;
        writer.reset()?;

        Ok(())
    }
}

fn test_bin_spec() -> ColorSpec {
    let mut color_spec = ColorSpec::new();
    color_spec
        .set_fg(Some(termcolor::Color::Magenta))
        .set_bold(true);
    color_spec
}

fn test_name_spec() -> ColorSpec {
    let mut color_spec = ColorSpec::new();
    color_spec
        .set_fg(Some(termcolor::Color::Blue))
        .set_bold(true);
    color_spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::SerializableFormat;
    use indoc::indoc;
    use maplit::btreemap;
    use pretty_assertions::assert_eq;
    use std::iter;

    #[test]
    fn test_parse() {
        let list_output = indoc! {"
            tests::foo::test_bar: test
            tests::baz::test_quux: test
        "};

        let test_filter = TestFilter::any();
        let fake_cwd: Utf8PathBuf = "/fake/cwd".into();
        let fake_friendly_name = "fake-package".to_owned();
        let test_binary = TestBinary {
            binary: "/fake/binary".into(),
            cwd: Some(fake_cwd.clone()),
            friendly_name: Some(fake_friendly_name.clone()),
        };
        let tests =
            TestList::new_with_outputs(iter::once((test_binary, &list_output)), &test_filter)
                .expect("valid output");
        assert_eq!(
            tests.tests,
            btreemap! {
                "/fake/binary".into() => TestBinInfo {
                    test_names: vec![
                        "tests::foo::test_bar".to_owned(),
                        "tests::baz::test_quux".to_owned(),
                    ],
                    cwd: Some(fake_cwd),
                    friendly_name: Some(fake_friendly_name),
                }
            }
        );

        // Check that the expected outputs are valid.
        static EXPECTED_PLAIN: &str = indoc! {"
            /fake/binary:
              cwd: /fake/cwd
                tests::foo::test_bar
                tests::baz::test_quux
        "};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "test-count": 2,
              "tests": {
                "/fake/binary": {
                  "friendly-name": "fake-package",
                  "test-names": [
                    "tests::foo::test_bar",
                    "tests::baz::test_quux"
                  ],
                  "cwd": "/fake/cwd"
                }
              }
            }"#};
        static EXPECTED_TOML_PRETTY: &str = indoc! {r#"
            test-count = 2
            [tests."/fake/binary"]
            friendly-name = 'fake-package'
            test-names = [
                'tests::foo::test_bar',
                'tests::baz::test_quux',
            ]
            cwd = '/fake/cwd'
        "#};

        assert_eq!(
            tests
                .to_string(OutputFormat::Plain)
                .expect("plain succeeded"),
            EXPECTED_PLAIN
        );
        assert_eq!(
            tests
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded"),
            EXPECTED_JSON_PRETTY
        );
        assert_eq!(
            tests
                .to_string(OutputFormat::Serializable(SerializableFormat::TomlPretty))
                .expect("toml-pretty succeeded"),
            EXPECTED_TOML_PRETTY
        );
    }
}
