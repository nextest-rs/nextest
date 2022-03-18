// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputContext, ExpectedError};
use camino::Utf8PathBuf;
use clap::Args;
use color_eyre::eyre::{Report, Result};
use guppy::graph::PackageGraph;
use nextest_runner::list::PathMapper;

#[derive(Debug, Args)]
#[clap(next_help_heading = "REUSE BUILD OPTIONS (EXPERIMENTAL)")]
pub(crate) struct ReuseBuildOpts {
    /// Path to binaries-metadata JSON
    #[clap(long, value_name = "PATH", env = "NEXTEST_BINARIES_METADATA")]
    pub(crate) binaries_metadata: Option<Utf8PathBuf>,

    /// Remapping for the test binaries directory
    #[clap(
        long,
        requires("binaries-metadata"),
        value_name = "PATH",
        env = "NEXTEST_BINARIES_DIR_REMAP"
    )]
    pub(crate) binaries_dir_remap: Option<Utf8PathBuf>,

    /// Path to cargo metadata JSON
    #[clap(
        long,
        conflicts_with("manifest-path"),
        value_name = "PATH",
        env = "NEXTEST_CARGO_METADATA"
    )]
    pub(crate) cargo_metadata: Option<Utf8PathBuf>,

    /// Remapping for the workspace root
    #[clap(
        long,
        requires("cargo-metadata"),
        value_name = "PATH",
        env = "NEXTEST_WORKSPACE_REMAP"
    )]
    pub(crate) workspace_remap: Option<Utf8PathBuf>,
}

impl ReuseBuildOpts {
    const EXPERIMENTAL_ENV: &'static str = "NEXTEST_EXPERIMENTAL_REUSE_BUILD";

    // (_output is not used, but must be passed in to ensure that the output is properly initialized
    // before calling this method)
    pub(crate) fn check_experimental(&self, _output: OutputContext) -> Result<()> {
        let used = self.binaries_metadata.is_some()
            || self.binaries_dir_remap.is_some()
            || self.cargo_metadata.is_some()
            || self.workspace_remap.is_some();

        let enabled = std::env::var(Self::EXPERIMENTAL_ENV).is_ok();

        if used && !enabled {
            Err(Report::new(ExpectedError::experimental_feature_error(
                "build reuse",
                Self::EXPERIMENTAL_ENV,
            )))
        } else {
            Ok(())
        }
    }

    pub(crate) fn make_path_mapper(&self, graph: &PackageGraph) -> Option<PathMapper> {
        PathMapper::new(
            graph,
            self.workspace_remap.clone(),
            self.binaries_dir_remap.clone(),
        )
    }
}
