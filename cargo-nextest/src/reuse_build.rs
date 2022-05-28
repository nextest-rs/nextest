// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputContext, ExpectedError};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use guppy::graph::PackageGraph;
use nextest_runner::{errors::PathMapperConstructKind, reuse_build::PathMapper};

#[derive(Debug, Args)]
#[clap(next_help_heading = "REUSE BUILD OPTIONS")]
pub(crate) struct ReuseBuildOpts {
    /// Path to binaries-metadata JSON
    #[clap(long, value_name = "PATH")]
    pub(crate) binaries_metadata: Option<Utf8PathBuf>,

    /// Remapping for the target directory
    #[clap(long, requires("binaries-metadata"), value_name = "PATH")]
    pub(crate) target_dir_remap: Option<Utf8PathBuf>,

    /// Path to cargo metadata JSON
    #[clap(long, conflicts_with("manifest-path"), value_name = "PATH")]
    pub(crate) cargo_metadata: Option<Utf8PathBuf>,

    /// Remapping for the workspace root
    #[clap(long, requires("cargo-metadata"), value_name = "PATH")]
    pub(crate) workspace_remap: Option<Utf8PathBuf>,
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

    pub(crate) fn make_path_mapper(
        &self,
        graph: &PackageGraph,
        orig_target_dir: &Utf8Path,
    ) -> Result<PathMapper, ExpectedError> {
        PathMapper::new(
            graph.workspace().root(),
            self.workspace_remap.as_deref(),
            orig_target_dir,
            self.target_dir_remap.as_deref(),
        )
        .map_err(|err| {
            let arg_name = match err.kind() {
                PathMapperConstructKind::WorkspaceRoot => "workspace-remap",
                PathMapperConstructKind::TargetDir => "target-dir-remap",
            };
            ExpectedError::PathMapperConstructError { arg_name, err }
        })
    }
}
