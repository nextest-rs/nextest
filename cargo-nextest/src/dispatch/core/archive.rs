// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Archive command options and execution.

use super::{base::BaseApp, filter::ArchiveBuildFilter};
use crate::{
    ExpectedError, Result, cargo_cli::CargoOptions, dispatch::helpers::build_filtersets,
    output::OutputWriter, reuse_build::ArchiveFormatOpt,
};
use camino::Utf8PathBuf;
use clap::Args;
use nextest_filtering::{FiltersetKind, ParseContext};
use nextest_runner::{
    redact::Redactor,
    reuse_build::{ArchiveReporter, PathMapper, apply_archive_filters, archive_to_file},
    test_filter::BinaryFilter,
    write_str::WriteStr,
};
use std::collections::BTreeSet;

/// Options for the archive command.
#[derive(Debug, Args)]
pub(crate) struct ArchiveOpts {
    #[clap(flatten)]
    pub(crate) cargo_options: CargoOptions,

    /// File to write archive to.
    #[arg(
        long,
        name = "archive-file",
        help_heading = "Archive options",
        value_name = "PATH"
    )]
    pub(crate) archive_file: Utf8PathBuf,

    /// Archive format.
    ///
    /// `auto` uses the file extension to determine the archive format. Currently supported is
    /// `.tar.zst`.
    #[arg(
        long,
        value_enum,
        help_heading = "Archive options",
        value_name = "FORMAT",
        default_value_t
    )]
    pub(crate) archive_format: ArchiveFormatOpt,

    #[clap(flatten)]
    pub(crate) archive_build_filter: ArchiveBuildFilter,

    /// Zstandard compression level (-7 to 22, higher is more compressed + slower).
    #[arg(
        long,
        help_heading = "Archive options",
        value_name = "LEVEL",
        default_value_t = 0,
        allow_negative_numbers = true
    )]
    pub(crate) zstd_level: i32,
    // ReuseBuildOpts, while it can theoretically work, is way too confusing so skip it.
}

/// Application for archiving tests.
pub(crate) struct ArchiveApp {
    base: BaseApp,
    archive_filter: ArchiveBuildFilter,
}

impl ArchiveApp {
    pub(crate) fn new(base: BaseApp, archive_filter: ArchiveBuildFilter) -> Result<Self> {
        Ok(Self {
            base,
            archive_filter,
        })
    }

    pub(crate) fn exec_archive(
        &self,
        output_file: &camino::Utf8Path,
        format: ArchiveFormatOpt,
        zstd_level: i32,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        // Do format detection first so we fail immediately.
        let format = format.to_archive_format(output_file)?;
        let binary_list = self.base.build_binary_list("test")?;
        let path_mapper = PathMapper::noop();
        let build_platforms = &binary_list.rust_build_meta.build_platforms;
        let pcx = ParseContext::new(self.base.graph());
        let (_, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self
            .base
            .load_profile(&config)?
            .apply_build_platforms(build_platforms);
        let redactor = if crate::output::should_redact() {
            Redactor::build_active(&binary_list.rust_build_meta)
                .with_path(output_file.to_path_buf(), "<archive-file>".to_owned())
                .build()
        } else {
            Redactor::noop()
        };
        let mut reporter = ArchiveReporter::new(self.base.output.verbose, redactor.clone());

        if self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr)
        {
            reporter.colorize();
        }

        let filtersets = build_filtersets(
            &pcx,
            &self.archive_filter.filterset,
            FiltersetKind::TestArchive,
        )?;
        let binary_filter = BinaryFilter::new(filtersets);
        let ecx = profile.filterset_ecx();

        let (binary_list_to_archive, filter_counts) = apply_archive_filters(
            self.base.graph(),
            binary_list.clone(),
            &binary_filter,
            &ecx,
            &path_mapper,
        )?;

        let mut writer = output_writer.stderr_writer();
        archive_to_file(
            profile,
            &binary_list_to_archive,
            filter_counts,
            &self.base.cargo_metadata_json,
            self.base.graph(),
            // Note that path_mapper is currently a no-op -- we don't support reusing builds for
            // archive creation because it's too confusing.
            &path_mapper,
            format,
            zstd_level,
            output_file,
            |event| {
                reporter.report_event(event, &mut writer)?;
                writer.write_str_flush()
            },
            redactor.clone(),
        )
        .map_err(|err| ExpectedError::ArchiveCreateError {
            archive_file: output_file.to_owned(),
            err,
            redactor,
        })?;

        Ok(())
    }
}
