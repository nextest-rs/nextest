// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use std::{fs, io, path::Path};

// This isn't general purpose -- it specifically excludes certain directories at the root and is
// generally not race-free.
pub(super) fn copy_dir_all(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    root: bool,
) -> io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if root && entry.path().file_name() == Some(std::ffi::OsStr::new("target")) {
                continue;
            }
            copy_dir_all(entry.path(), dst.join(entry.file_name()), false)?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A temporary copy of the test project
///
/// This avoid concurrent accesses to the `target` folder.
#[derive(Debug)]
pub struct TempProject {
    temp_dir: Option<Utf8TempDir>,
    temp_root: Utf8PathBuf,
    workspace_root: Utf8PathBuf,
    target_dir: Utf8PathBuf,
}

impl TempProject {
    pub fn new() -> color_eyre::Result<Self> {
        Self::new_impl(None)
    }

    pub fn new_custom_target_dir(target_dir: &Utf8Path) -> color_eyre::Result<Self> {
        Self::new_impl(Some(target_dir.to_path_buf()))
    }

    fn new_impl(custom_target_dir: Option<Utf8PathBuf>) -> color_eyre::Result<Self> {
        let temp_dir = camino_tempfile::Builder::new()
            .prefix("nextest-fixture")
            .tempdir()?;
        // Note: can't use canonicalize here because it ends up creating a UNC path on Windows,
        // which doesn't match compile time.
        let temp_root: Utf8PathBuf = fixup_macos_path(temp_dir.path());
        let workspace_root = temp_root.join("src");
        let src_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fixtures/nextest-tests");

        copy_dir_all(src_dir, &workspace_root, true)?;

        let target_dir = match custom_target_dir {
            Some(dir) => fixup_macos_path(&dir),
            None => workspace_root.join("target"),
        };

        Ok(Self {
            temp_dir: Some(temp_dir),
            temp_root,
            workspace_root,
            target_dir,
        })
    }

    #[expect(dead_code)]
    pub fn persist(&mut self) {
        if let Some(dir) = self.temp_dir.take() {
            _ = dir.into_path();
        }
    }

    pub fn temp_root(&self) -> &Utf8Path {
        &self.temp_root
    }

    pub fn workspace_root(&self) -> &Utf8Path {
        &self.workspace_root
    }

    pub fn target_dir(&self) -> &Utf8Path {
        &self.target_dir
    }

    pub fn set_target_dir(&mut self, target_dir: impl Into<Utf8PathBuf>) {
        self.target_dir = target_dir.into();
    }

    pub fn binaries_metadata_path(&self) -> Utf8PathBuf {
        self.target_dir.join("binaries_metadata.json")
    }

    pub fn cargo_metadata_path(&self) -> Utf8PathBuf {
        self.target_dir.join("cargo_metadata.json")
    }

    pub fn manifest_path(&self) -> Utf8PathBuf {
        self.workspace_root.join("Cargo.toml")
    }
}

#[cfg(target_os = "macos")]
fn fixup_macos_path(path: &Utf8Path) -> Utf8PathBuf {
    // Prepend "/private" to the workspace path since macOS creates temp dirs there.
    if path.starts_with("/var/folders") {
        let mut s = String::from("/private");
        s.push_str(path.as_str());
        Utf8PathBuf::from(s)
    } else {
        path.to_path_buf()
    }
}

#[cfg(not(target_os = "macos"))]
fn fixup_macos_path(path: &Utf8Path) -> Utf8PathBuf {
    path.to_path_buf()
}

#[cfg(unix)]
mod unix {
    use super::UdsStatus;
    use camino::Utf8Path;
    use color_eyre::eyre::{Context, Result};

    pub(crate) fn create_uds(path: &Utf8Path) -> Result<UdsStatus> {
        // This creates a Unix domain socket by binding it to a path.
        use std::os::unix::net::UnixListener;

        UnixListener::bind(path).wrap_err_with(|| format!("failed to bind UDS at {path}"))?;
        Ok(UdsStatus::Created)
    }
}
#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(windows)]
mod windows {
    use super::UdsStatus;
    use camino::Utf8Path;
    use color_eyre::eyre::Result;

    pub(crate) fn create_uds(_path: &Utf8Path) -> Result<UdsStatus> {
        // While Unix domain sockets are supported on Windows, Rust 1.77's `is_file()` returns true for
        // them. This means that the `UnknownFileType` warning can't be produced for them. So we can't
        // actually test that case on Windows.
        Ok(UdsStatus::NotCreated)
    }
}
#[cfg(windows)]
pub(crate) use windows::*;

#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
#[expect(dead_code)]
pub(crate) enum UdsStatus {
    Created,
    NotCreated,
    NotRequested,
}
