// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{FromMessagesError, WriteTestListError},
    list::{BinaryListState, OutputFormat, RustMetadata, Styles},
};
use camino::Utf8PathBuf;
use cargo_metadata::Message;
use guppy::{graph::PackageGraph, PackageId};
use nextest_metadata::{BinaryListSummary, BuildPlatform, RustTestBinarySummary};
use owo_colors::OwoColorize;
use std::{io, io::Write};

/// A Rust test binary built by Cargo.
#[derive(Clone, Debug)]
pub struct RustTestBinary {
    /// A unique ID.
    pub id: String,
    /// The path to the binary artifact.
    pub path: Utf8PathBuf,
    /// The package this artifact belongs to.
    pub package_id: String,
    /// The unique binary name defined in `Cargo.toml` or inferred by the filename.
    pub name: String,
    /// Platform for which this binary was built.
    /// (Proc-macro tests are built for the host.)
    pub build_platform: BuildPlatform,
}

/// The list of Rust test binaries built by Cargo.
#[derive(Clone, Debug)]
pub struct BinaryList {
    /// Rust-related metadata.
    pub rust_metadata: RustMetadata<BinaryListState>,

    /// The list of test binaries.
    pub rust_binaries: Vec<RustTestBinary>,
}

impl BinaryList {
    /// Parses Cargo messages from the given `BufRead` and returns a list of test binaries.
    pub fn from_messages(
        reader: impl io::BufRead,
        graph: &PackageGraph,
    ) -> Result<Self, FromMessagesError> {
        let mut state = BinaryListBuildState::new(graph);

        for message in Message::parse_stream(reader) {
            let message = message.map_err(FromMessagesError::ReadMessages)?;
            state.process_message(message)?;
        }

        Ok(state.finish())
    }

    /// Constructs the list from its summary format
    pub fn from_summary(summary: BinaryListSummary) -> Self {
        let rust_binaries = summary
            .rust_binaries
            .into_values()
            .map(|bin| RustTestBinary {
                name: bin.binary_name,
                path: bin.binary_path,
                package_id: bin.package_id,
                id: bin.binary_id,
                build_platform: bin.build_platform,
            })
            .collect();
        Self {
            rust_metadata: RustMetadata::from_summary(summary.rust_metadata),
            rust_binaries,
        }
    }

    /// Outputs this list to the given writer.
    pub fn write(
        &self,
        output_format: OutputFormat,
        writer: impl Write,
        colorize: bool,
    ) -> Result<(), WriteTestListError> {
        match output_format {
            OutputFormat::Human { verbose } => self
                .write_human(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Serializable(format) => format
                .to_writer(&self.to_summary(), writer)
                .map_err(WriteTestListError::Json),
        }
    }

    fn to_summary(&self) -> BinaryListSummary {
        let rust_binaries = self
            .rust_binaries
            .iter()
            .map(|bin| {
                let summary = RustTestBinarySummary {
                    binary_name: bin.name.clone(),
                    package_id: bin.package_id.clone(),
                    binary_path: bin.path.clone(),
                    binary_id: bin.id.clone(),
                    build_platform: bin.build_platform,
                };
                (bin.id.clone(), summary)
            })
            .collect();

        BinaryListSummary {
            rust_metadata: self.rust_metadata.to_summary(),
            rust_binaries,
        }
    }

    fn write_human(&self, mut writer: impl Write, verbose: bool, colorize: bool) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }
        for bin in &self.rust_binaries {
            if verbose {
                writeln!(writer, "{}:", bin.id.style(styles.binary_id))?;
                writeln!(writer, "  {} {}", "bin:".style(styles.field), bin.path)?;
                writeln!(
                    writer,
                    "  {} {}",
                    "build platform:".style(styles.field),
                    bin.build_platform,
                )?;
            } else {
                writeln!(writer, "{}", bin.id.style(styles.binary_id))?;
            }
        }
        Ok(())
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String, WriteTestListError> {
        // Ugh this sucks. String really should have an io::Write impl that errors on non-UTF8 text.
        let mut buf = Vec::with_capacity(1024);
        self.write(output_format, &mut buf, false)?;
        Ok(String::from_utf8(buf).expect("buffer is valid UTF-8"))
    }
}

#[derive(Debug)]
struct BinaryListBuildState<'g> {
    graph: &'g PackageGraph,
    rust_target_dir: Utf8PathBuf,
    rust_binaries: Vec<RustTestBinary>,
}

impl<'g> BinaryListBuildState<'g> {
    fn new(graph: &'g PackageGraph) -> Self {
        let rust_target_dir = graph.workspace().target_directory().to_path_buf();

        Self {
            graph,
            rust_target_dir,
            rust_binaries: vec![],
        }
    }

    fn process_message(&mut self, message: Message) -> Result<(), FromMessagesError> {
        match message {
            Message::CompilerArtifact(artifact) if artifact.profile.test => {
                if let Some(path) = artifact.executable {
                    let package_id = artifact.package_id.repr;

                    // Look up the executable by package ID.
                    let package = self
                        .graph
                        .metadata(&PackageId::new(package_id.clone()))
                        .map_err(FromMessagesError::PackageGraph)?;

                    // Construct the binary ID from the package and build target.
                    let mut id = package.name().to_owned();
                    let name = artifact.target.name;

                    // To ensure unique binary IDs, we use the following scheme:
                    // 1. If the target is a lib, use the package name.
                    //      There can only be one lib per package, so this
                    //      will always be unique.
                    if !artifact.target.kind.contains(&"lib".to_owned()) {
                        id.push_str("::");

                        match artifact.target.kind.get(0) {
                            // 2. For integration tests, use the target name.
                            //      Cargo enforces unique names for the same
                            //      kind of targets in a package, so these
                            //      will always be unique.
                            Some(kind) if kind == "test" => {
                                id.push_str(&name);
                            }
                            // 3. For all other target kinds, use a
                            //      combination of the target kind and
                            //      the target name. For the same reason
                            //      as above, these will always be unique.
                            Some(kind) => {
                                id.push_str(&format!("{}/{}", kind, name));
                            }
                            None => {
                                return Err(FromMessagesError::MissingTargetKind {
                                    package_name: package.name().to_owned(),
                                    binary_name: name.clone(),
                                });
                            }
                        }
                    }

                    let platform = if artifact.target.kind.len() == 1
                        && artifact.target.kind.get(0).map(String::as_str) == Some("proc-macro")
                    {
                        BuildPlatform::Host
                    } else {
                        BuildPlatform::Target
                    };

                    self.rust_binaries.push(RustTestBinary {
                        path,
                        package_id,
                        name,
                        id,
                        build_platform: platform,
                    })
                }
            }
            _ => {
                // Ignore all other messages.
            }
        }

        Ok(())
    }

    fn finish(mut self) -> BinaryList {
        self.rust_binaries.sort_by(|b1, b2| b1.id.cmp(&b2.id));
        BinaryList {
            rust_metadata: RustMetadata::new(self.rust_target_dir),
            rust_binaries: self.rust_binaries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::SerializableFormat;
    use indoc::indoc;

    #[test]
    fn test_parse_binary_list() {
        let fake_bin_test = RustTestBinary {
            id: "fake-package::bin/fake-binary".to_owned(),
            path: "/fake/binary".into(),
            package_id: "fake-package 0.1.0 (path+file:///Users/fakeuser/project/fake-package)"
                .to_owned(),
            name: "fake-binary".to_owned(),
            build_platform: BuildPlatform::Target,
        };
        let fake_macro_test = RustTestBinary {
            id: "fake-macro::proc-macro/fake-macro".to_owned(),
            path: "/fake/macro".into(),
            package_id: "fake-macro 0.1.0 (path+file:///Users/fakeuser/project/fake-macro)"
                .to_owned(),
            name: "fake-macro".to_owned(),
            build_platform: BuildPlatform::Host,
        };

        let binary_list = BinaryList {
            rust_metadata: RustMetadata::new("/fake"),
            rust_binaries: vec![fake_bin_test, fake_macro_test],
        };

        // Check that the expected outputs are valid.
        static EXPECTED_HUMAN: &str = indoc! {"
        fake-package::bin/fake-binary
        fake-macro::proc-macro/fake-macro
        "};
        static EXPECTED_HUMAN_VERBOSE: &str = indoc! {r#"
        fake-package::bin/fake-binary:
          bin: /fake/binary
          build platform: target
        fake-macro::proc-macro/fake-macro:
          bin: /fake/macro
          build platform: host
        "#};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
        {
          "rust-metadata": {
            "target-directory": "/fake"
          },
          "rust-binaries": {
            "fake-macro::proc-macro/fake-macro": {
              "binary-id": "fake-macro::proc-macro/fake-macro",
              "binary-name": "fake-macro",
              "package-id": "fake-macro 0.1.0 (path+file:///Users/fakeuser/project/fake-macro)",
              "binary-path": "/fake/macro",
              "build-platform": "host"
            },
            "fake-package::bin/fake-binary": {
              "binary-id": "fake-package::bin/fake-binary",
              "binary-name": "fake-binary",
              "package-id": "fake-package 0.1.0 (path+file:///Users/fakeuser/project/fake-package)",
              "binary-path": "/fake/binary",
              "build-platform": "target"
            }
          }
        }"#};

        assert_eq!(
            binary_list
                .to_string(OutputFormat::Human { verbose: false })
                .expect("human succeeded"),
            EXPECTED_HUMAN
        );
        assert_eq!(
            binary_list
                .to_string(OutputFormat::Human { verbose: true })
                .expect("human succeeded"),
            EXPECTED_HUMAN_VERBOSE
        );
        assert_eq!(
            binary_list
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeded"),
            EXPECTED_JSON_PRETTY
        );
    }
}
