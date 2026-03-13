// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Show-config command implementation.

use crate::{
    Result,
    cargo_cli::CargoOptions,
    dispatch::{
        EarlyArgs,
        common::ConfigOpts,
        core::{App, BaseApp, TestBuildFilter, current_version},
        helpers::locate_workspace_root,
    },
    output::{OutputContext, OutputWriter},
    reuse_build::ReuseBuildOpts,
};
use camino::Utf8PathBuf;
use clap::Subcommand;
use nextest_runner::{config::core::NextestVersionEval, errors::WriteTestListError};
use tracing::Level;

/// Subcommands for show-config.
#[derive(Debug, Subcommand)]
pub(crate) enum ShowConfigCommand {
    /// Show version-related configuration.
    Version {},
    /// Show defined test groups and their associated tests.
    TestGroups {
        /// Show default test groups.
        #[arg(long)]
        show_default: bool,

        /// Show only the named groups.
        #[arg(long)]
        groups: Vec<nextest_runner::config::elements::TestGroup>,

        #[clap(flatten)]
        cargo_options: Box<CargoOptions>,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        #[clap(flatten)]
        reuse_build: Box<ReuseBuildOpts>,
    },
}

impl ShowConfigCommand {
    pub(crate) fn exec(
        self,
        early_args: EarlyArgs,
        manifest_path: Option<Utf8PathBuf>,
        config_opts: ConfigOpts,
        output: OutputContext,
        output_writer: &mut OutputWriter,
    ) -> Result<i32> {
        match self {
            Self::Version {} => {
                let workspace_root = locate_workspace_root(manifest_path.as_deref(), output)?;

                let config = config_opts.make_version_only_config(&workspace_root)?;
                let current_version = current_version();

                let show = nextest_runner::show_config::ShowNextestVersion::new(
                    config.nextest_version(),
                    &current_version,
                    config_opts.override_version_check,
                );
                show.write_human(
                    &mut output_writer.stdout_writer(),
                    output.color.should_colorize(supports_color::Stream::Stdout),
                )
                .map_err(WriteTestListError::Io)?;

                match config
                    .nextest_version()
                    .eval(&current_version, config_opts.override_version_check)
                {
                    NextestVersionEval::Satisfied => Ok(0),
                    NextestVersionEval::Error { .. } => {
                        crate::helpers::log_needs_update(
                            Level::ERROR,
                            crate::helpers::BYPASS_VERSION_TEXT,
                            &output.stderr_styles(),
                        );
                        Ok(nextest_metadata::NextestExitCode::REQUIRED_VERSION_NOT_MET)
                    }
                    NextestVersionEval::Warn { .. } => {
                        crate::helpers::log_needs_update(
                            Level::WARN,
                            crate::helpers::BYPASS_VERSION_TEXT,
                            &output.stderr_styles(),
                        );
                        Ok(nextest_metadata::NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
                    }
                    NextestVersionEval::ErrorOverride { .. }
                    | NextestVersionEval::WarnOverride { .. } => Ok(0),
                }
            }
            Self::TestGroups {
                show_default,
                groups,
                cargo_options,
                build_filter,
                reuse_build,
            } => {
                let base = BaseApp::new(
                    output,
                    early_args,
                    *reuse_build,
                    *cargo_options,
                    config_opts,
                    manifest_path,
                    output_writer,
                )?;
                let app = App::new(base, build_filter)?;

                app.exec_show_test_groups(show_default, groups)?;

                Ok(0)
            }
        }
    }
}
