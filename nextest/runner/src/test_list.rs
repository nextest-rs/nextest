// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

mod output_format;
pub use output_format::*;

use crate::{
    errors::{FromMessagesError, ParseTestListError, WriteTestListError},
    test_filter::{FilterMatch, TestFilterBuilder},
};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::Message;
use duct::{cmd, Expression};
use guppy::graph::PackageMetadata;
use guppy::{graph::PackageGraph, PackageId};
use once_cell::sync::OnceCell;
use owo_colors::{OwoColorize, Style};
use serde::Serialize;
use std::{collections::BTreeMap, io, io::Write, path::Path};

// TODO: capture ignored and not-ignored tests

/// Represents a test binary.
///
/// Accepted as input to `TestList::new`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinary<'g> {
    /// The test binary.
    pub binary: Utf8PathBuf,

    /// A unique identifier for this binary, typically the package + binary name defined in
    /// `Cargo.toml`.
    pub binary_id: String,

    /// Metadata for the package this binary is part of. This is used to set the correct
    /// environment variables.
    #[serde(skip)]
    pub package: PackageMetadata<'g>,

    /// The working directory that this test should be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Utf8PathBuf,
}

impl<'g> TestBinary<'g> {
    /// Parse Cargo messages from the given `BufRead` and return a list of test binaries.
    pub fn from_messages(
        graph: &'g PackageGraph,
        reader: impl io::BufRead,
    ) -> Result<Vec<Self>, FromMessagesError> {
        let mut binaries = vec![];

        for message in Message::parse_stream(reader) {
            let message = message.map_err(FromMessagesError::ReadMessages)?;
            match message {
                Message::CompilerArtifact(artifact) if artifact.profile.test => {
                    if let Some(binary) = artifact.executable {
                        // Look up the executable by package ID.
                        let package_id = PackageId::new(artifact.package_id.repr);
                        let package = graph
                            .metadata(&package_id)
                            .map_err(FromMessagesError::PackageGraph)?;

                        // Tests are run in the directory containing Cargo.toml
                        let cwd = package
                            .manifest_path()
                            .parent()
                            .unwrap_or_else(|| {
                                panic!(
                                    "manifest path {} doesn't have a parent",
                                    package.manifest_path()
                                )
                            })
                            .to_path_buf();

                        // Construct the binary ID from the package and build target.
                        let mut binary_id = package.name().to_owned();
                        if artifact.target.name != package.name() {
                            binary_id.push_str("::");
                            binary_id.push_str(&artifact.target.name);
                        }

                        binaries.push(TestBinary {
                            binary,
                            binary_id,
                            package,
                            cwd,
                        })
                    }
                }
                _ => {
                    // Ignore all other messages.
                }
            }
        }

        Ok(binaries)
    }
}

/// List of tests, gotten by executing a test binary with the `--list` command.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestList<'g> {
    /// Number of tests (including skipped and ignored) across all binaries.
    test_count: usize,
    test_binaries: BTreeMap<Utf8PathBuf, TestBinInfo<'g>>,
    #[serde(skip)]
    styles: Box<Styles>,
    // Values computed on first access.
    #[serde(skip)]
    skip_count: OnceCell<usize>,
}

/// Information about a test binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestBinInfo<'g> {
    /// A unique identifier for this binary, typically the package + binary name defined in
    /// `Cargo.toml`.
    pub binary_id: String,

    /// Test names and other information.
    pub tests: BTreeMap<String, RustTestInfo>,

    /// Package metadata.
    #[serde(skip)]
    pub package: PackageMetadata<'g>,

    /// The working directory that this test binary will be executed in. If None, the current directory
    /// will not be changed.
    pub cwd: Utf8PathBuf,
}

/// Information about a single Rust test.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RustTestInfo {
    /// Returns true if this test is marked ignored.
    ///
    /// Ignored tests, if run, are executed with the `--ignored` argument.
    pub ignored: bool,

    /// Whether the test matches the provided test filter.
    ///
    /// Only tests that match the filter are run.
    pub filter_match: FilterMatch,
}

impl<'g> TestList<'g> {
    /// Creates a new test list by running the given command and applying the specified filter.
    pub fn new(
        test_binaries: impl IntoIterator<Item = TestBinary<'g>>,
        filter: &TestFilterBuilder,
    ) -> Result<Self, ParseTestListError> {
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
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(Self {
            test_binaries,
            test_count,
            styles: Box::new(Styles::default()),
            skip_count: OnceCell::new(),
        })
    }

    /// Creates a new test list with the given binary names and outputs.
    pub fn new_with_outputs(
        test_bin_outputs: impl IntoIterator<Item = (TestBinary<'g>, impl AsRef<str>, impl AsRef<str>)>,
        filter: &TestFilterBuilder,
    ) -> Result<Self, ParseTestListError> {
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
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(Self {
            test_binaries,
            test_count,
            styles: Box::new(Styles::default()),
            skip_count: OnceCell::new(),
        })
    }

    /// Colorizes output.
    pub fn colorize(&mut self) {
        self.styles.colorize();
    }

    /// Returns the total number of tests across all binaries.
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Returns the total number of skipped tests.
    pub fn skip_count(&self) -> usize {
        *self.skip_count.get_or_init(|| {
            self.iter_tests()
                .filter(|instance| !instance.test_info.filter_match.is_match())
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
    pub fn write(
        &self,
        output_format: OutputFormat,
        writer: impl Write,
    ) -> Result<(), WriteTestListError> {
        match output_format {
            OutputFormat::Plain => self.write_plain(writer).map_err(WriteTestListError::Io),
            OutputFormat::Serializable(format) => format
                .to_writer(self, writer)
                .map_err(WriteTestListError::Json),
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
            bin_info.tests.iter().map(move |(name, test_info)| {
                TestInstance::new(name, test_bin, bin_info, test_info)
            })
        })
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String, WriteTestListError> {
        // Ugh this sucks. String really should have an io::Write impl that errors on non-UTF8 text.
        let mut buf = Vec::with_capacity(1024);
        self.write(output_format, &mut buf)?;
        Ok(String::from_utf8(buf).expect("buffer is valid UTF-8"))
    }

    // ---
    // Helper methods
    // ---

    fn process_output(
        test_binary: TestBinary<'g>,
        filter: &TestFilterBuilder,
        non_ignored: impl AsRef<str>,
        ignored: impl AsRef<str>,
    ) -> Result<(Utf8PathBuf, TestBinInfo<'g>), ParseTestListError> {
        let mut tests = BTreeMap::new();

        // Treat ignored and non-ignored as separate sets of single filters, so that partitioning
        // based on one doesn't affect the other.
        let mut non_ignored_filter = filter.build();
        for test_name in Self::parse(non_ignored.as_ref())? {
            tests.insert(
                test_name.into(),
                RustTestInfo {
                    ignored: false,
                    filter_match: non_ignored_filter.filter_match(test_name, false),
                },
            );
        }

        let mut ignored_filter = filter.build();
        for test_name in Self::parse(ignored.as_ref())? {
            // TODO: catch dups
            tests.insert(
                test_name.into(),
                RustTestInfo {
                    ignored: true,
                    filter_match: ignored_filter.filter_match(test_name, true),
                },
            );
        }

        let TestBinary {
            binary,
            package,
            cwd,
            binary_id,
        } = test_binary;

        Ok((
            binary,
            TestBinInfo {
                binary_id,
                tests,
                package,
                cwd,
            },
        ))
    }

    /// Parses the output of --list --format terse and returns a sorted list.
    fn parse(list_output: &str) -> Result<Vec<&'_ str>, ParseTestListError> {
        let mut list = Self::parse_impl(list_output).collect::<Result<Vec<_>, _>>()?;
        list.sort_unstable();
        Ok(list)
    }

    fn parse_impl(
        list_output: &str,
    ) -> impl Iterator<Item = Result<&'_ str, ParseTestListError>> + '_ {
        // The output is in the form:
        // <test name>: test
        // <test name>: test
        // ...

        list_output.lines().map(move |line| {
            line.strip_suffix(": test").ok_or_else(|| {
                ParseTestListError::parse_line(
                    format!("line '{}' did not end with the string ': test'", line),
                    list_output,
                )
            })
        })
    }

    fn write_plain(&self, mut writer: impl Write) -> io::Result<()> {
        for (test_bin, info) in &self.test_binaries {
            writeln!(writer, "{}:", test_bin.style(self.styles.test_bin))?;
            writeln!(writer, "  {} {}", "cwd:".style(self.styles.field), info.cwd)?;

            for (name, info) in &info.tests {
                write!(writer, "    {}", name.style(self.styles.test_name))?;
                if !info.filter_match.is_match() {
                    write!(writer, " (skipped)")?;
                }
                writeln!(writer)?;
            }
        }
        Ok(())
    }
}

impl<'g> TestBinary<'g> {
    /// Run this binary with and without --ignored and get the corresponding outputs.
    fn exec(&self) -> Result<(String, String), ParseTestListError> {
        let non_ignored = self.exec_single(false)?;
        let ignored = self.exec_single(true)?;
        Ok((non_ignored, ignored))
    }

    fn exec_single(&self, ignored: bool) -> Result<String, ParseTestListError> {
        let mut argv = vec!["--list", "--format", "terse"];
        if ignored {
            argv.push("--ignored");
        }
        let cmd = cmd(AsRef::<Path>::as_ref(&self.binary), argv)
            .dir(&self.cwd)
            .stdout_capture();

        cmd.read().map_err(|error| {
            ParseTestListError::command(
                format!(
                    "'{} --list --format --terse{}'",
                    self.binary,
                    if ignored { " --ignored" } else { "" }
                ),
                error,
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

    /// Information about the binary.
    pub bin_info: &'a TestBinInfo<'a>,

    /// Information about the test.
    pub test_info: &'a RustTestInfo,
}

impl<'a> TestInstance<'a> {
    /// Creates a new `TestInstance`.
    pub(crate) fn new(
        name: &'a (impl AsRef<str> + ?Sized),
        binary: &'a (impl AsRef<Utf8Path> + ?Sized),
        bin_info: &'a TestBinInfo,
        test_info: &'a RustTestInfo,
    ) -> Self {
        Self {
            name: name.as_ref(),
            binary: binary.as_ref(),
            bin_info,
            test_info,
        }
    }

    /// Creates the command expression for this test instance.
    pub(crate) fn make_expression(&self) -> Expression {
        // TODO: non-rust tests
        let mut args = vec!["--exact", self.name, "--nocapture"];
        if self.test_info.ignored {
            args.push("--ignored");
        }

        let package = self.bin_info.package;

        let cmd = cmd(AsRef::<Path>::as_ref(self.binary), args)
            .dir(&self.bin_info.cwd)
            // These environment variables are set at runtime by cargo test:
            // https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
            .env(
                "CARGO_MANIFEST_DIR",
                package.manifest_path().parent().unwrap(),
            )
            .env("CARGO_PKG_VERSION", format!("{}", package.version()))
            .env(
                "CARGO_PKG_VERSION_MAJOR",
                format!("{}", package.version().major),
            )
            .env(
                "CARGO_PKG_VERSION_MINOR",
                format!("{}", package.version().minor),
            )
            .env(
                "CARGO_PKG_VERSION_PATCH",
                format!("{}", package.version().patch),
            )
            .env(
                "CARGO_PKG_VERSION_PRE",
                format!("{}", package.version().pre),
            )
            .env("CARGO_PKG_AUTHORS", package.authors().join(":"))
            .env("CARGO_PKG_NAME", package.name())
            .env(
                "CARGO_PKG_DESCRIPTION",
                package.description().unwrap_or_default(),
            )
            .env("CARGO_PKG_HOMEPAGE", package.homepage().unwrap_or_default())
            .env("CARGO_PKG_LICENSE", package.license().unwrap_or_default())
            .env(
                "CARGO_PKG_LICENSE_FILE",
                package.license_file().unwrap_or_else(|| "".as_ref()),
            )
            .env(
                "CARGO_PKG_REPOSITORY",
                package.repository().unwrap_or_default(),
            );

        cmd
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Styles {
    pub(super) test_bin: Style,
    pub(super) test_name: Style,
    field: Style,
}

impl Styles {
    pub(super) fn colorize(&mut self) {
        self.test_bin = Style::new().magenta().bold();
        self.test_name = Style::new().blue().bold();
        self.field = Style::new().yellow().bold();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_filter::{FilterMatch, MismatchReason, RunIgnored};
    use guppy::CargoMetadata;
    use indoc::indoc;
    use maplit::btreemap;
    use once_cell::sync::Lazy;
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

        let test_filter = TestFilterBuilder::any(RunIgnored::Default);
        let fake_cwd: Utf8PathBuf = "/fake/cwd".into();
        let fake_binary_id = "fake-package".to_owned();
        let test_binary = TestBinary {
            binary: "/fake/binary".into(),
            cwd: fake_cwd.clone(),
            package: package_metadata(),
            binary_id: fake_binary_id.clone(),
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
                            ignored: false,
                            filter_match: FilterMatch::Matches,
                        },
                        "tests::baz::test_quux".to_owned() => RustTestInfo {
                            ignored: false,
                            filter_match: FilterMatch::Matches,
                        },
                        "tests::ignored::test_bar".to_owned() => RustTestInfo {
                            ignored: true,
                            filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                        },
                        "tests::baz::test_ignored".to_owned() => RustTestInfo {
                            ignored: true,
                            filter_match: FilterMatch::Mismatch { reason: MismatchReason::Ignored },
                        },
                    },
                    cwd: fake_cwd,
                    package: package_metadata(),
                    binary_id: fake_binary_id,
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
                  "binary-id": "fake-package",
                  "tests": {
                    "tests::baz::test_ignored": {
                      "ignored": true,
                      "filter-match": {
                        "status": "mismatch",
                        "reason": "ignored"
                      }
                    },
                    "tests::baz::test_quux": {
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::foo::test_bar": {
                      "ignored": false,
                      "filter-match": {
                        "status": "matches"
                      }
                    },
                    "tests::ignored::test_bar": {
                      "ignored": true,
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

    static PACKAGE_GRAPH_FIXTURE: Lazy<PackageGraph> = Lazy::new(|| {
        static FIXTURE_JSON: &str = include_str!("../../fixtures/cargo-metadata.json");
        let metadata = CargoMetadata::parse_json(FIXTURE_JSON).expect("fixture is valid JSON");
        metadata
            .build_graph()
            .expect("fixture is valid PackageGraph")
    });

    static PACKAGE_METADATA_ID: &str = "metadata-helper 0.1.0 (path+file:///Users/fakeuser/local/testcrates/metadata/metadata-helper)";
    fn package_metadata() -> PackageMetadata<'static> {
        PACKAGE_GRAPH_FIXTURE
            .metadata(&PackageId::new(PACKAGE_METADATA_ID))
            .expect("package ID is valid")
    }
}
