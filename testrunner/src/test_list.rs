// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    output::OutputFormat,
    test_filter::{FilterMatch, TestFilter},
};
use anyhow::{anyhow, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use duct::cmd;
use once_cell::sync::OnceCell;
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
    /// Number of tests (including skipped and ignored) across all binaries.
    test_count: usize,
    test_binaries: BTreeMap<Utf8PathBuf, TestBinInfo>,
    // Values computed on first access.
    #[serde(skip)]
    skip_count: OnceCell<usize>,
}

/// Information about a test binary.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinInfo {
    /// A friendly name for this binary. If provided, this name will be used instead of the binary.
    pub friendly_name: Option<String>,

    /// Test names and other information.
    pub tests: BTreeMap<String, RustTestInfo>,

    /// The working directory that this test binary will be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Option<Utf8PathBuf>,
}

/// Information about a single Rust test.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustTestInfo {
    /// Whether the test matches the provided test filter.
    ///
    /// Only tests that match the filter are run.
    pub filter_match: FilterMatch,
}

impl TestList {
    /// Creates a new test list by running the given command and applying the specified filter.
    pub fn new(
        test_binaries: impl IntoIterator<Item = TestBinary>,
        filter: &TestFilter,
    ) -> Result<Self> {
        let mut test_count = 0;

        let test_binaries = test_binaries
            .into_iter()
            .map(|test_binary| {
                let (non_ignored, ignored) = test_binary.exec()?;
                let (bin, info) = Self::process_output(
                    test_binary,
                    filter,
                    non_ignored.as_str(),
                    ignored.as_str(),
                )?;
                test_count += info.tests.len();
                Ok((bin, info))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self {
            test_binaries,
            test_count,
            skip_count: OnceCell::new(),
        })
    }

    /// Creates a new test list with the given binary names and outputs.
    pub fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<Item = (TestBinary, impl AsRef<str>, impl AsRef<str>)>,
        filter: &TestFilter,
    ) -> Result<Self> {
        let mut test_count = 0;

        let test_binaries = test_bin_outputs
            .into_iter()
            .map(|(test_binary, non_ignored, ignored)| {
                let (bin, info) = Self::process_output(
                    test_binary,
                    filter,
                    non_ignored.as_ref(),
                    ignored.as_ref(),
                )?;
                test_count += info.tests.len();
                Ok((bin, info))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self {
            test_binaries,
            test_count,
            skip_count: OnceCell::new(),
        })
    }

    /// Returns the total number of tests across all binaries.
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Returns the total number of skipped tests.
    pub fn skip_count(&self) -> usize {
        *self.skip_count.get_or_init(|| {
            self.iter_tests()
                .filter(|instance| !instance.info.filter_match.is_match())
                .count()
        })
    }

    /// Returns the total number of tests that aren't skipped.
    ///
    /// It is always the case that `run_count + skip_count == test_count`.
    pub fn run_count(&self) -> usize {
        self.test_count - self.skip_count()
    }

    /// Returns the total number of binaries that contain tests.
    pub fn binary_count(&self) -> usize {
        self.test_binaries.len()
    }

    /// Returns the tests for a given binary, or `None` if the binary wasn't in the list.
    pub fn get(&self, test_bin: impl AsRef<Utf8Path>) -> Option<&TestBinInfo> {
        self.test_binaries.get(test_bin.as_ref())
    }

    /// Outputs this list to the given writer.
    pub fn write(&self, output_format: OutputFormat, writer: impl WriteColor) -> Result<()> {
        match output_format {
            OutputFormat::Plain => self.write_plain(writer).context("error writing test list"),
            OutputFormat::Serializable(format) => format.to_writer(self, writer),
        }
    }

    /// Iterates over all the test binaries.
    pub fn iter(&self) -> impl Iterator<Item = (&Utf8Path, &TestBinInfo)> + '_ {
        self.test_binaries
            .iter()
            .map(|(path, info)| (path.as_path(), info))
    }

    /// Iterates over the list of tests, returning the path and test name.
    pub fn iter_tests(&self) -> impl Iterator<Item = TestInstance<'_>> + '_ {
        self.test_binaries.iter().flat_map(|(test_bin, bin_info)| {
            bin_info.tests.iter().map(move |(name, info)| {
                TestInstance::new(
                    name,
                    test_bin,
                    bin_info.friendly_name.as_deref(),
                    info,
                    bin_info.cwd.as_deref(),
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
        non_ignored: impl AsRef<str>,
        ignored: impl AsRef<str>,
    ) -> Result<(Utf8PathBuf, TestBinInfo)> {
        let TestBinary {
            binary,
            cwd,
            friendly_name,
        } = test_binary;

        let mut tests = BTreeMap::new();
        for test_name in Self::parse(non_ignored.as_ref()) {
            let test_name = test_name?;
            tests.insert(
                test_name.into(),
                RustTestInfo {
                    filter_match: filter.filter_match(&test_name, false),
                },
            );
        }

        for test_name in Self::parse(ignored.as_ref()) {
            let test_name = test_name?;
            // TODO: catch dups
            tests.insert(
                test_name.into(),
                RustTestInfo {
                    filter_match: filter.filter_match(&test_name, true),
                },
            );
        }

        Ok((
            binary,
            TestBinInfo {
                tests,
                cwd,
                friendly_name,
            },
        ))
    }

    /// Parses the output of --list --format terse.
    fn parse(
        list_output: &(impl AsRef<str> + ?Sized),
    ) -> impl Iterator<Item = Result<&'_ str>> + '_ {
        Self::parse_impl(list_output.as_ref())
    }

    fn parse_impl(list_output: &str) -> impl Iterator<Item = Result<&'_ str>> + '_ {
        // The output is in the form:
        // <test name>: test
        // <test name>: test
        // ...

        list_output.lines().map(move |line| {
            line.strip_suffix(": test").ok_or_else(|| {
                anyhow!(
                    "line '{}' did not end with the string ': test', full output:\n{}",
                    line,
                    list_output
                )
            })
        })
    }

    fn write_plain(&self, mut writer: impl WriteColor) -> io::Result<()> {
        let test_bin_spec = test_bin_spec();
        let field_spec = Self::field_spec();
        let test_name_spec = test_name_spec();

        for (test_bin, info) in &self.test_binaries {
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

            for (name, info) in &info.tests {
                writer.set_color(&test_name_spec)?;
                write!(writer, "    {}", name)?;
                writer.reset()?;

                if !info.filter_match.is_match() {
                    write!(writer, " (skipped)")?;
                }
                writeln!(writer)?;
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

impl TestBinary {
    /// Run this binary with and without --ignored and get the corresponding outputs.
    fn exec(&self) -> Result<(String, String)> {
        let non_ignored = self.exec_single(false)?;
        let ignored = self.exec_single(true)?;
        Ok((non_ignored, ignored))
    }

    fn exec_single(&self, ignored: bool) -> Result<String> {
        let mut argv = vec!["--list", "--format", "terse"];
        if ignored {
            argv.push("--ignored");
        }
        let mut cmd = cmd(AsRef::<Path>::as_ref(&self.binary), argv).stdout_capture();
        if let Some(cwd) = &self.cwd {
            cmd = cmd.dir(cwd);
        };

        cmd.read().with_context(|| {
            format!(
                "running '{} --list --format --terse{}' failed",
                self.binary,
                if ignored { " --ignored" } else { "" }
            )
        })
    }
}

/// Represents a single test with its associated binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestInstance<'a> {
    /// The name of the test.
    pub name: &'a str,

    /// The test binary.
    pub binary: &'a Utf8Path,

    /// The friendly name of the binary, if any.
    pub friendly_name: Option<&'a str>,

    /// Information about the test.
    pub info: &'a RustTestInfo,

    /// The working directory for this test. If None, the test will not be changed.
    pub cwd: Option<&'a Utf8Path>,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub(crate) fn new(
        name: &'a (impl AsRef<str> + ?Sized),
        binary: &'a (impl AsRef<Utf8Path> + ?Sized),
        friendly_name: Option<&'a str>,
        info: &'a RustTestInfo,
        cwd: Option<&'a Utf8Path>,
    ) -> Self {
        Self {
            name: name.as_ref(),
            binary: binary.as_ref(),
            friendly_name,
            info,
            cwd,
        }
    }
}

pub(super) fn test_bin_spec() -> ColorSpec {
    let mut color_spec = ColorSpec::new();
    color_spec
        .set_fg(Some(termcolor::Color::Magenta))
        .set_bold(true);
    color_spec
}

pub(super) fn test_name_spec() -> ColorSpec {
    let mut color_spec = ColorSpec::new();
    color_spec
        .set_fg(Some(termcolor::Color::Blue))
        .set_bold(true);
    color_spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        output::SerializableFormat,
        test_filter::{FilterMatch, MismatchReason, RunIgnored},
    };
    use indoc::indoc;
    use maplit::btreemap;
    use pretty_assertions::assert_eq;
    use std::iter;

    #[test]
    fn test_parse() {
        let non_ignored_output = indoc! {"
            tests::foo::test_bar: test
            tests::baz::test_quux: test
        "};
        let ignored_output = indoc! {"
            tests::ignored::test_bar: test
            tests::baz::test_ignored: test
        "};

        let test_filter = TestFilter::any(RunIgnored::Default);
        let fake_cwd: Utf8PathBuf = "/fake/cwd".into();
        let fake_friendly_name = "fake-package".to_owned();
        let test_binary = TestBinary {
            binary: "/fake/binary".into(),
            cwd: Some(fake_cwd.clone()),
            friendly_name: Some(fake_friendly_name.clone()),
        };
        let test_list = TestList::new_with_outputs(
            iter::once((test_binary, &non_ignored_output, &ignored_output)),
            &test_filter,
        )
        .expect("valid output");
        assert_eq!(
            test_list.test_binaries,
            btreemap! {
                "/fake/binary".into() => TestBinInfo {
                    tests: btreemap! {
                        "tests::foo::test_bar".to_owned() => RustTestInfo {
                            filter_match: FilterMatch::Matches,
                        },
                        "tests::baz::test_quux".to_owned() => RustTestInfo {
                            filter_match: FilterMatch::Matches,
                        },
                        "tests::ignored::test_bar".to_owned() => RustTestInfo {
                            filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                        },
                        "tests::baz::test_ignored".to_owned() => RustTestInfo {
                            filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                        },
                    },
                    cwd: Some(fake_cwd),
                    friendly_name: Some(fake_friendly_name),
                }
            }
        );

        // Check that the expected outputs are valid.
        static EXPECTED_PLAIN: &str = indoc! {"
            /fake/binary:
              cwd: /fake/cwd
                tests::baz::test_ignored (skipped)
                tests::baz::test_quux
                tests::foo::test_bar
                tests::ignored::test_bar (skipped)
        "};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "test-count": 4,
              "test-binaries": {
                "/fake/binary": {
                  "friendly-name": "fake-package",
                  "tests": {
                    "tests::baz::test_ignored": {
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_quux": {
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::foo::test_bar": {
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::ignored::test_bar": {
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    }
                  },
                  "cwd": "/fake/cwd"
                }
              }
            }"#};

        assert_eq!(
            test_list
                .to_string(OutputFormat::Plain)
                .expect("plain succeeded"),
            EXPECTED_PLAIN
        );
        assert_eq!(
            test_list
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded"),
            EXPECTED_JSON_PRETTY
        );
    }
}
