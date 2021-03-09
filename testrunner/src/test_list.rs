// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputFormat, test_filter::TestFilter};
use anyhow::{anyhow, Context, Result};
use camino::Utf8Path;
use duct::cmd;
use serde::Serialize;
use std::{io, path::Path};

// TODO: capture ignored and not-ignored tests

/// List of tests, gotten by executing a test binary with the `--list` command.
#[derive(Debug, Serialize)]
pub struct TestList {
    tests: Vec<Box<str>>,
}

impl TestList {
    /// Creates a new test list by running the given command and applying the specified filter.
    pub fn new(test_bin: &Utf8Path, filter: &TestFilter) -> Result<Self> {
        let output = cmd!(
            AsRef::<Path>::as_ref(test_bin),
            "--list",
            "--format",
            "terse"
        )
        .stdout_capture()
        .read()
        .with_context(|| format!("running '{} --list --format --terse' failed", test_bin))?;

        // Parse the output.
        Self::parse(output, filter)
    }

    /// Creates a new test list by parsing the output of --list --format terse.
    pub fn parse(list_output: impl AsRef<str>, filter: &TestFilter) -> Result<Self> {
        let tests = Self::parse_impl(list_output.as_ref(), filter)?;
        Ok(Self { tests })
    }

    /// Outputs this list to the given writer.
    pub fn write(&self, output_format: OutputFormat, mut writer: impl io::Write) -> Result<()> {
        match output_format {
            OutputFormat::Plain => {
                for test in &self.tests {
                    writeln!(writer, "{}", test).context("error writing output")?;
                }
                Ok(())
            }
            OutputFormat::Serializable(format) => format.to_writer(self, writer),
        }
    }

    /// Iterates over the list of tests.
    pub fn iter(&self) -> impl Iterator<Item = &'_ str> + '_ {
        self.tests.iter().map(|s| &**s)
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

    fn parse_impl(list_output: &str, filter: &TestFilter) -> Result<Vec<Box<str>>> {
        // The output is in the form:
        // <test name>: test
        // <test name>: test
        // ...

        let mut tests = vec![];
        for line in list_output.lines() {
            let test_name = line
                .strip_suffix(": test")
                .ok_or_else(|| anyhow!("line '{}' did not end with the string ': test'", line))?;
            if filter.is_match(test_name) {
                tests.push(test_name.into());
            }
        }
        Ok(tests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::SerializableFormat;
    use indoc::indoc;

    #[test]
    fn test_parse() {
        let list_output = indoc! {"
            tests::foo::test_bar: test
            tests::baz::test_quux: test
        "};

        let test_filter = TestFilter::any();
        let tests = TestList::parse(&list_output, &test_filter).expect("valid output");
        assert_eq!(
            tests.tests,
            vec![
                "tests::foo::test_bar".to_owned().into_boxed_str(),
                "tests::baz::test_quux".to_owned().into_boxed_str(),
            ]
        );

        // Check that the expected outputs are valid.
        static EXPECTED_PLAIN: &str = indoc! {"
            tests::foo::test_bar
            tests::baz::test_quux
        "};
        static EXPECTED_JSON: &str =
            r#"{"tests":["tests::foo::test_bar","tests::baz::test_quux"]}"#;
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
            {
              "tests": [
                "tests::foo::test_bar",
                "tests::baz::test_quux"
              ]
            }"#};
        static EXPECTED_TOML: &str =
            "tests = [\"tests::foo::test_bar\", \"tests::baz::test_quux\"]\n";
        static EXPECTED_TOML_PRETTY: &str = indoc! {r#"
            tests = [
                'tests::foo::test_bar',
                'tests::baz::test_quux',
            ]
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
