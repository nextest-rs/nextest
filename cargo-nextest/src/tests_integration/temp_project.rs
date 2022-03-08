// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{
    fs, io,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// A temporary copy of the test project
///
/// This avoid concurrent accesses to the `target` folder.
pub struct TempProject {
    workspace_dir: TempDir,
}

fn copy_dir_all(src: &Path, dst: &Path, root: bool) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if root && entry.path().file_name() == Some(std::ffi::OsStr::new("target")) {
                continue;
            }
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()), false)?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

impl TempProject {
    pub fn new() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("nextest-fixture")
            .tempdir()?;

        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fixtures/nextest-tests");

        copy_dir_all(&src_dir, dir.path(), true)?;

        Ok(Self { workspace_dir: dir })
    }

    pub fn workspace_root(&self) -> &Path {
        self.workspace_dir.path()
    }

    pub fn binaries_metadata_path(&self) -> PathBuf {
        self.workspace_root().join("binaries_metadata.json")
    }

    pub fn cargo_metadata_path(&self) -> PathBuf {
        self.workspace_root().join("cargo_metadata.json")
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.workspace_dir.path().join("Cargo.toml")
    }
}
