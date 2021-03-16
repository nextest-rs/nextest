// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputFormat, test_filter::TestFilter};
use anyhow::{anyhow, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use duct::cmd;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};

// TODO: capture ignored and not-ignored tests

/// Represents a test binary.
///
/// Accepted as input to `TestList::new`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestBinary {
    /// The test binary.
    pub binary: Utf8PathBuf,

    /// The working directory that this test should be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Option<Utf8PathBuf>,
}

/// List of tests, gotten by executing a test binary with the `--list` command.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestList {
    tests: BTreeMap<Utf8PathBuf, TestBinInfo>,
}

/// Information about a test binary.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinInfo {
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
        let tests = test_binaries
            .into_iter()
            .map(|test_binary| {
                let TestBinary { binary, cwd } = test_binary;

                let mut cmd = cmd!(
                    AsRef::<Path>::as_ref(&binary),
                    "--list",
                    "--format",
                    "terse"
                )
                .stdout_capture();
                if let Some(cwd) = &cwd {
                    cmd = cmd.dir(cwd);
                };

                let output = cmd.read().with_context(|| {
                    format!("running '{} --list --format --terse' failed", binary)
                })?;

                // Parse the output.
                let test_names = Self::parse(output, filter)?;

                Ok((binary, TestBinInfo { test_names, cwd }))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self { tests })
    }

    /// Creates a new test list with the given binary names and outputs.
    pub fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<Item = (TestBinary, impl AsRef<str>)>,
        filter: &TestFilter,
    ) -> Result<Self> {
        let tests = test_bin_outputs
            .into_iter()
            .map(|(test_binary, output)| {
                let TestBinary { binary, cwd } = test_binary;

                let output = output.as_ref();
                let test_names = Self::parse(output, filter)?;

                Ok((binary, TestBinInfo { test_names, cwd }))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self { tests })
    }

    /// Outputs this list to the given writer.
    pub fn write(&self, output_format: OutputFormat, mut writer: impl io::Write) -> Result<()> {
        match output_format {
            OutputFormat::Plain => {
                for (test_bin, info) in &self.tests {
                    writeln!(writer, "{}:", test_bin).context("error writing output")?;
                    if let Some(cwd) = &info.cwd {
                        writeln!(writer, "  cwd: {}", cwd).context("error writing output")?;
                    }
                    for test_name in &info.test_names {
                        writeln!(writer, "    {}", test_name).context("error writing output")?;
                    }
                }
                Ok(())
            }
            OutputFormat::Serializable(format) => format.to_writer(self, writer),
        }
    }

    /// Returns the tests for a given binary, or `None` if the binary wasn't in the list.
    pub fn get(&self, test_bin: impl AsRef<Utf8Path>) -> Option<&TestBinInfo> {
        self.tests.get(test_bin.as_ref())
    }

    /// Iterates over the list of tests, returning the path and test name.
    pub fn iter(&self) -> impl Iterator<Item = TestInstance<'_>> + '_ {
        self.tests.iter().flat_map(|(test_bin, info)| {
            info.test_names
                .iter()
                .map(move |test_name| TestInstance::new(test_bin, test_name, info.cwd.as_deref()))
        })
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String> {
        // Ugh this sucks. String really should have an io::Write impl that errors on non-UTF8 text.
        let mut buf = vec![];
        self.write(output_format, &mut buf)?;
        Ok(String::from_utf8(buf).expect("buffer is valid UTF-8"))
    }

    // ---
    // Helper methods
    // ---

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
}

/// Represents a single test with its associated binary.
#[derive(Clone, Copy, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct TestInstance<'a> {
    /// The test binary.
    pub binary: &'a Utf8Path,

    /// The name of the test.
    pub test_name: &'a str,

    /// The working directory for this test. If None, the test will not be changed.
    pub cwd: Option<&'a Utf8Path>,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub fn new(
        binary: &'a (impl AsRef<Utf8Path> + ?Sized),
        test_name: &'a (impl AsRef<str> + ?Sized),
        cwd: Option<&'a Utf8Path>,
    ) -> Self {
        Self {
            binary: binary.as_ref(),
            test_name: test_name.as_ref(),
            cwd,
        }
    }
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
        let test_binary = TestBinary {
            binary: "/fake/binary".into(),
            cwd: Some(fake_cwd.clone()),
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
        static EXPECTED_JSON: &str = r#"{"tests":{"/fake/binary":{"test-names":["tests::foo::test_bar","tests::baz::test_quux"],"cwd":"/fake/cwd"}}}"#;
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "tests": {
                "/fake/binary": {
                  "test-names": [
                    "tests::foo::test_bar",
                    "tests::baz::test_quux"
                  ],
                  "cwd": "/fake/cwd"
                }
              }
            }"#};
        static EXPECTED_TOML: &str = indoc! {r#"
            [tests."/fake/binary"]
            test-names = ["tests::foo::test_bar", "tests::baz::test_quux"]
            cwd = "/fake/cwd"
        "#};
        static EXPECTED_TOML_PRETTY: &str = indoc! {r#"
            [tests."/fake/binary"]
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
                .to_string(OutputFormat::Serializable(SerializableFormat::Json))
                .expect("json succeeded"),
            EXPECTED_JSON
        );
        assert_eq!(
            tests
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded"),
            EXPECTED_JSON_PRETTY
        );
        assert_eq!(
            tests
                .to_string(OutputFormat::Serializable(SerializableFormat::Toml))
                .expect("toml succeeded"),
            EXPECTED_TOML
        );
        assert_eq!(
            tests
                .to_string(OutputFormat::Serializable(SerializableFormat::TomlPretty))
                .expect("toml-pretty succeeded"),
            EXPECTED_TOML_PRETTY
        );
    }
}
