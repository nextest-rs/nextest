// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Self command implementation for managing the nextest installation.

use crate::{ExpectedError, Result, output::OutputContext};
use camino::Utf8PathBuf;
use clap::{Args, Subcommand, ValueEnum};
use nextest_runner::{config::core::NextestConfig, user_config::UserConfig};
use std::io::Write;
use tracing::info;

/// Arguments for specifying which version to update to.
#[derive(Debug, Args)]
#[group(id = "version_spec", multiple = false)]
pub(crate) struct UpdateVersionOpt {
    /// Version or version range to download.
    #[arg(long, help_heading = "Version selection")]
    version: Option<String>,

    /// Update to the latest version, including prereleases.
    ///
    /// This installs the latest beta, RC, or stable version.
    #[arg(long, help_heading = "Version selection")]
    beta: bool,

    /// Update to the latest RC or stable version.
    ///
    /// This installs the latest RC or stable version. Beta versions are
    /// excluded.
    #[arg(long, help_heading = "Version selection")]
    rc: bool,
}

impl UpdateVersionOpt {
    /// Converts to `UpdateVersionReq`.
    #[cfg(feature = "self-update")]
    pub(crate) fn to_update_version_req(
        &self,
    ) -> Result<nextest_runner::update::UpdateVersionReq, ExpectedError> {
        use nextest_runner::update::{PrereleaseKind, UpdateVersionReq};

        if self.beta {
            Ok(UpdateVersionReq::LatestPrerelease(PrereleaseKind::Beta))
        } else if self.rc {
            Ok(UpdateVersionReq::LatestPrerelease(PrereleaseKind::Rc))
        } else if let Some(version) = &self.version {
            version
                .parse()
                .map(UpdateVersionReq::Version)
                .map_err(|err| ExpectedError::UpdateVersionParseError { err })
        } else {
            Ok(UpdateVersionReq::Latest)
        }
    }
}

/// Subcommands for self.
#[derive(Debug, Subcommand)]
pub(crate) enum SelfCommand {
    #[clap(hide = true)]
    /// Perform setup actions (currently a no-op).
    Setup {
        /// The entity running the setup command.
        #[arg(long, value_enum, default_value_t = SetupSource::User)]
        source: SetupSource,
    },
    /// Print an embedded JSON Schema.
    Schema {
        #[command(subcommand)]
        kind: SchemaCommand,
    },
    #[cfg_attr(
        not(feature = "self-update"),
        doc = "This version of nextest does not have self-update enabled.\n\
        \n\
        Always exits with code 93 (SELF_UPDATE_UNAVAILABLE).
        "
    )]
    #[cfg_attr(
        feature = "self-update",
        doc = "Download and install updates to nextest.\n\
        \n\
        This command checks the internet for updates to nextest, then downloads and
        installs them if an update is available."
    )]
    Update {
        #[command(flatten)]
        version: UpdateVersionOpt,

        /// Check for updates rather than downloading them.
        ///
        /// If no update is available, exits with code 0. If an update is available, exits with code
        /// 80 (UPDATE_AVAILABLE).
        #[arg(short = 'n', long)]
        check: bool,

        /// Do not prompt for confirmation.
        #[arg(short = 'y', long, conflicts_with = "check")]
        yes: bool,

        /// Force downgrades and reinstalls.
        #[arg(short, long)]
        force: bool,

        /// URL or path to fetch releases.json from.
        #[arg(long)]
        releases_url: Option<String>,
    },
}

/// Source of the setup command.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SetupSource {
    User,
    SelfUpdate,
    PackageManager,
}

/// Selects which embedded JSON Schema to print.
#[derive(Debug, Subcommand)]
pub(crate) enum SchemaCommand {
    /// Print the JSON Schema for `.config/nextest.toml`.
    RepoConfig(SchemaOutputOpts),
    /// Print the JSON Schema for the user config file (e.g. `~/.config/nextest/config.toml`).
    UserConfig(SchemaOutputOpts),
}

/// Output options shared by `cargo nextest self schema` subcommands.
#[derive(Debug, Args)]
pub(crate) struct SchemaOutputOpts {
    /// Output file path. Defaults to stdout.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    output: Option<Utf8PathBuf>,
}

impl SchemaCommand {
    fn exec(self) -> Result<i32> {
        match self {
            Self::RepoConfig(opts) => opts.exec(NextestConfig::SCHEMA, "repo config"),
            Self::UserConfig(opts) => opts.exec(UserConfig::SCHEMA, "user config"),
        }
    }
}

impl SchemaOutputOpts {
    fn exec(&self, schema: &str, label: &'static str) -> Result<i32> {
        match &self.output {
            Some(path) => {
                std::fs::write(path, schema).map_err(|err| ExpectedError::SchemaWriteError {
                    label,
                    path: path.clone(),
                    err,
                })?;
                info!("wrote JSON Schema for {label} to {path}");
            }
            None => {
                // Lock stdout to keep the write and flush atomic, and propagate
                // flush errors so that broken pipes (e.g. `… | head`) don't
                // silently truncate output.
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                stdout
                    .write_all(schema.as_bytes())
                    .map_err(|err| ExpectedError::WriteError { err })?;
                stdout
                    .flush()
                    .map_err(|err| ExpectedError::WriteError { err })?;
            }
        }
        Ok(0)
    }
}

impl SelfCommand {
    #[cfg_attr(not(feature = "self-update"), expect(unused_variables))]
    pub(crate) fn exec(self, output: OutputContext) -> Result<i32> {
        match self {
            Self::Setup { source: _source } => Ok(0),
            Self::Schema { kind } => kind.exec(),
            Self::Update {
                version,
                check,
                yes,
                force,
                releases_url,
            } => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "self-update")] {
                        let version_req = version.to_update_version_req()?;
                        crate::update::perform_update(
                            version_req,
                            check,
                            yes,
                            force,
                            releases_url,
                            output,
                        )
                    } else {
                        let _ = version;
                        tracing::info!(
                            "this version of cargo-nextest cannot perform self-updates\n\
                            (hint: this usually means nextest was installed by a package manager)"
                        );
                        Ok(nextest_metadata::NextestExitCode::SELF_UPDATE_UNAVAILABLE)
                    }
                }
            }
        }
    }
}
