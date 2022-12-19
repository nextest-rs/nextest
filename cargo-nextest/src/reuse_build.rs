// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputContext, ExpectedError, OutputWriter, Result};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, ValueEnum};
use guppy::graph::PackageGraph;
use nextest_runner::{
    errors::PathMapperConstructKind,
    reuse_build::{
        ArchiveFormat, ArchiveReporter, ExtractDestination, MetadataWithRemap, PathMapper,
        ReuseBuildInfo,
    },
};
use std::io::Write;

#[derive(Debug, Default, Args)]
#[command(
    next_help_heading = "Reuse build options",
    // These groups define data sources for various aspects of reuse-build inputs
    group = clap::ArgGroup::new("cargo-metadata-sources"),
    group = clap::ArgGroup::new("binaries-metadata-sources"),
    group = clap::ArgGroup::new("target-dir-remap-sources"),
)]
pub(crate) struct ReuseBuildOpts {
    /// Path to nextest archive
    #[arg(
        long,
        groups = &["cargo-metadata-sources", "binaries-metadata-sources", "target-dir-remap-sources"],
        conflicts_with_all = &["cargo-opts", "binaries_metadata", "cargo_metadata"],
        value_name = "PATH",
    )]
    pub(crate) archive_file: Option<Utf8PathBuf>,

    /// Archive format
    #[arg(
        long,
        value_enum,
        default_value_t,
        requires = "archive_file",
        value_name = "FORMAT"
    )]
    pub(crate) archive_format: ArchiveFormatOpt,

    /// Destination directory to extract archive to [default: temporary directory]
    #[arg(
        long,
        conflicts_with = "cargo-opts",
        requires = "archive_file",
        value_name = "DIR"
    )]
    pub(crate) extract_to: Option<Utf8PathBuf>,

    /// Overwrite files in destination directory while extracting archive
    #[arg(long, conflicts_with = "cargo-opts", requires_all = &["archive_file", "extract_to"])]
    pub(crate) extract_overwrite: bool,

    /// Persist temporary directory destination is extracted to
    #[arg(long, conflicts_with_all = &["cargo-opts", "extract_to"], requires = "archive_file")]
    pub(crate) persist_extract_tempdir: bool,

    /// Path to cargo metadata JSON
    #[arg(
        long,
        group = "cargo-metadata-sources",
        conflicts_with = "manifest_path",
        value_name = "PATH"
    )]
    pub(crate) cargo_metadata: Option<Utf8PathBuf>,

    /// Remapping for the workspace root
    #[arg(long, requires = "cargo-metadata-sources", value_name = "PATH")]
    pub(crate) workspace_remap: Option<Utf8PathBuf>,

    /// Path to binaries-metadata JSON
    #[arg(
        long,
        group = "binaries-metadata-sources",
        conflicts_with = "cargo-opts",
        value_name = "PATH"
    )]
    pub(crate) binaries_metadata: Option<Utf8PathBuf>,

    /// Remapping for the target directory
    #[arg(
        long,
        group = "target-dir-remap-sources",
        // Note: --target-dir-remap is incompatible with --archive, hence this requires
        // binaries-metadata and not binaries-metadata-sources.
        requires = "binaries_metadata",
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
    ) -> Result<ReuseBuildInfo> {
        if let Some(archive_file) = &self.archive_file {
            let format = self.archive_format.to_archive_format(archive_file)?;
            // Process this archive.
            let dest = match &self.extract_to {
                Some(dir) => ExtractDestination::Destination {
                    dir: dir.clone(),
                    overwrite: self.extract_overwrite,
                },
                None => ExtractDestination::TempDir {
                    persist: self.persist_extract_tempdir,
                },
            };

            let mut reporter = ArchiveReporter::new(output.verbose);
            if output.color.should_colorize(supports_color::Stream::Stderr) {
                reporter.colorize();
            }

            let mut writer = output_writer.stderr_writer();
            return ReuseBuildInfo::extract_archive(
                archive_file,
                format,
                dest,
                |event| {
                    reporter.report_event(event, &mut writer)?;
                    writer.flush()
                },
                self.workspace_remap.as_deref(),
            )
            .map_err(|err| ExpectedError::ArchiveExtractError {
                archive_file: archive_file.clone(),
                err: Box::new(err),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ArchiveFormatOpt {
    Auto,
    #[clap(alias = "tar-zstd")]
    TarZst,
}

impl ArchiveFormatOpt {
    pub(crate) fn to_archive_format(self, archive_file: &Utf8Path) -> Result<ArchiveFormat> {
        match self {
            Self::TarZst => Ok(ArchiveFormat::TarZst),
            Self::Auto => ArchiveFormat::autodetect(archive_file).map_err(|err| {
                ExpectedError::UnknownArchiveFormat {
                    archive_file: archive_file.to_owned(),
                    err,
                }
            }),
        }
    }
}

impl Default for ArchiveFormatOpt {
    fn default() -> Self {
        Self::Auto
    }
}

pub(crate) fn make_path_mapper(
    info: &ReuseBuildInfo,
    graph: &PackageGraph,
    orig_target_dir: &Utf8Path,
) -> Result<PathMapper> {
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
