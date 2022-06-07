// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Self-updates for nextest.

use crate::errors::UpdateError;
use camino::Utf8PathBuf;
use mukti_metadata::{MuktiProject, MuktiReleasesJson, ReleaseVersionData};
use self_update::{
    cargo_crate_version,
    update::{Release, ReleaseAsset, ReleaseUpdate},
    Download,
};
use semver::Version;
use target_spec::Platform;

/// Update backend using mukti
#[derive(Clone, Debug)]
pub struct MuktiBackend {
    /// The URL to download releases from
    pub url: String,

    /// The package name.
    pub package_name: String,

    /// Colorizes the output.
    pub colorize: bool,
}

impl MuktiBackend {
    /// Fetch releases.
    pub fn fetch_releases(&self) -> Result<MuktiReleases, UpdateError> {
        let mut releases_buf: Vec<u8> = Vec::new();
        Download::from_url(&self.url)
            .show_progress(true)
            .download_to(&mut releases_buf)
            .map_err(UpdateError::SelfUpdate)?;
        let mut releases_json: MuktiReleasesJson =
            serde_json::from_slice(&releases_buf).map_err(UpdateError::ReleaseMetadataDe)?;

        let project = match releases_json.projects.remove(&self.package_name) {
            Some(project) => project,
            None => {
                return Err(UpdateError::MuktiProjectNotFound {
                    not_found: self.package_name.clone(),
                    known: releases_json.projects.keys().cloned().collect(),
                })
            }
        };

        MuktiReleases::new(&self.package_name, project, self.colorize)
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MuktiReleases {
    /// The package name.
    pub package_name: String,

    /// The mukti project.
    pub project: MuktiProject,

    /// The install path.
    pub bin_install_path: Utf8PathBuf,

    /// Do not confirm.
    pub no_confirm: bool,

    /// Colorizes the output.
    pub colorize: bool,
}

impl MuktiReleases {
    fn new(package_name: &str, project: MuktiProject, colorize: bool) -> Result<Self, UpdateError> {
        let bin_install_path = std::env::current_exe()
            .and_then(|exe| {
                Utf8PathBuf::try_from(exe)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
            })
            .map_err(UpdateError::CurrentExe)?;

        Ok(Self {
            package_name: package_name.to_owned(),
            project,
            bin_install_path,
            no_confirm: false,
            colorize,
        })
    }

    fn get_latest_release(&self) -> Result<(&Version, &ReleaseVersionData), String> {
        let range = self
            .project
            .latest
            .ok_or_else(|| "release data has no latest release".to_owned())?;
        let range_data = self
            .project
            .ranges
            .get(&range)
            .ok_or_else(|| format!("no data found for range {}", range))?;
        let latest_version = &range_data.latest;
        let version_data = range_data
            .versions
            .get(latest_version)
            .ok_or_else(|| format!("no data found for version {}", latest_version))?;
        Ok((latest_version, version_data))
    }

    fn target(&self) -> String {
        let current = Platform::current().expect("current platform could not be detected");
        let triple_str = current.triple_str();
        if triple_str.ends_with("-apple-darwin") {
            // Nextest builds a universal binary for Mac.
            "universal-apple-darwin".to_owned()
        } else {
            triple_str.to_owned()
        }
    }

    // ---
    // Helper methods
    // ---

    fn release_version_data_to_release(
        &self,
        version: &Version,
        version_data: &ReleaseVersionData,
    ) -> Release {
        let assets = version_data
            .locations
            .iter()
            .filter_map(|location| {
                // Only return .tar.gz files.
                (location.format == ".tar.gz").then(|| ReleaseAsset {
                    download_url: location.url.clone(),
                    // Use the target for the name -- `ReleaseAsset`'s methods use that.
                    name: location.target.clone(),
                })
            })
            .collect();
        Release {
            name: self.package_name.clone(),
            version: format!("{}", version),
            // We currently don't track date in mukti.
            date: "".to_owned(),
            body: None,
            assets,
        }
    }
}

impl ReleaseUpdate for MuktiReleases {
    fn get_latest_release(&self) -> self_update::errors::Result<Release> {
        let (version, version_data) = self
            .get_latest_release()
            .map_err(self_update::errors::Error::Release)?;
        Ok(self.release_version_data_to_release(version, version_data))
    }

    fn get_release_version(
        &self,
        _version: &str,
    ) -> self_update::errors::Result<self_update::update::Release> {
        todo!("need to implement getting release info for a specific version")
    }

    fn current_version(&self) -> String {
        cargo_crate_version!().to_owned()
    }

    fn target(&self) -> String {
        self.target()
    }

    fn target_version(&self) -> Option<String> {
        // TODO: need to implement support for updating to specific versions.
        None
    }

    fn bin_name(&self) -> String {
        "cargo-nextest".to_owned()
    }

    fn bin_install_path(&self) -> std::path::PathBuf {
        self.bin_install_path.clone().into()
    }

    fn bin_path_in_archive(&self) -> std::path::PathBuf {
        "cargo-nextest".into()
    }

    fn show_download_progress(&self) -> bool {
        // TODO: make configurable
        true
    }

    fn show_output(&self) -> bool {
        // TODO: make configurable?
        false
    }

    fn no_confirm(&self) -> bool {
        self.no_confirm
    }

    fn progress_style(&self) -> Option<indicatif::ProgressStyle> {
        // TODO: set a custom progress style
        None
    }

    fn auth_token(&self) -> Option<String> {
        None
    }
}
