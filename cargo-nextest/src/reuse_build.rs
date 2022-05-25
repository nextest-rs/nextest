// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::io::Write;

use crate::{output::OutputContext, ExpectedError, OutputWriter};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use guppy::graph::PackageGraph;
use nextest_runner::{
    errors::PathMapperConstructKind,
    reuse_build::{
        ArchiveReporter, ExtractDestination, MetadataWithRemap, PathMapper, ReuseBuildInfo,
    },
};
use owo_colors::Stream;

#[derive(Debug, Default, Args)]
#[clap(
    next_help_heading = "REUSE BUILD OPTIONS",
    // These groups define data sources for various aspects of reuse-build inputs
    group = clap::ArgGroup::new("cargo-metadata-sources"),
    group = clap::ArgGroup::new("binaries-metadata-sources"),
    group = clap::ArgGroup::new("target-dir-remap-sources"),
)]
pub(crate) struct ReuseBuildOpts {
    /// Path to nextest archive (.tar.zst)
    #[clap(
        long,
        groups = &["cargo-metadata-sources", "binaries-metadata-sources", "target-dir-remap-sources"],
        conflicts_with_all = &["cargo-opts", "binaries-metadata", "cargo-metadata"],
        value_name = "PATH",
    )]
    pub(crate) archive: Option<Utf8PathBuf>,

    /// Destination directory to extract archive to [default: temporary directory]
    #[clap(
        long,
        conflicts_with = "cargo-opts",
        requires = "archive",
        value_name = "DIR"
    )]
    pub(crate) extract_to: Option<Utf8PathBuf>,

    /// Overwrite files in destination directory while extracting archive
    #[clap(long, conflicts_with = "cargo-opts", requires_all = &["archive", "extract-to"])]
    pub(crate) extract_overwrite: bool,

    /// Persist temporary directory destination is extracted to
    #[clap(long, conflicts_with_all = &["cargo-opts", "extract-to"], requires = "archive")]
    pub(crate) persist_extract_dir: bool,

    /// Path to cargo metadata JSON
    #[clap(
        long,
        group = "cargo-metadata-sources",
        conflicts_with = "manifest-path",
        value_name = "PATH"
    )]
    pub(crate) cargo_metadata: Option<Utf8PathBuf>,

    /// Remapping for the workspace root
    #[clap(long, requires = "cargo-metadata-sources", value_name = "PATH")]
    pub(crate) workspace_remap: Option<Utf8PathBuf>,

    /// Path to binaries-metadata JSON
    #[clap(
        long,
        group = "binaries-metadata-sources",
        conflicts_with = "cargo-opts",
        value_name = "PATH"
    )]
    pub(crate) binaries_metadata: Option<Utf8PathBuf>,

    /// Remapping for the target directory
    #[clap(
        long,
        group = "target-dir-remap-sources",
        // Note: --target-dir-remap is incompatible with --archive, hence this requires
        // binaries-metadata and not binaries-metadata-sources.
        requires = "binaries-metadata",
        value_name = "PATH"
    )]
    pub(crate) target_dir_remap: Option<Utf8PathBuf>,
}

impl ReuseBuildOpts {
    const EXPERIMENTAL_ENV: &'static str = "NEXTEST_EXPERIMENTAL_REUSE_BUILD";

    // (_output is not used, but must be passed in to ensure that the output is properly initialized
    // before calling this method)
    pub(crate) fn check_experimental(&self, _output: OutputContext) {
        if std::env::var(Self::EXPERIMENTAL_ENV).is_ok() {
            log::warn!("build reuse is no longer experimental: NEXTEST_EXPERIMENTAL_REUSE_BUILD does not need to be set");
        }
    }

    pub(crate) fn process(
        &self,
        output: OutputContext,
        output_writer: &mut OutputWriter,
    ) -> Result<ReuseBuildInfo, ExpectedError> {
        if let Some(archive_file) = &self.archive {
            // Process this archive.
            let dest = match &self.extract_to {
                Some(dir) => ExtractDestination::Destination {
                    dir: dir.clone(),
                    overwrite: self.extract_overwrite,
                },
                None => ExtractDestination::TempDir {
                    persist: self.persist_extract_dir,
                },
            };

            let mut reporter = ArchiveReporter::new(output.verbose);
            if output.color.should_colorize(Stream::Stderr) {
                reporter.colorize();
            }

            let mut writer = output_writer.stderr_writer();
            return ReuseBuildInfo::extract_archive(
                archive_file,
                dest,
                |event| {
                    reporter.report_event(event, &mut writer)?;
                    writer.flush()
                },
                self.workspace_remap.as_deref(),
            )
            .map_err(|err| ExpectedError::ArchiveExtractError {
                archive_file: archive_file.clone(),
                err,
            });
        }

        let cargo_metadata = self.cargo_metadata.as_ref().map(|path| MetadataWithRemap {
            metadata: path.clone().into(),
            remap: self.workspace_remap.clone(),
        });

        let binaries_metadata = self
            .binaries_metadata
            .as_ref()
            .map(|path| MetadataWithRemap {
                metadata: path.clone().into(),
                remap: self.target_dir_remap.clone(),
            });

        Ok(ReuseBuildInfo::new(cargo_metadata, binaries_metadata))
    }
}

pub(crate) fn make_path_mapper(
    info: &ReuseBuildInfo,
    graph: &PackageGraph,
    orig_target_dir: &Utf8Path,
) -> Result<PathMapper, ExpectedError> {
    PathMapper::new(
        graph.workspace().root(),
        info.workspace_remap(),
        orig_target_dir,
        info.target_dir_remap(),
    )
    .map_err(|err| {
        let arg_name = match err.kind() {
            PathMapperConstructKind::WorkspaceRoot => "workspace-remap",
            PathMapperConstructKind::TargetDir => "target-dir-remap",
        };
        ExpectedError::PathMapperConstructError { arg_name, err }
    })
}
