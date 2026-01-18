// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Self command implementation for managing the nextest installation.

use crate::{Result, output::OutputContext};
use clap::{Args, Subcommand, ValueEnum};

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
    ) -> Result<nextest_runner::update::UpdateVersionReq, crate::ExpectedError> {
        use crate::ExpectedError;
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

impl SelfCommand {
    #[cfg_attr(not(feature = "self-update"), expect(unused_variables))]
    pub(crate) fn exec(self, output: OutputContext) -> Result<i32> {
        match self {
            Self::Setup { source: _source } => Ok(0),
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
