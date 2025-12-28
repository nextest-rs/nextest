// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::{FromMessagesError, RustBuildMetaParseError, WriteTestListError},
    helpers::convert_rel_path_to_forward_slash,
    list::{BinaryListState, OutputFormat, RustBuildMeta, Styles},
    platform::BuildPlatforms,
    write_str::WriteStr,
};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Artifact, BuildScript, Message, PackageId, TargetKind};
use guppy::graph::PackageGraph;
use nextest_metadata::{
    BinaryListSummary, BuildPlatform, RustBinaryId, RustNonTestBinaryKind,
    RustNonTestBinarySummary, RustTestBinaryKind, RustTestBinarySummary,
};
use owo_colors::OwoColorize;
use std::{collections::HashSet, io};
use tracing::warn;

/// A Rust test binary built by Cargo.
#[derive(Clone, Debug)]
pub struct RustTestBinary {
    /// A unique ID.
    pub id: RustBinaryId,
    /// The path to the binary artifact.
    pub path: Utf8PathBuf,
    /// The package this artifact belongs to.
    pub package_id: String,
    /// The kind of Rust test binary this is.
    pub kind: RustTestBinaryKind,
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
    pub rust_build_meta: RustBuildMeta<BinaryListState>,

    /// The list of test binaries.
    pub rust_binaries: Vec<RustTestBinary>,
}

impl BinaryList {
    /// Parses Cargo messages from the given `BufRead` and returns a list of test binaries.
    pub fn from_messages(
        reader: impl io::BufRead,
        graph: &PackageGraph,
        build_platforms: BuildPlatforms,
    ) -> Result<Self, FromMessagesError> {
        let mut state = BinaryListBuildState::new(graph, build_platforms);

        for message in Message::parse_stream(reader) {
            let message = message.map_err(FromMessagesError::ReadMessages)?;
            state.process_message(message)?;
        }

        Ok(state.finish())
    }

    /// Constructs the list from its summary format
    pub fn from_summary(summary: BinaryListSummary) -> Result<Self, RustBuildMetaParseError> {
        let rust_binaries = summary
            .rust_binaries
            .into_values()
            .map(|bin| RustTestBinary {
                name: bin.binary_name,
                path: bin.binary_path,
                package_id: bin.package_id,
                kind: bin.kind,
                id: bin.binary_id,
                build_platform: bin.build_platform,
            })
            .collect();
        Ok(Self {
            rust_build_meta: RustBuildMeta::from_summary(summary.rust_build_meta)?,
            rust_binaries,
        })
    }

    /// Outputs this list to the given writer.
    pub fn write(
        &self,
        output_format: OutputFormat,
        writer: &mut dyn WriteStr,
        colorize: bool,
    ) -> Result<(), WriteTestListError> {
        match output_format {
            OutputFormat::Human { verbose } => self
                .write_human(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Oneline { verbose } => self
                .write_oneline(writer, verbose, colorize)
                .map_err(WriteTestListError::Io),
            OutputFormat::Serializable(format) => format.to_writer(&self.to_summary(), writer),
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
                    kind: bin.kind.clone(),
                    binary_path: bin.path.clone(),
                    binary_id: bin.id.clone(),
                    build_platform: bin.build_platform,
                };
                (bin.id.clone(), summary)
            })
            .collect();

        BinaryListSummary {
            rust_build_meta: self.rust_build_meta.to_summary(),
            rust_binaries,
        }
    }

    fn write_human(
        &self,
        writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
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

    fn write_oneline(
        &self,
        writer: &mut dyn WriteStr,
        verbose: bool,
        colorize: bool,
    ) -> io::Result<()> {
        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }
        for bin in &self.rust_binaries {
            write!(writer, "{}", bin.id.style(styles.binary_id))?;
            if verbose {
                write!(
                    writer,
                    " [{}{}] [{}{}]",
                    "bin: ".style(styles.field),
                    bin.path,
                    "build platform: ".style(styles.field),
                    bin.build_platform,
                )?;
            }
            writeln!(writer)?;
        }
        Ok(())
    }

    /// Outputs this list as a string with the given format.
    pub fn to_string(&self, output_format: OutputFormat) -> Result<String, WriteTestListError> {
        let mut s = String::with_capacity(1024);
        self.write(output_format, &mut s, false)?;
        Ok(s)
    }
}

#[derive(Debug)]
struct BinaryListBuildState<'g> {
    graph: &'g PackageGraph,
    rust_binaries: Vec<RustTestBinary>,
    rust_build_meta: RustBuildMeta<BinaryListState>,
    alt_target_dir: Option<Utf8PathBuf>,
}

impl<'g> BinaryListBuildState<'g> {
    fn new(graph: &'g PackageGraph, build_platforms: BuildPlatforms) -> Self {
        let rust_target_dir = graph.workspace().target_directory().to_path_buf();
        // For testing only, not part of the public API.
        let alt_target_dir = std::env::var("__NEXTEST_ALT_TARGET_DIR")
            .ok()
            .map(Utf8PathBuf::from);

        Self {
            graph,
            rust_binaries: vec![],
            rust_build_meta: RustBuildMeta::new(rust_target_dir, build_platforms),
            alt_target_dir,
        }
    }

    fn process_message(&mut self, message: Message) -> Result<(), FromMessagesError> {
        match message {
            Message::CompilerArtifact(artifact) => {
                self.process_artifact(artifact)?;
            }
            Message::BuildScriptExecuted(build_script) => {
                self.process_build_script(build_script)?;
            }
            _ => {
                // Ignore all other messages.
            }
        }

        Ok(())
    }

    fn process_artifact(&mut self, artifact: Artifact) -> Result<(), FromMessagesError> {
        if let Some(path) = artifact.executable {
            self.detect_base_output_dir(&path);

            if artifact.profile.test {
                let package_id = artifact.package_id.repr;

                // Look up the executable by package ID.

                let name = artifact.target.name;

                let package = self
                    .graph
                    .metadata(&guppy::PackageId::new(package_id.clone()))
                    .map_err(FromMessagesError::PackageGraph)?;

                let kind = artifact.target.kind;
                if kind.is_empty() {
                    return Err(FromMessagesError::MissingTargetKind {
                        package_name: package.name().to_owned(),
                        binary_name: name.clone(),
                    });
                }

                let (computed_kind, platform) = if kind.iter().any(|k| {
                    // https://doc.rust-lang.org/nightly/cargo/reference/cargo-targets.html#the-crate-type-field
                    matches!(
                        k,
                        TargetKind::Lib
                            | TargetKind::RLib
                            | TargetKind::DyLib
                            | TargetKind::CDyLib
                            | TargetKind::StaticLib
                    )
                }) {
                    (RustTestBinaryKind::LIB, BuildPlatform::Target)
                } else if let Some(TargetKind::ProcMacro) = kind.first() {
                    (RustTestBinaryKind::PROC_MACRO, BuildPlatform::Host)
                } else {
                    // Non-lib kinds should always have just one element. Grab the first one.
                    (
                        RustTestBinaryKind::new(
                            kind.into_iter()
                                .next()
                                .expect("already checked that kind is non-empty")
                                .to_string(),
                        ),
                        BuildPlatform::Target,
                    )
                };

                // Construct the binary ID from the package and build target.
                let id = RustBinaryId::from_parts(package.name(), &computed_kind, &name);

                self.rust_binaries.push(RustTestBinary {
                    path,
                    package_id,
                    kind: computed_kind,
                    name,
                    id,
                    build_platform: platform,
                });
            } else if artifact
                .target
                .kind
                .iter()
                .any(|x| matches!(x, TargetKind::Bin))
            {
                // This is a non-test binary -- add it to the map.
                // Error case here implies that the returned path wasn't in the target directory -- ignore it
                // since it shouldn't happen in normal use.
                if let Ok(rel_path) = path.strip_prefix(&self.rust_build_meta.target_directory) {
                    let non_test_binary = RustNonTestBinarySummary {
                        name: artifact.target.name,
                        kind: RustNonTestBinaryKind::BIN_EXE,
                        path: convert_rel_path_to_forward_slash(rel_path),
                    };

                    self.rust_build_meta
                        .non_test_binaries
                        .entry(artifact.package_id.repr)
                        .or_default()
                        .insert(non_test_binary);
                };
            }
        } else if artifact
            .target
            .kind
            .iter()
            .any(|x| matches!(x, TargetKind::DyLib | TargetKind::CDyLib))
        {
            // Also look for and grab dynamic libraries to store in archives.
            for filename in artifact.filenames {
                if let Ok(rel_path) = filename.strip_prefix(&self.rust_build_meta.target_directory)
                {
                    let non_test_binary = RustNonTestBinarySummary {
                        name: artifact.target.name.clone(),
                        kind: RustNonTestBinaryKind::DYLIB,
                        path: convert_rel_path_to_forward_slash(rel_path),
                    };
                    self.rust_build_meta
                        .non_test_binaries
                        .entry(artifact.package_id.repr.clone())
                        .or_default()
                        .insert(non_test_binary);
                }
            }
        }

        Ok(())
    }

    /// Look for paths that contain "deps" in their second-to-last component,
    /// and are descendants of the target directory.
    /// The paths without "deps" are base output directories.
    ///
    /// e.g. path/to/repo/target/debug/deps/test-binary => add "debug"
    /// to base output dirs.
    ///
    /// Note that test binaries are always present in "deps", so we should always
    /// have a match.
    ///
    /// The `Option` in the return value is to let ? work.
    fn detect_base_output_dir(&mut self, artifact_path: &Utf8Path) -> Option<()> {
        // Artifact paths must be relative to the target directory.
        let rel_path = artifact_path
            .strip_prefix(&self.rust_build_meta.target_directory)
            .ok()?;
        let parent = rel_path.parent()?;
        if parent.file_name() == Some("deps") {
            let base = parent.parent()?;
            if !self.rust_build_meta.base_output_directories.contains(base) {
                self.rust_build_meta
                    .base_output_directories
                    .insert(convert_rel_path_to_forward_slash(base));
            }
        }
        Some(())
    }

    fn process_build_script(&mut self, build_script: BuildScript) -> Result<(), FromMessagesError> {
        for path in build_script.linked_paths {
            self.detect_linked_path(&build_script.package_id, &path);
        }

        // We only care about build scripts for workspace packages.
        let package_id = guppy::PackageId::new(build_script.package_id.repr);
        let in_workspace = self.graph.metadata(&package_id).map_or_else(
            |_| {
                // Warn about processing a package that isn't in the package graph.
                warn!(
                    target: "nextest-runner::list",
                    "warning: saw package ID `{}` which wasn't produced by cargo metadata",
                    package_id
                );
                false
            },
            |p| p.in_workspace(),
        );
        if in_workspace {
            // Ignore this build script if it's not in the target directory.
            if let Ok(rel_out_dir) = build_script
                .out_dir
                .strip_prefix(&self.rust_build_meta.target_directory)
            {
                self.rust_build_meta.build_script_out_dirs.insert(
                    package_id.repr().to_owned(),
                    convert_rel_path_to_forward_slash(rel_out_dir),
                );
            }
        }

        Ok(())
    }

    /// The `Option` in the return value is to let ? work.
    fn detect_linked_path(&mut self, package_id: &PackageId, path: &Utf8Path) -> Option<()> {
        // Remove anything up to the first "=" (e.g. "native=").
        let actual_path = match path.as_str().split_once('=') {
            Some((_, p)) => p.into(),
            None => path,
        };

        let rel_path = match actual_path.strip_prefix(&self.rust_build_meta.target_directory) {
            Ok(rel) => rel,
            Err(_) => {
                // For a seeded build (like in our test suite), Cargo will
                // return:
                //
                // * the new path if the linked path exists
                // * the original path if the linked path does not exist
                //
                // Linked paths not existing is not an ordinary condition, but
                // we want to test it within nextest. We filter out paths if
                // they're not a subdirectory of the target directory. With
                // __NEXTEST_ALT_TARGET_DIR, we can simulate that for an
                // alternate target directory.
                if let Some(alt_target_dir) = &self.alt_target_dir {
                    actual_path.strip_prefix(alt_target_dir).ok()?
                } else {
                    return None;
                }
            }
        };

        self.rust_build_meta
            .linked_paths
            .entry(convert_rel_path_to_forward_slash(rel_path))
            .or_default()
            .insert(package_id.repr.clone());

        Some(())
    }

    fn finish(mut self) -> BinaryList {
        self.rust_binaries.sort_by(|b1, b2| b1.id.cmp(&b2.id));

        // Clean out any build script output directories for which there's no corresponding binary.
        let relevant_package_ids = self
            .rust_binaries
            .iter()
            .map(|bin| bin.package_id.clone())
            .collect::<HashSet<_>>();

        self.rust_build_meta
            .build_script_out_dirs
            .retain(|package_id, _| relevant_package_ids.contains(package_id));

        BinaryList {
            rust_build_meta: self.rust_build_meta,
            rust_binaries: self.rust_binaries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cargo_config::{TargetDefinitionLocation, TargetTriple, TargetTripleSource},
        list::SerializableFormat,
        platform::{HostPlatform, PlatformLibdir, TargetPlatform},
    };
    use indoc::indoc;
    use maplit::btreeset;
    use nextest_metadata::PlatformLibdirUnavailable;
    use pretty_assertions::assert_eq;
    use target_spec::{Platform, TargetFeatures};

    #[test]
    fn test_parse_binary_list() {
        let fake_bin_test = RustTestBinary {
            id: "fake-package::bin/fake-binary".into(),
            path: "/fake/binary".into(),
            package_id: "fake-package 0.1.0 (path+file:///Users/fakeuser/project/fake-package)"
                .to_owned(),
            kind: RustTestBinaryKind::LIB,
            name: "fake-binary".to_owned(),
            build_platform: BuildPlatform::Target,
        };
        let fake_macro_test = RustTestBinary {
            id: "fake-macro::proc-macro/fake-macro".into(),
            path: "/fake/macro".into(),
            package_id: "fake-macro 0.1.0 (path+file:///Users/fakeuser/project/fake-macro)"
                .to_owned(),
            kind: RustTestBinaryKind::PROC_MACRO,
            name: "fake-macro".to_owned(),
            build_platform: BuildPlatform::Host,
        };

        let fake_triple = TargetTriple {
            platform: Platform::new("aarch64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap(),
            source: TargetTripleSource::CliOption,
            location: TargetDefinitionLocation::Builtin,
        };
        let fake_host_libdir = "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib";
        let build_platforms = BuildPlatforms {
            host: HostPlatform {
                platform: TargetTriple::x86_64_unknown_linux_gnu().platform,
                libdir: PlatformLibdir::Available(Utf8PathBuf::from(fake_host_libdir)),
            },
            target: Some(TargetPlatform {
                triple: fake_triple,
                // Test out the error case for unavailable libdirs.
                libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR),
            }),
        };

        let mut rust_build_meta = RustBuildMeta::new("/fake/target", build_platforms);
        rust_build_meta
            .base_output_directories
            .insert("my-profile".into());
        rust_build_meta.non_test_binaries.insert(
            "my-package-id".into(),
            btreeset! {
                RustNonTestBinarySummary {
                    name: "my-name".into(),
                    kind: RustNonTestBinaryKind::BIN_EXE,
                    path: "my-profile/my-name".into(),
                },
                RustNonTestBinarySummary {
                    name: "your-name".into(),
                    kind: RustNonTestBinaryKind::DYLIB,
                    path: "my-profile/your-name.dll".into(),
                },
                RustNonTestBinarySummary {
                    name: "your-name".into(),
                    kind: RustNonTestBinaryKind::DYLIB,
                    path: "my-profile/your-name.exp".into(),
                },
            },
        );

        let binary_list = BinaryList {
            rust_build_meta,
            rust_binaries: vec![fake_bin_test, fake_macro_test],
        };

        // Check that the expected outputs are valid.
        static EXPECTED_HUMAN: &str = indoc! {"
        fake-package::bin/fake-binary
        fake-macro::proc-macro/fake-macro
        "};
        static EXPECTED_HUMAN_VERBOSE: &str = indoc! {r"
        fake-package::bin/fake-binary:
          bin: /fake/binary
          build platform: target
        fake-macro::proc-macro/fake-macro:
          bin: /fake/macro
          build platform: host
        "};
        static EXPECTED_JSON_PRETTY: &str = indoc! {r#"
        {
          "rust-build-meta": {
            "target-directory": "/fake/target",
            "base-output-directories": [
              "my-profile"
            ],
            "non-test-binaries": {
              "my-package-id": [
                {
                  "name": "my-name",
                  "kind": "bin-exe",
                  "path": "my-profile/my-name"
                },
                {
                  "name": "your-name",
                  "kind": "dylib",
                  "path": "my-profile/your-name.dll"
                },
                {
                  "name": "your-name",
                  "kind": "dylib",
                  "path": "my-profile/your-name.exp"
                }
              ]
            },
            "build-script-out-dirs": {},
            "linked-paths": [],
            "platforms": {
              "host": {
                "platform": {
                  "triple": "x86_64-unknown-linux-gnu",
                  "target-features": "unknown"
                },
                "libdir": {
                  "status": "available",
                  "path": "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib"
                }
              },
              "targets": [
                {
                  "platform": {
                    "triple": "aarch64-unknown-linux-gnu",
                    "target-features": "unknown"
                  },
                  "libdir": {
                    "status": "unavailable",
                    "reason": "rustc-output-error"
                  }
                }
              ]
            },
            "target-platforms": [
              {
                "triple": "aarch64-unknown-linux-gnu",
                "target-features": "unknown"
              }
            ],
            "target-platform": "aarch64-unknown-linux-gnu"
          },
          "rust-binaries": {
            "fake-macro::proc-macro/fake-macro": {
              "binary-id": "fake-macro::proc-macro/fake-macro",
              "binary-name": "fake-macro",
              "package-id": "fake-macro 0.1.0 (path+file:///Users/fakeuser/project/fake-macro)",
              "kind": "proc-macro",
              "binary-path": "/fake/macro",
              "build-platform": "host"
            },
            "fake-package::bin/fake-binary": {
              "binary-id": "fake-package::bin/fake-binary",
              "binary-name": "fake-binary",
              "package-id": "fake-package 0.1.0 (path+file:///Users/fakeuser/project/fake-package)",
              "kind": "lib",
              "binary-path": "/fake/binary",
              "build-platform": "target"
            }
          }
        }"#};
        // Non-verbose oneline is the same as non-verbose human.
        static EXPECTED_ONELINE: &str = indoc! {"
            fake-package::bin/fake-binary
            fake-macro::proc-macro/fake-macro
        "};
        static EXPECTED_ONELINE_VERBOSE: &str = indoc! {r"
            fake-package::bin/fake-binary [bin: /fake/binary] [build platform: target]
            fake-macro::proc-macro/fake-macro [bin: /fake/macro] [build platform: host]
        "};

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
        assert_eq!(
            binary_list
                .to_string(OutputFormat::Oneline { verbose: false })
                .expect("oneline succeeded"),
            EXPECTED_ONELINE
        );
        assert_eq!(
            binary_list
                .to_string(OutputFormat::Oneline { verbose: true })
                .expect("oneline verbose succeeded"),
            EXPECTED_ONELINE_VERBOSE
        );
    }
}
