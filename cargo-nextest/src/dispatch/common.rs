// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Common options shared between cargo nextest and cargo ntr.

use crate::{ExpectedError, Result};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use nextest_filtering::ParseContext;
use nextest_runner::config::core::{NextestConfig, ToolConfigFile, VersionOnlyConfig};
use std::collections::BTreeSet;

/// Options shared between cargo nextest and cargo ntr.
#[derive(Debug, Args)]
pub(crate) struct CommonOpts {
    /// Path to Cargo.toml.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Manifest options"
    )]
    pub(crate) manifest_path: Option<Utf8PathBuf>,

    #[clap(flatten)]
    pub(crate) output: crate::output::OutputOpts,

    #[clap(flatten)]
    pub(crate) config_opts: ConfigOpts,
}

/// Configuration options for nextest.
#[derive(Debug, Args)]
#[command(next_help_heading = "Config options")]
pub(crate) struct ConfigOpts {
    /// Config file [default: workspace-root/.config/nextest.toml].
    #[arg(long, global = true, value_name = "PATH")]
    pub config_file: Option<Utf8PathBuf>,

    /// Tool-specific config files.
    ///
    /// Some tools on top of nextest may want to set up their own default configuration but
    /// prioritize user configuration on top. Use this argument to insert configuration
    /// that's lower than --config-file in priority but above the default config shipped with
    /// nextest.
    ///
    /// Arguments are specified in the format "tool:abs_path", for example
    /// "my-tool:/path/to/nextest.toml" (or "my-tool:C:\\path\\to\\nextest.toml" on Windows).
    /// Paths must be absolute.
    ///
    /// This argument may be specified multiple times. Files that come later are lower priority
    /// than those that come earlier.
    #[arg(long = "tool-config-file", global = true, value_name = "TOOL:ABS_PATH")]
    pub tool_config_files: Vec<ToolConfigFile>,

    /// Override checks for the minimum version defined in nextest's config.
    ///
    /// Repository and tool-specific configuration files can specify minimum required and
    /// recommended versions of nextest. This option overrides those checks.
    #[arg(long, global = true)]
    pub override_version_check: bool,

    /// The nextest profile to use.
    ///
    /// Nextest's configuration supports multiple profiles, which can be used to set up different
    /// configurations for different purposes. (For example, a configuration for local runs and one
    /// for CI.) This option selects the profile to use.
    #[arg(
        long,
        short = 'P',
        env = "NEXTEST_PROFILE",
        global = true,
        help_heading = "Config options"
    )]
    pub(crate) profile: Option<String>,
}

impl ConfigOpts {
    /// Creates a nextest version-only config with the given options.
    pub(crate) fn make_version_only_config(
        &self,
        workspace_root: &Utf8Path,
    ) -> Result<VersionOnlyConfig> {
        VersionOnlyConfig::from_sources(
            workspace_root,
            self.config_file.as_deref(),
            &self.tool_config_files,
        )
        .map_err(ExpectedError::config_parse_error)
    }

    /// Creates a nextest config with the given options.
    pub(crate) fn make_config(
        &self,
        workspace_root: &Utf8Path,
        pcx: &ParseContext<'_>,
        experimental: &BTreeSet<nextest_runner::config::core::ConfigExperimental>,
    ) -> Result<NextestConfig> {
        NextestConfig::from_sources(
            workspace_root,
            pcx,
            self.config_file.as_deref(),
            &self.tool_config_files,
            experimental,
        )
        .map_err(ExpectedError::config_parse_error)
    }
}
