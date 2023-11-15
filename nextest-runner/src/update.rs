// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Self-updates for nextest.

use crate::errors::{UpdateError, UpdateVersionParseError};
use camino::{Utf8Path, Utf8PathBuf};
use mukti_metadata::{MuktiProject, MuktiReleasesJson, ReleaseLocation, ReleaseStatus};
use self_update::{ArchiveKind, Compression, Download, Extract};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::{
    fs,
    io::{self, BufWriter},
    str::FromStr,
};
use target_spec::Platform;

/// Update backend using mukti
#[derive(Clone, Debug)]
pub struct MuktiBackend {
    /// The URL to download releases from
    pub url: String,

    /// The package name.
    pub package_name: String,
}

impl MuktiBackend {
    /// Fetch releases.
    pub fn fetch_releases(&self, current_version: Version) -> Result<NextestReleases, UpdateError> {
        log::info!(target: "nextest-runner::update", "checking for self-updates");
        // Is the URL a file that exists on disk? If so, use that.
        let as_path = Utf8Path::new(&self.url);
        let releases_buf = if as_path.exists() {
            fs::read(as_path).map_err(|error| UpdateError::ReadLocalMetadata {
                path: as_path.to_owned(),
                error,
            })?
        } else {
            let mut releases_buf: Vec<u8> = Vec::new();
            Download::from_url(&self.url)
                .download_to(&mut releases_buf)
                .map_err(UpdateError::SelfUpdate)?;
            releases_buf
        };

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

        NextestReleases::new(&self.package_name, project, current_version)
    }
}

/// Release info for nextest.
///
/// Returned by [`MuktiBackend::fetch_releases`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NextestReleases {
    /// The package name.
    pub package_name: String,

    /// The mukti project.
    pub project: MuktiProject,

    /// The currently running version.
    pub current_version: Version,

    /// The install path.
    pub bin_install_path: Utf8PathBuf,
}

impl NextestReleases {
    fn new(
        package_name: &str,
        project: MuktiProject,
        current_version: Version,
    ) -> Result<Self, UpdateError> {
        let bin_install_path = std::env::current_exe()
            .and_then(|exe| {
                Utf8PathBuf::try_from(exe)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
            })
            .map_err(UpdateError::CurrentExe)?;

        Ok(Self {
            package_name: package_name.to_owned(),
            project,
            current_version,
            bin_install_path,
        })
    }

    /// Checks for whether an update should be performed.
    pub fn check<'a>(
        &'a self,
        version: &UpdateVersion,
        force: bool,
        bin_path_in_archive: &'a Utf8Path,
        perform_setup_fn: impl FnOnce(&Version) -> bool,
    ) -> Result<CheckStatus<'a>, UpdateError> {
        let (version, version_data) = self.get_version_data(version)?;
        log::debug!(
            target: "nextest-runner::update",
            "current version is {}, update version is {version}",
            self.current_version,
        );

        if &self.current_version == version && !force {
            return Ok(CheckStatus::AlreadyOnRequested(version.clone()));
        }
        if &self.current_version > version && !force {
            return Ok(CheckStatus::DowngradeNotAllowed {
                current_version: self.current_version.clone(),
                requested: version.clone(),
            });
        }

        // Look for data for this platform.
        let triple = self.target_triple();
        log::debug!(target: "nextest-runner::update", "target triple: {triple}");

        let location = version_data
            .locations
            .iter()
            .find(|&data| data.format == TAR_GZ_SUFFIX && data.target == triple)
            .ok_or_else(|| {
                let known_triples = version_data
                    .locations
                    .iter()
                    .filter(|data| data.format == TAR_GZ_SUFFIX)
                    .map(|data| data.target.clone())
                    .collect();
                UpdateError::NoTargetData {
                    version: version.clone(),
                    triple,
                    known_triples,
                }
            })?;

        let force_disable_setup = version_data
            .metadata
            .map_or(false, |metadata| metadata.force_disable_setup);
        let perform_setup = !force_disable_setup && perform_setup_fn(version);

        Ok(CheckStatus::Success(MuktiUpdateContext {
            context: self,
            version: version.clone(),
            location: location.clone(),
            bin_path_in_archive,
            perform_setup,
        }))
    }

    // ---
    // Helper methods
    // ---

    fn get_version_data(
        &self,
        version: &UpdateVersion,
    ) -> Result<(&Version, ReleaseVersionData), UpdateError> {
        let (version, release_data) = match version {
            UpdateVersion::Exact(version) => {
                self.project.get_version_data(version).ok_or_else(|| {
                    let known = self
                        .project
                        .all_versions()
                        .map(|(v, release_data)| (v.clone(), release_data.status))
                        .collect();
                    UpdateError::VersionNotFound {
                        version: version.clone(),
                        known,
                    }
                })?
            }
            UpdateVersion::Req(req) => self
                .project
                .get_latest_matching(req)
                .ok_or_else(|| UpdateError::NoMatchForVersionReq { req: req.clone() })?,
        };

        // Parse the metadata into our custom format.
        let metadata = if release_data.metadata.is_null() {
            None
        } else {
            // Attempt to parse the metadata.
            match serde_json::from_value::<NextestReleaseMetadata>(release_data.metadata.clone()) {
                Ok(metadata) => Some(metadata),
                Err(error) => {
                    log::warn!(
                        target: "nextest-runner::update",
                        "failed to parse custom release metadata: {error}",
                    );
                    None
                }
            }
        };

        let release_data = ReleaseVersionData {
            release_url: release_data.release_url.clone(),
            status: release_data.status,
            locations: release_data.locations.clone(),
            metadata,
        };
        Ok((version, release_data))
    }

    fn target_triple(&self) -> String {
        let current = Platform::current().expect("current platform could not be detected");
        let triple_str = current.triple_str();
        if triple_str.ends_with("-apple-darwin") {
            // Nextest builds a universal binary for Mac.
            "universal-apple-darwin".to_owned()
        } else {
            triple_str.to_owned()
        }
    }
}

/// Like `mukti-metadata`'s `ReleaseVersionData`, except with parsed metadata.
#[derive(Clone, Debug)]
pub struct ReleaseVersionData {
    /// Canonical URL for this release
    pub release_url: String,

    /// The status of a release
    pub status: ReleaseStatus,

    /// Release locations
    pub locations: Vec<ReleaseLocation>,

    /// Custom domain-specific information stored about this release.
    pub metadata: Option<NextestReleaseMetadata>,
}

/// Nextest-specific release metadata.
#[derive(Clone, Debug, Deserialize)]
pub struct NextestReleaseMetadata {
    /// Whether to force disable `cargo nextest self setup` for this version.
    #[serde(default)]
    pub force_disable_setup: bool,
}

/// The result of [`NextestReleases::check`].
#[derive(Clone, Debug)]
pub enum CheckStatus<'a> {
    /// The current version is the same as the requested version.
    AlreadyOnRequested(Version),

    /// A downgrade was requested but wasn't allowed.
    DowngradeNotAllowed {
        /// The currently running version.
        current_version: Version,

        /// The requested version.
        requested: Version,
    },

    /// All checks were performed successfully and we are ready to update.
    Success(MuktiUpdateContext<'a>),
}
/// Context for an update.
///
/// Returned as part of the `Success` variant of [`CheckStatus`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MuktiUpdateContext<'a> {
    /// The `MuktiReleases` context.
    pub context: &'a NextestReleases,

    /// The version being updated to.
    pub version: Version,

    /// The target-specific release location from which the package will be downloaded.
    pub location: ReleaseLocation,

    /// The path to the binary within the archive.
    pub bin_path_in_archive: &'a Utf8Path,

    /// Whether to run `cargo nextest self setup` as part of the update.
    pub perform_setup: bool,
}

impl<'a> MuktiUpdateContext<'a> {
    /// Performs the update.
    pub fn do_update(&self) -> Result<(), UpdateError> {
        // This method is adapted from self_update's update_extended.

        let tmp_dir_parent = self.context.bin_install_path.parent().ok_or_else(|| {
            UpdateError::CurrentExe(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "parent directory of current exe `{}` could not be determined",
                    self.context.bin_install_path
                ),
            ))
        })?;
        let tmp_backup_dir_prefix = format!("__{}_backup", self.context.package_name);
        #[allow(clippy::redundant_clone)]
        let tmp_backup_filename = tmp_backup_dir_prefix.clone();

        if cfg!(windows) {
            // Windows executables can not be removed while they are running, which prevents clean up
            // of the temporary directory by the `tempfile` crate after we move the running executable
            // into it during an update. We clean up any previously created temporary directories here.
            // Ignore errors during cleanup since this is not critical for completing the update.
            let _ = cleanup_backup_temp_directories(
                tmp_dir_parent,
                &tmp_backup_dir_prefix,
                &tmp_backup_filename,
            );
        }

        let tmp_archive_dir_prefix = format!("{}_download", self.context.package_name);
        let tmp_archive_dir = camino_tempfile::Builder::new()
            .prefix(&tmp_archive_dir_prefix)
            .tempdir_in(tmp_dir_parent)
            .map_err(|error| UpdateError::TempDirCreate {
                location: tmp_dir_parent.to_owned(),
                error,
            })?;
        let tmp_dir_path: &Utf8Path = tmp_archive_dir.path();
        let tmp_archive_path =
            tmp_dir_path.join(format!("{}.{TAR_GZ_SUFFIX}", self.context.package_name));
        let tmp_archive = fs::File::create(&tmp_archive_path).map_err(|error| {
            UpdateError::TempArchiveCreate {
                archive_path: tmp_archive_path.clone(),
                error,
            }
        })?;
        let mut tmp_archive_buf = BufWriter::new(tmp_archive);

        let mut download = Download::from_url(&self.location.url);
        let mut headers = http::header::HeaderMap::new();
        headers.insert(
            http::header::ACCEPT,
            "application/octet-stream".parse().unwrap(),
        );
        download.set_headers(headers);
        download.show_progress(true);
        // TODO: set progress style

        download
            .download_to(&mut tmp_archive_buf)
            .map_err(UpdateError::SelfUpdate)?;

        log::debug!(target: "nextest-runner::update", "downloaded to {tmp_archive_path}");

        let tmp_archive =
            tmp_archive_buf
                .into_inner()
                .map_err(|error| UpdateError::TempArchiveWrite {
                    archive_path: tmp_archive_path.clone(),
                    error: error.into_error(),
                })?;
        tmp_archive
            .sync_all()
            .map_err(|error| UpdateError::TempArchiveWrite {
                archive_path: tmp_archive_path.clone(),
                error,
            })?;
        std::mem::drop(tmp_archive);

        // Now extract data from this archive.
        Extract::from_source(tmp_archive_path.as_std_path())
            .archive(ArchiveKind::Tar(Some(Compression::Gz)))
            .extract_file(
                tmp_archive_dir.path().as_std_path(),
                self.bin_path_in_archive,
            )
            .map_err(UpdateError::SelfUpdate)?;

        // Since we're currently restricted to .tar.gz which carries metadata with it, there's no
        // need to make this file executable.

        let new_exe = tmp_dir_path.join(self.bin_path_in_archive);
        log::debug!(target: "nextest-runner::update", "extracted to {new_exe}, replacing existing binary");

        let tmp_backup_dir = camino_tempfile::Builder::new()
            .prefix(&tmp_backup_dir_prefix)
            .tempdir_in(tmp_dir_parent)
            .map_err(|error| UpdateError::TempDirCreate {
                location: tmp_dir_parent.to_owned(),
                error,
            })?;

        let tmp_backup_dir_path: &Utf8Path = tmp_backup_dir.path();
        let tmp_file_path = tmp_backup_dir_path.join(&tmp_backup_filename);

        Move::from_source(&new_exe)
            .replace_using_temp(&tmp_file_path)
            .to_dest(&self.context.bin_install_path)?;

        // Finally, run `cargo nextest self setup` if requested.
        if self.perform_setup {
            log::info!(target: "nextest-runner::update", "running `cargo nextest self setup`");
            let mut cmd = std::process::Command::new(&self.context.bin_install_path);
            cmd.args(["nextest", "self", "setup", "--source", "self-update"]);
            let status = cmd.status().map_err(UpdateError::SelfSetup)?;
            if !status.success() {
                return Err(UpdateError::SelfSetup(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "`cargo nextest self setup` failed with exit code {}",
                        status
                            .code()
                            .map_or("(unknown)".to_owned(), |c| c.to_string())
                    ),
                )));
            }
        }

        Ok(())
    }
}

/// Moves a file from the given path to the specified destination.
///
/// `source` and `dest` must be on the same filesystem.
/// If `replace_using_temp` is specified, the destination file will be
/// replaced using the given temporary path.
/// If the existing `dest` file is a currently running long running program,
/// `replace_using_temp` may run into errors cleaning up the temp dir.
/// If that's the case for your use-case, consider not specifying a temp dir to use.
///
/// * Errors:
///     * Io - copying / renaming
#[derive(Debug)]
struct Move<'a> {
    source: &'a Utf8Path,
    temp: Option<&'a Utf8Path>,
}
impl<'a> Move<'a> {
    /// Specify source file
    pub fn from_source(source: &'a Utf8Path) -> Move<'a> {
        Self { source, temp: None }
    }

    /// If specified and the destination file already exists, the "destination"
    /// file will be moved to the given temporary location before the "source"
    /// file is moved to the "destination" file.
    ///
    /// In the event of an `io` error while renaming "source" to "destination",
    /// the temporary file will be moved back to "destination".
    ///
    /// The `temp` dir must be explicitly provided since `rename` operations require
    /// files to live on the same filesystem.
    pub fn replace_using_temp(&mut self, temp: &'a Utf8Path) -> &mut Self {
        self.temp = Some(temp);
        self
    }

    /// Move source file to specified destination
    pub fn to_dest(&self, dest: &Utf8Path) -> Result<(), UpdateError> {
        match self.temp {
            None => Self::fs_rename(self.source, dest),
            Some(temp) => {
                if dest.exists() {
                    // Move the existing dest to a temp location so we can move it
                    // back it there's an error. If the existing `dest` file is a
                    // long running program, this may prevent the temp dir from
                    // being cleaned up.
                    Self::fs_rename(dest, temp)?;
                    if let Err(e) = Self::fs_rename(self.source, dest) {
                        Self::fs_rename(temp, dest)?;
                        return Err(e);
                    }
                } else {
                    Self::fs_rename(self.source, dest)?;
                }
                Ok(())
            }
        }
    }

    // ---
    // Helper methods
    // ---

    fn fs_rename(source: &Utf8Path, dest: &Utf8Path) -> Result<(), UpdateError> {
        fs::rename(source, dest).map_err(|error| UpdateError::FsRename {
            source: source.to_owned(),
            dest: dest.to_owned(),
            error,
        })
    }
}

fn cleanup_backup_temp_directories(
    tmp_dir_parent: &Utf8Path,
    tmp_dir_prefix: &str,
    expected_tmp_filename: &str,
) -> io::Result<()> {
    for entry in fs::read_dir(tmp_dir_parent)? {
        let entry = entry?;
        let tmp_dir_name = if let Ok(tmp_dir_name) = entry.file_name().into_string() {
            tmp_dir_name
        } else {
            continue;
        };

        // For safety, check that the temporary directory contains only the expected backup
        // binary file before removing. If subdirectories or other files exist then the user
        // is using the temp directory for something else. This is unlikely, but we should
        // be careful with `fs::remove_dir_all`.
        let is_expected_tmp_file = |tmp_file_entry: std::io::Result<fs::DirEntry>| {
            tmp_file_entry
                .ok()
                .filter(|e| e.file_name() == expected_tmp_filename)
                .is_some()
        };

        if tmp_dir_name.starts_with(tmp_dir_prefix)
            && fs::read_dir(entry.path())?.all(is_expected_tmp_file)
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

const TAR_GZ_SUFFIX: &str = "tar.gz";

/// Represents the version this project is being updated to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateVersion {
    /// Update to this exact version.
    Exact(Version),

    /// Update to the latest non-pre-release, non-yanked version matching this [`VersionReq`].
    Req(VersionReq),
}

/// Parses x.y.z as if it were =x.y.z, and provides error messages in the case of invalid
/// values.
impl FromStr for UpdateVersion {
    type Err = UpdateVersionParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        // Adapted from Cargo's source:
        // https://github.com/rust-lang/cargo/blob/6b8e1922261bbed1894bf40069fb2d5dc8d62fb0/src/cargo/ops/cargo_install.rs#L760-L806

        // If the version begins with character <, >, =, ^, ~ parse it as a
        // version range, otherwise parse it as a specific version

        if input == "latest" {
            return Ok(UpdateVersion::Req(VersionReq::STAR));
        }

        let first = input
            .chars()
            .next()
            .ok_or(UpdateVersionParseError::EmptyString)?;

        let is_req = "<>=^~".contains(first) || input.contains('*');
        if is_req {
            match input.parse::<VersionReq>() {
                Ok(v) => Ok(Self::Req(v)),
                Err(error) => Err(UpdateVersionParseError::InvalidVersionReq {
                    input: input.to_owned(),
                    error,
                }),
            }
        } else {
            match input.parse::<Version>() {
                Ok(v) => Ok(Self::Exact(v)),
                Err(error) => Err(UpdateVersionParseError::InvalidVersion {
                    input: input.to_owned(),
                    error,
                }),
            }
        }
    }
}
