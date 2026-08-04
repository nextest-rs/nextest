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
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    io,
};
use tracing::{debug, warn};

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
        let mut builder = BinaryListBuilder::new(graph, build_platforms);

        for message in Message::parse_stream(reader) {
            let message = message.map_err(FromMessagesError::ReadMessages)?;
            builder.process_message(message)?;
        }

        Ok(builder.finish())
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
        BinaryListSummary {
            rust_build_meta: self.rust_build_meta.to_summary(),
            rust_binaries: self.binary_summaries(),
        }
    }

    /// Produces a summary suitable for archive metadata.
    ///
    /// * `build_directory` is omitted so it defaults to `target_directory` on
    ///   extraction.
    /// * Binary paths under `build_directory` are remapped to `target_directory`
    ///   so the `PathMapper` can remap them correctly on extraction.
    pub(crate) fn to_archive_summary(&self) -> BinaryListSummary {
        let target_dir = &self.rust_build_meta.target_directory;
        let build_directory = &self.rust_build_meta.build_directory;

        let rust_binaries = self
            .rust_binaries
            .iter()
            .map(|bin| {
                // In the archive, test binaries are stored under target/.
                // Remap paths from build_directory to target_directory so the PathMapper
                // can relocate them on extraction.
                let binary_path = target_dir.join(
                    bin.path
                        .strip_prefix(build_directory)
                        .expect("test binary paths must be within the build directory"),
                );
                let summary = RustTestBinarySummary {
                    binary_name: bin.name.clone(),
                    package_id: bin.package_id.clone(),
                    kind: bin.kind.clone(),
                    binary_path,
                    binary_id: bin.id.clone(),
                    build_platform: bin.build_platform,
                };
                (bin.id.clone(), summary)
            })
            .collect();

        BinaryListSummary {
            rust_build_meta: self.rust_build_meta.to_archive_summary(),
            rust_binaries,
        }
    }

    fn binary_summaries(&self) -> BTreeMap<RustBinaryId, RustTestBinarySummary> {
        self.rust_binaries
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
            .collect()
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

/// Incrementally builds a [`BinaryList`] from Cargo messages.
#[derive(Debug)]
pub struct BinaryListBuilder<'g> {
    state: BinaryListBuildState<'g>,
}

impl<'g> BinaryListBuilder<'g> {
    /// Creates a new builder for Cargo messages.
    pub fn new(graph: &'g PackageGraph, build_platforms: BuildPlatforms) -> Self {
        Self {
            state: BinaryListBuildState::new(graph, build_platforms),
        }
    }

    /// Processes a single Cargo message.
    pub fn process_message(&mut self, message: Message) -> Result<(), FromMessagesError> {
        self.state.process_message(message)
    }

    /// Processes a single line of Cargo output.
    ///
    /// This uses the same single-line parsing behavior as
    /// [`cargo_metadata::Message::parse_stream`].
    pub fn process_message_line(&mut self, line: &str) -> Result<(), FromMessagesError> {
        self.process_message(parse_message_line(line))
    }

    /// Finishes building the binary list.
    pub fn finish(self) -> BinaryList {
        self.state.finish()
    }
}

// Adapted from cargo_metadata::MessageIter::next (cargo_metadata 0.23.1).
fn parse_message_line(line: &str) -> Message {
    let mut deserializer = serde_json::Deserializer::from_str(line);
    deserializer.disable_recursion_limit();
    Message::deserialize(&mut deserializer).unwrap_or_else(|_| Message::TextLine(line.to_owned()))
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
        // Use the build directory if on Cargo 1.91 or newer. Fall back to
        // the target directory for older Cargo versions.
        let build_directory = graph
            .workspace()
            .build_directory()
            .unwrap_or_else(|| graph.workspace().target_directory())
            .to_path_buf();
        // For testing only, not part of the public API.
        let alt_target_dir = std::env::var("__NEXTEST_ALT_TARGET_DIR")
            .ok()
            .map(Utf8PathBuf::from);

        Self {
            graph,
            rust_binaries: vec![],
            rust_build_meta: RustBuildMeta::new(rust_target_dir, build_directory, build_platforms),
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
                        build_platform: self.non_test_build_platform(&path),
                    };

                    self.rust_build_meta.non_test_binaries.insert(
                        guppy::PackageId::new(artifact.package_id.repr),
                        non_test_binary,
                    );
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
                        build_platform: self.non_test_build_platform(&filename),
                    };
                    self.rust_build_meta.non_test_binaries.insert(
                        guppy::PackageId::new(artifact.package_id.repr.clone()),
                        non_test_binary,
                    );
                }
            }
        }

        Ok(())
    }

    /// Records the base output directory an artifact was built into.
    ///
    /// A base output directory is the `[<triple>/]<profile>` prefix of an
    /// artifact's path, relative to the build directory. Only these shapes are
    /// recognized:
    ///
    /// * the legacy layout: `debug/deps/test-binary`
    /// * the legacy layout, for `[[example]]` targets: `debug/examples/test-binary`
    /// * the build-dir layout v2: `debug/build/my-package/f3694e28990a9310/out/test-binary`
    ///
    /// In each case, the base output directory is `debug`. For each one, nextest
    /// adds two directories to the dynamic library path:
    ///
    /// * the Cargo artifact directory, where Cargo uplifts final artifacts to
    ///   (the base output directory under the *target* directory)
    /// * the legacy `deps` directory (under the *build* directory)
    ///
    /// The `Option` in the return value is to let ? work.
    fn detect_base_output_dir(&mut self, artifact_path: &Utf8Path) -> Option<()> {
        // Artifact paths must be relative to the build directory (which
        // equals the target directory unless Cargo's build.build-dir is
        // configured).
        //
        // Unlike `artifact_build_platform`, which picks whichever is the
        // innermost of the build and target directories, this resolves against
        // the build directory and nothing else, because its result goes into
        // `base_output_directories`, which is relative to the build directory.
        let rel_path = match artifact_path.strip_prefix(&self.rust_build_meta.build_directory) {
            Ok(rel) => rel,
            Err(_) => {
                debug!(
                    target: "nextest-runner::list",
                    "artifact path `{}` is not within the build directory `{}`, \
                     skipping base output directory detection",
                    artifact_path, self.rust_build_meta.build_directory,
                );
                return None;
            }
        };

        let base = base_output_dir(rel_path)?;
        if !self.rust_build_meta.base_output_directories.contains(base) {
            self.rust_build_meta
                .base_output_directories
                .insert(convert_rel_path_to_forward_slash(base));
        }
        Some(())
    }

    fn non_test_build_platform(&self, artifact_path: &Utf8Path) -> Option<BuildPlatform> {
        artifact_build_platform(
            artifact_path,
            &self.rust_build_meta.target_directory,
            &self.rust_build_meta.build_directory,
            self.rust_build_meta
                .build_platforms
                .target
                .as_ref()
                .map(|target| target.triple.platform.triple_str()),
        )
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
            // Build script out_dirs are relative to the build directory.
            match build_script
                .out_dir
                .strip_prefix(&self.rust_build_meta.build_directory)
            {
                Ok(rel_out_dir) => {
                    self.rust_build_meta.build_script_out_dirs.insert(
                        package_id.repr().to_owned(),
                        convert_rel_path_to_forward_slash(rel_out_dir),
                    );
                }
                Err(_) => {
                    debug!(
                        target: "nextest-runner::list",
                        "build script out_dir `{}` for package `{}` is not within \
                         the build directory `{}`, skipping",
                        build_script.out_dir, package_id,
                        self.rust_build_meta.build_directory,
                    );
                }
            }

            // Capture build script environment variables from the structured
            // cargo message, avoiding the need to parse the raw output file.
            if !build_script.env.is_empty() {
                self.rust_build_meta
                    .build_script_info
                    .get_or_insert_with(BTreeMap::new)
                    .entry(package_id.repr().to_owned())
                    .or_default()
                    .envs = build_script.env.into_iter().collect();
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

        let rel_path = match actual_path.strip_prefix(&self.rust_build_meta.build_directory) {
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
        if let Some(info) = &mut self.rust_build_meta.build_script_info {
            info.retain(|package_id, _| relevant_package_ids.contains(package_id));
        }

        // All test binaries live inside a base output dir under both Cargo
        // layouts, so an empty set suggests that we didn't recognize the
        // layout. It's worth warning about this.
        if !self.rust_binaries.is_empty() && self.rust_build_meta.base_output_directories.is_empty()
        {
            warn!(
                target: "nextest-runner::list",
                "failed to detect any base output directories under the build directory `{}`; \
                 tests that link against dynamic libraries may fail to start. \
                 This usually means Cargo's build directory layout changed -- \
                 please report it at https://github.com/nextest-rs/nextest/issues/new",
                self.rust_build_meta.build_directory,
            );
        }

        BinaryList {
            rust_build_meta: self.rust_build_meta,
            rust_binaries: self.rust_binaries,
        }
    }
}

/// Determines the build platform for a particular artifact.
///
/// This is a heuristic due to Cargo's lack of platform information in build
/// message output. See <https://github.com/rust-lang/cargo/issues/12869> for
/// the Cargo issue.
fn artifact_build_platform(
    artifact_path: &Utf8Path,
    target_directory: &Utf8Path,
    build_directory: &Utf8Path,
    target_triple: Option<&str>,
) -> Option<BuildPlatform> {
    // If we're not cross compiling, we must be building for the target.
    let Some(target_triple) = target_triple else {
        return Some(BuildPlatform::Target);
    };

    // As of this writing (2026-08), callers ensure that the artifact path is
    // under either the target directory or the build directory. But we can
    // reasonably return None here if that assumption doesn't hold.
    let Some(rel_path) = [target_directory, build_directory]
        .into_iter()
        .filter_map(|root| artifact_path.strip_prefix(root).ok())
        .min_by_key(|rel_path| rel_path.as_str().len())
    else {
        debug!(
            target: "nextest-runner::list",
            "artifact path `{}` is under neither the target directory `{}` nor the build \
             directory `{}`, recording its build platform as unknown",
            artifact_path, target_directory, build_directory,
        );
        return None;
    };

    if rel_path.starts_with(target_triple) {
        Some(BuildPlatform::Target)
    } else {
        Some(BuildPlatform::Host)
    }
}

fn base_output_dir(rel_path: &Utf8Path) -> Option<&Utf8Path> {
    let parent = rel_path.parent()?;
    let base = match parent.file_name()? {
        // The legacy layout.
        //
        // Test binaries built from `[[example]]` targets go to `examples`
        // rather than `deps` -- see Cargo's `CompilationFiles::output_dir`.
        // (Under the v2 layout they go to the `out` directory below, like every
        // other compilation unit.)
        "deps" | "examples" => parent.parent()?,
        // The build-dir v2 layout.
        "out" => parent
            .ancestors()
            // There is a subtle point here: we want to restrict this branch to
            // the v2 layout.
            //
            // With the legacy layout, build script out dirs are of the form
            // `<base>/build/<package>-<hash>/out`. `nth(3)` would return
            // `<base>`, whose last component is the profile's directory name.
            // That name can never be `build`, because:
            //
            // * Cargo does not allow profiles to have the name `build`
            // * `profile.<name>.dir-name`, the only potential way to decouple a
            //   profile's directory name from the profile name, is currently
            //   disallowed as of 2026-07.
            //
            // So the filter below discards legacy build script out dirs without
            // also discarding any real base output directory.
            //
            // With the v2 layout, out dirs are of the form
            // `<base>/build/<package>/<hash>/out`. `nth(3)` returns `build`.
            // This is also the shape of build script out dirs under v2, which
            // is harmless: they resolve to the same `<base>`.
            .nth(3)
            .filter(|dir| dir.file_name() == Some("build"))?
            .parent()?,
        _ => return None,
    };

    // A base output dir is always `[<triple>/]<profile>`, so an empty one means
    // the path didn't have the shape we expected.
    (!base.as_str().is_empty()).then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cargo_config::{TargetDefinitionLocation, TargetTriple, TargetTripleSource},
        list::{
            SerializableFormat,
            test_helpers::{PACKAGE_GRAPH_FIXTURE, PACKAGE_METADATA_ID, package_info},
        },
        platform::{HostPlatform, PlatformLibdir, TargetPlatform},
    };
    use indoc::indoc;
    use maplit::btreeset;
    use nextest_metadata::PlatformLibdirUnavailable;
    use pretty_assertions::assert_eq;
    use serde_json::json;
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

        let mut rust_build_meta =
            RustBuildMeta::new("/fake/target", "/fake/target", build_platforms);
        // With a target triple set, a binary built for the target platform
        // lives under `<triple>/<profile>`, otherwise just under `<profile>`.
        rust_build_meta
            .base_output_directories
            .insert("aarch64-unknown-linux-gnu/my-profile".into());
        rust_build_meta
            .base_output_directories
            .insert("my-profile".into());
        for non_test_binary in [
            RustNonTestBinarySummary {
                name: "my-name".into(),
                kind: RustNonTestBinaryKind::BIN_EXE,
                path: "aarch64-unknown-linux-gnu/my-profile/my-name".into(),
                build_platform: Some(BuildPlatform::Target),
            },
            RustNonTestBinarySummary {
                name: "your-name".into(),
                kind: RustNonTestBinaryKind::DYLIB,
                path: "my-profile/your-name.dll".into(),
                build_platform: Some(BuildPlatform::Host),
            },
            RustNonTestBinarySummary {
                name: "your-name".into(),
                kind: RustNonTestBinaryKind::DYLIB,
                path: "my-profile/your-name.exp".into(),
                build_platform: Some(BuildPlatform::Host),
            },
        ] {
            rust_build_meta
                .non_test_binaries
                .insert(guppy::PackageId::new("my-package-id"), non_test_binary);
        }

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
            "build-directory": "/fake/target",
            "base-output-directories": [
              "aarch64-unknown-linux-gnu/my-profile",
              "my-profile"
            ],
            "non-test-binaries": {
              "my-package-id": [
                {
                  "name": "my-name",
                  "kind": "bin-exe",
                  "path": "aarch64-unknown-linux-gnu/my-profile/my-name",
                  "build-platform": "target"
                },
                {
                  "name": "your-name",
                  "kind": "dylib",
                  "path": "my-profile/your-name.dll",
                  "build-platform": "host"
                },
                {
                  "name": "your-name",
                  "kind": "dylib",
                  "path": "my-profile/your-name.exp",
                  "build-platform": "host"
                }
              ]
            },
            "build-script-out-dirs": {},
            "build-script-info": {},
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

    #[test]
    fn test_parse_binary_list_from_message_lines() {
        let build_platforms = BuildPlatforms {
            host: HostPlatform {
                platform: TargetTriple::x86_64_unknown_linux_gnu().platform,
                libdir: PlatformLibdir::Available("/fake/libdir".into()),
            },
            target: None,
        };
        let package = package_info();
        // The fixture sets build_directory separately from target_directory, so
        // an artifact resolved against the wrong root fails to produce a base
        // output directory.
        let artifact_path = PACKAGE_GRAPH_FIXTURE
            .workspace()
            .build_directory()
            .expect("fixture sets build_directory")
            .join("debug/deps/metadata_helper-test");
        let compiler_artifact = artifact_json(
            &package.name,
            &["lib"],
            std::slice::from_ref(&artifact_path),
            Some(&artifact_path),
            TestTarget::Yes,
        );
        let input = format!("this is not JSON\n{}\n\n", compiler_artifact);

        let from_messages = BinaryList::from_messages(
            input.as_bytes(),
            &PACKAGE_GRAPH_FIXTURE,
            build_platforms.clone(),
        )
        .expect("parsing from messages succeeds");

        let mut builder = BinaryListBuilder::new(&PACKAGE_GRAPH_FIXTURE, build_platforms);
        for line in input.lines() {
            builder
                .process_message_line(line)
                .expect("processing line succeeds");
        }
        let from_lines = builder.finish();

        assert_eq!(
            from_lines.rust_build_meta.base_output_directories,
            btreeset! { Utf8PathBuf::from("debug") },
            "base output directory detected from the artifact's executable path"
        );

        assert_eq!(
            from_lines
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeds"),
            from_messages
                .to_string(OutputFormat::Serializable(SerializableFormat::JsonPretty))
                .expect("json-pretty succeeds")
        );
    }

    #[test]
    fn test_artifact_build_platform() {
        static TARGET_DIR: &str = "/w/target";
        static TRIPLE: &str = "aarch64-unknown-linux-gnu";

        struct Case {
            artifact_path: &'static str,
            // None here means Cargo's `build.build-dir` is unset, so it equals
            // the target directory.
            build_directory: Option<&'static str>,
            target_triple: Option<&'static str>,
            expected: Option<BuildPlatform>,
            description: &'static str,
        }

        let cases = [
            Case {
                artifact_path: "/w/target/debug/libfoo.so",
                build_directory: None,
                target_triple: None,
                expected: Some(BuildPlatform::Target),
                description: "with no target platform the host and the target coincide, and \
                    nextest reports target",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "a dylib uplifted into the target platform's artifact directory",
            },
            Case {
                artifact_path: "/w/target/debug/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "the same dylib, built for the host as a build dependency",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/deps/libfoo.rlib",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "an rlib that was never uplifted, legacy layout",
            },
            Case {
                artifact_path: "/w/target/debug/deps/libfoo.rlib",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "the same rlib built for the host, legacy layout",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/build/foo/9e6e6f7b/out/libfoo.rlib",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "an rlib that was never uplifted, build-dir layout v2",
            },
            Case {
                artifact_path: "/w/target/debug/build/foo/9e6e6f7b/out/libfoo.rlib",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "the same rlib built for the host, build-dir layout v2",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: Some("/w/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "a build directory beside the target directory: uplifts are unaffected",
            },
            Case {
                artifact_path: "/w/target/build/aarch64-unknown-linux-gnu/debug/deps/libfoo.rlib",
                build_directory: Some("/w/target/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "a build directory inside the target directory: the build directory is the \
                    innermost root, so the triple is still the first component under it",
            },
            Case {
                artifact_path: "/w/target/build/debug/deps/libfoo.rlib",
                build_directory: Some("/w/target/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "a build directory inside the target directory, host artifact",
            },
            Case {
                artifact_path: "/w/target/build/aarch64-unknown-linux-gnu/debug/build/foo/9e6e6f7b/out/libfoo.rlib",
                build_directory: Some("/w/target/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "the shape Cargo emits under the build-dir v2 layout for \
                    `build.build-dir = target/build` plus --target: nesting and v2 at once. \
                    Verified by hand against a real nightly build",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: Some("/w/target/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "a build directory inside the target directory: uplifts still go to the \
                    target directory, which is the innermost root containing them",
            },
            Case {
                artifact_path: "/w/target/debug/libfoo.so",
                build_directory: Some("/w/target/build"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "a host dylib uplifted alongside a nested build directory. The \
                    nested build directory does not contain it, so dropping the target directory \
                    from the candidate roots would leave nothing to strip",
            },
            Case {
                artifact_path: "/w/target/debug/deps/aarch64-unknown-linux-gnu/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "only the first component is the triple slot; a triple-named \
                    directory deeper in the path means nothing",
            },
            Case {
                artifact_path: "/w/target/debug/libfoo.so",
                build_directory: Some("/w"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "the reverse nesting: `build.build-dir` and `build.target-dir` are \
                    independent, so the target directory can sit inside the build directory. It \
                    is then the innermost root",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: Some("/w"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "the reverse nesting, target artifact: resolving against the outer \
                    build directory would see `target` first and misreport this as host",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu-ilp32/debug/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "a directory whose name merely starts with the triple is not the triple",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: Some("/w/target/aarch"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "root selection is component-wise, so a build directory that is only \
                    a string prefix of the path never wins; a byte-wise prefix test would strip \
                    `/w/target/aarch` here and misread the artifact as host",
            },
            Case {
                artifact_path: "/w/target/my-custom-target/debug/libfoo.so",
                build_directory: None,
                target_triple: Some("my-custom-target"),
                expected: Some(BuildPlatform::Target),
                description: "a custom JSON target's directory is its file stem, which is also its triple \
                    string in nextest",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Target),
                description: "KNOWN WRONG, pinned so the limitation stays visible: a custom \
                    profile named exactly after the triple puts host uplifts at \
                    `<profile>/libfoo.so` under the target directory, and the profile component \
                    occupies the `<triple>` slot, so a host artifact reads as target. \
                    Self-inflicted",
            },
            Case {
                artifact_path: "/w/target/aarch64-unknown-linux-gnu/debug/libfoo.so",
                build_directory: Some("/w/target/aarch64-unknown-linux-gnu"),
                target_triple: Some(TRIPLE),
                expected: Some(BuildPlatform::Host),
                description: "KNOWN WRONG, pinned so the limitation stays visible: a build \
                    directory nested in the target directory and named after the triple becomes \
                    the innermost root and swallows the triple component, so an uplifted target \
                    artifact reads as host. Self-inflicted, like the triple-named custom profile \
                    above",
            },
            Case {
                artifact_path: "/elsewhere/debug/libfoo.so",
                build_directory: None,
                target_triple: Some(TRIPLE),
                expected: None,
                description: "a path under neither root has no `<triple>` slot to read, so the \
                    platform is unknown",
            },
        ];

        for case in cases {
            let build_directory = case.build_directory.unwrap_or(TARGET_DIR);
            assert_eq!(
                artifact_build_platform(
                    Utf8Path::new(case.artifact_path),
                    Utf8Path::new(TARGET_DIR),
                    Utf8Path::new(build_directory),
                    case.target_triple,
                ),
                case.expected,
                "{}: build platform for {} (build directory {build_directory})",
                case.description,
                case.artifact_path,
            );
        }
    }

    fn cross_build_platforms() -> BuildPlatforms {
        let target_triple = TargetTriple {
            platform: Platform::new("aarch64-unknown-linux-gnu", TargetFeatures::Unknown)
                .expect("aarch64-unknown-linux-gnu is a builtin triple"),
            source: TargetTripleSource::CliOption,
            location: TargetDefinitionLocation::Builtin,
        };
        BuildPlatforms {
            host: HostPlatform {
                platform: TargetTriple::x86_64_unknown_linux_gnu().platform,
                libdir: PlatformLibdir::Available("/fake/host/libdir".into()),
            },
            target: Some(TargetPlatform::new(
                target_triple,
                PlatformLibdir::Available("/fake/target/libdir".into()),
            )),
        }
    }

    #[test]
    fn test_non_test_build_platform_resolves_against_the_build_directory() {
        let mut state = BinaryListBuildState::new(&PACKAGE_GRAPH_FIXTURE, cross_build_platforms());
        state.rust_build_meta.target_directory = "/w/target".into();
        state.rust_build_meta.build_directory = "/w/target/build".into();

        assert_eq!(
            state.non_test_build_platform(Utf8Path::new(
                "/w/target/build/aarch64-unknown-linux-gnu/debug/deps/libfoo.rlib"
            )),
            Some(BuildPlatform::Target),
            "the nested build directory is the innermost root, so the triple is the first \
             component under it; resolving against the target directory instead would see \
             `build` first and misreport this as host"
        );
    }

    #[test]
    fn test_non_test_binaries_record_the_build_platform() {
        let build_platforms = cross_build_platforms();

        let workspace = PACKAGE_GRAPH_FIXTURE.workspace();
        let target_dir = workspace.target_directory();
        let build_dir = workspace
            .build_directory()
            .expect("fixture sets build_directory");

        // This simulates `cargo test --no-run --target
        // aarch64-unknown-linux-gnu` for a crate with `crate-type = ["dylib",
        // "rlib"]` reachable through both a build dependency (i.e. host) and a
        // regular dependency (target). Cargo compiles such libraries twice, and
        // uplifts each build's dylib into the build's artifact directory, but
        // leaves the rlibs in the build directory.
        let input = [
            dylib_artifact_json(
                "shared",
                &[
                    target_dir.join("debug/libshared.so"),
                    build_dir.join("debug/deps/libshared.rlib"),
                ],
            ),
            dylib_artifact_json(
                "shared",
                &[
                    target_dir.join("aarch64-unknown-linux-gnu/debug/libshared.so"),
                    build_dir.join("aarch64-unknown-linux-gnu/debug/deps/libshared.rlib"),
                ],
            ),
            bin_artifact_json(
                "mainbin",
                &target_dir.join("aarch64-unknown-linux-gnu/debug/mainbin"),
            ),
        ]
        .map(|artifact| artifact.to_string())
        .join("\n");

        let binary_list =
            BinaryList::from_messages(input.as_bytes(), &PACKAGE_GRAPH_FIXTURE, build_platforms)
                .expect("parsing from messages succeeds");

        assert_eq!(
            binary_list.rust_build_meta.non_test_binaries.to_summary(),
            BTreeMap::from([(
                PACKAGE_METADATA_ID.to_owned(),
                btreeset! {
                    RustNonTestBinarySummary {
                        name: "mainbin".to_owned(),
                        kind: RustNonTestBinaryKind::BIN_EXE,
                        path: "aarch64-unknown-linux-gnu/debug/mainbin".into(),
                        build_platform: Some(BuildPlatform::Target),
                    },
                    RustNonTestBinarySummary {
                        name: "shared".to_owned(),
                        kind: RustNonTestBinaryKind::DYLIB,
                        path: "aarch64-unknown-linux-gnu/debug/libshared.so".into(),
                        build_platform: Some(BuildPlatform::Target),
                    },
                    RustNonTestBinarySummary {
                        name: "shared".to_owned(),
                        kind: RustNonTestBinaryKind::DYLIB,
                        path: "debug/libshared.so".into(),
                        build_platform: Some(BuildPlatform::Host),
                    },
                },
            )]),
            "the two builds of one dylib target are told apart by the triple component of their \
             uplifted paths; the rlibs live in the fixture's build directory, which is not under \
             the target directory, so they are not recorded at all"
        );
    }

    #[derive(Clone, Copy)]
    enum TestTarget {
        Yes,
        No,
    }

    impl TestTarget {
        fn as_bool(self) -> bool {
            match self {
                Self::Yes => true,
                Self::No => false,
            }
        }
    }

    fn bin_artifact_json(name: &str, path: &Utf8Path) -> serde_json::Value {
        artifact_json(
            name,
            &["bin"],
            &[path.to_owned()],
            Some(path),
            TestTarget::No,
        )
    }

    fn dylib_artifact_json(name: &str, filenames: &[Utf8PathBuf]) -> serde_json::Value {
        artifact_json(name, &["dylib", "rlib"], filenames, None, TestTarget::No)
    }

    fn artifact_json(
        name: &str,
        kind: &[&str],
        filenames: &[Utf8PathBuf],
        executable: Option<&Utf8Path>,
        test_target: TestTarget,
    ) -> serde_json::Value {
        let package = package_info();
        let src_path = package
            .manifest_path
            .parent()
            .expect("manifest path has a parent")
            .join("src/lib.rs");
        let is_test = test_target.as_bool();

        json!({
            "reason": "compiler-artifact",
            "package_id": PACKAGE_METADATA_ID,
            "manifest_path": package.manifest_path,
            "target": {
                "name": name,
                "kind": kind,
                "crate_types": kind,
                "required-features": [],
                "src_path": src_path,
                "edition": "2021",
                "doctest": is_test,
                "test": is_test,
                "doc": is_test
            },
            "profile": {
                "opt_level": "0",
                "debuginfo": 0,
                "debug_assertions": true,
                "overflow_checks": true,
                "test": is_test
            },
            "features": [],
            "filenames": filenames,
            "executable": executable,
            "fresh": false
        })
    }

    #[test]
    fn test_base_output_dir() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("debug/deps/foo-9e6e6f7b", Some("debug"), "legacy layout"),
            (
                "aarch64-unknown-linux-gnu/debug/deps/foo-9e6e6f7b",
                Some("aarch64-unknown-linux-gnu/debug"),
                "legacy layout with a target triple",
            ),
            (
                "debug/build/metadata-helper/9e6e6f7b/out/foo",
                Some("debug"),
                "v2 layout",
            ),
            (
                "aarch64-unknown-linux-gnu/debug/build/metadata-helper/9e6e6f7b/out/foo",
                Some("aarch64-unknown-linux-gnu/debug"),
                "v2 layout with a target triple",
            ),
            (
                "debug/build/build/9e6e6f7b/out/foo",
                Some("debug"),
                "v2 layout, package named build",
            ),
            (
                "debug/examples/foo-9e6e6f7b",
                Some("debug"),
                "legacy layout, example test binary",
            ),
            (
                "aarch64-unknown-linux-gnu/debug/examples/foo-9e6e6f7b",
                Some("aarch64-unknown-linux-gnu/debug"),
                "legacy layout, example test binary with a target triple",
            ),
            (
                "debug/build/metadata-helper-9e6e6f7b/out/foo",
                None,
                "legacy build script out dir",
            ),
            (
                "aarch64-unknown-linux-gnu/debug/build/metadata-helper-9e6e6f7b/out/foo",
                None,
                "legacy build script out dir with a target triple",
            ),
            ("debug/foo", None, "uplifted into the profile directory"),
            (
                "out/foo",
                None,
                "out directory directly under the build directory",
            ),
            (
                "deps/foo",
                None,
                "deps directory directly under the build directory",
            ),
            (
                "examples/foo",
                None,
                "examples directory directly under the build directory",
            ),
            (
                "build/metadata-helper/9e6e6f7b/out/foo",
                None,
                "v2 build directory directly under the build directory",
            ),
        ];

        for (rel_artifact_path, expected, description) in cases {
            assert_eq!(
                base_output_dir(Utf8Path::new(rel_artifact_path)),
                expected.map(Utf8Path::new),
                "{description}: base output dir for artifact path {rel_artifact_path}"
            );
        }
    }
}
