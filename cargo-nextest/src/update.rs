// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{output::OutputContext, ExpectedError, Result};
use camino::Utf8PathBuf;
use nextest_metadata::NextestExitCode;
use nextest_runner::update::{CheckStatus, MuktiBackend, UpdateVersion};
use owo_colors::{OwoColorize, Stream};
use semver::Version;
use std::cmp::Ordering;

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
    version: &str,
    check: bool,
    yes: bool,
    force: bool,
    releases_url: Option<String>,
    output: OutputContext,
) -> Result<i32> {
    let version = version
        .parse::<UpdateVersion>()
        .map_err(|err| ExpectedError::UpdateVersionParseError { err })?;
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

    let status = releases.check(&version, force, &bin_path_in_archive, |v| {
        // Use cmp_precedence here to disregard build metadata.
        v.cmp_precedence(&min_version_with_setup()).is_ge()
    })?;

    match status {
        CheckStatus::AlreadyOnRequested(version) => {
            log::info!(
                "cargo-nextest is already at the latest version: {}",
                version.if_supports_color(Stream::Stderr, |s| s.bold())
            );
            Ok(0)
        }
        CheckStatus::DowngradeNotAllowed {
            current_version,
            requested,
        } => {
            log::info!(
                "not performing downgrade from {} to {}\n\
            (pass in --force to force downgrade)",
                current_version.if_supports_color(Stream::Stderr, |s| s.bold()),
                requested.if_supports_color(Stream::Stderr, |s| s.bold()),
            );
            Ok(NextestExitCode::UPDATE_DOWNGRADE_NOT_PERFORMED)
        }
        CheckStatus::Success(ctx) => {
            log::info!(
                "{} available: {} -> {}",
                match ctx.version.cmp(&current_version) {
                    Ordering::Greater => "update",
                    Ordering::Equal => "reinstall",
                    Ordering::Less => "downgrade",
                },
                current_version.if_supports_color(Stream::Stderr, |s| s.bold()),
                ctx.version.if_supports_color(Stream::Stderr, |s| s.bold())
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
                ctx.do_update()
                    .map_err(|err| ExpectedError::UpdateError { err })?;
                log::info!(
                    "cargo-nextest updated to {}",
                    ctx.version.if_supports_color(Stream::Stderr, |s| s.bold())
                );
                Ok(0)
            } else {
                log::info!("update canceled");
                Ok(NextestExitCode::UPDATE_CANCELED)
            }
        }
    }
}
