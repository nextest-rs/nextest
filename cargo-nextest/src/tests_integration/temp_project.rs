// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use fs_extra::dir::CopyOptions;
use std::convert::TryInto;
use tempfile::TempDir;

/// A temporary copy of the test project
///
/// This avoid concurrent accesses to the `target` folder.
pub struct TempProject {
    #[allow(dead_code)]
    temp_dir: TempDir,
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
        let temp_dir = tempfile::Builder::new()
            .prefix("nextest-fixture")
            .tempdir()?;
        let utf8_path: Utf8PathBuf = temp_dir
            .path()
            .to_path_buf()
            .try_into()
            .expect("tempdir should be valid UTF-8");

        let src_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fixtures/nextest-tests");

        fs_extra::copy_items(&[&src_dir], &utf8_path, &CopyOptions::new())?;

        let workspace_root = utf8_path.join("nextest-tests");

        let target_dir = custom_target_dir.unwrap_or_else(|| workspace_root.join("target"));
        match std::fs::remove_dir_all(&target_dir) {
            Ok(()) => {}
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    color_eyre::eyre::bail!(err);
                }
            }
        }

        Ok(Self {
            temp_dir,
            workspace_root,
            target_dir,
        })
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
