// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{ExpectedError, Result, output::OutputContext};
use camino::Utf8PathBuf;
use nextest_metadata::NextestExitCode;
use nextest_runner::update::{CheckStatus, MuktiBackend, UpdateVersionReq};
use owo_colors::OwoColorize;
use semver::Version;
use std::cmp::Ordering;
use tracing::{debug, info};

// Returns the smallest version with a `self setup` command.
fn min_version_with_setup() -> Version {
    // For testing, we allow this to be overridden via an environment variable.
    if let Ok(version) = std::env::var("__NEXTEST_SETUP_MIN_VERSION") {
        version
            .parse()
            .expect("__NEXTEST_SETUP_MIN_VERSION must be a valid semver version")
    } else {
        Version::parse("0.9.62").unwrap()
    }
}

/// Perform an update.
pub(crate) fn perform_update(
    version_req: UpdateVersionReq,
    check: bool,
    yes: bool,
    force: bool,
    releases_url: Option<String>,
    output: OutputContext,
) -> Result<i32> {
    let releases_url =
        releases_url.unwrap_or_else(|| "https://get.nexte.st/releases.json".to_owned());

    // Configure the backend.
    let backend = MuktiBackend {
        url: releases_url,
        package_name: "cargo-nextest".to_owned(),
    };

    let current_version: Version = env!("CARGO_PKG_VERSION")
        .parse()
        .expect("cargo-nextest uses semantic versioning");

    let releases = backend.fetch_releases(current_version.clone())?;

    // The binary is always present at this path.
    let mut bin_path_in_archive = Utf8PathBuf::from("cargo-nextest");
    bin_path_in_archive.set_extension(std::env::consts::EXE_EXTENSION);

    let status = releases.check(&version_req, force, &bin_path_in_archive, |v| {
        // Use cmp_precedence here to disregard build metadata.
        v.cmp_precedence(&min_version_with_setup()).is_ge()
    })?;

    let styles = output.stderr_styles();

    match status {
        CheckStatus::AlreadyOnRequested(version) => {
            info!(
                "cargo-nextest is already at the latest version: {}",
                version.style(styles.bold),
            );
            Ok(0)
        }
        CheckStatus::DowngradeNotAllowed {
            current_version,
            requested,
        } => {
            info!(
                "not performing downgrade from {} to {}\n\
            (pass in --force to force downgrade)",
                current_version.style(styles.bold),
                requested.style(styles.bold),
            );
            Ok(NextestExitCode::UPDATE_DOWNGRADE_NOT_PERFORMED)
        }
        CheckStatus::Success(ctx) => {
            info!(
                "{} available: {} -> {}",
                match ctx.version.cmp(&current_version) {
                    Ordering::Greater => "update",
                    Ordering::Equal => "reinstall",
                    Ordering::Less => "downgrade",
                },
                current_version.style(styles.bold),
                ctx.version.style(styles.bold)
            );
            if check {
                // check + non-empty ops implies a non-zero exit status.
                return Ok(NextestExitCode::UPDATE_AVAILABLE);
            }

            let should_apply = if yes {
                true
            } else {
                let colorful_theme = dialoguer::theme::ColorfulTheme::default();
                let confirm = if output.color.should_colorize(supports_color::Stream::Stderr) {
                    dialoguer::Confirm::with_theme(&colorful_theme)
                } else {
                    dialoguer::Confirm::with_theme(&dialoguer::theme::SimpleTheme)
                };
                confirm
                    .with_prompt("proceed?")
                    .default(true)
                    .show_default(true)
                    .interact()
                    .map_err(|err| ExpectedError::DialoguerError { err })?
            };

            if should_apply {
                debug!(url = ctx.location.url, "updating cargo nextest");
                ctx.do_update()
                    .map_err(|err| ExpectedError::UpdateError { err })?;
                info!(
                    "cargo-nextest updated to {}",
                    ctx.version.style(styles.bold)
                );
                Ok(0)
            } else {
                info!("update cancelled");
                Ok(NextestExitCode::UPDATE_CANCELED)
            }
        }
    }
}
