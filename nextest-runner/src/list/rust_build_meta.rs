// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::list::{BinaryListState, PathMapper, TestListState};
use camino::Utf8PathBuf;
use nextest_metadata::RustBuildMetaSummary;
use std::{collections::BTreeSet, marker::PhantomData};

/// Rust-related metadata used for builds and test runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustBuildMeta<State> {
    /// The target directory for build artifacts.
    pub target_directory: Utf8PathBuf,

    /// A list of base output directories, relative to the target directory. These directories
    /// and their "deps" subdirectories are added to the dynamic library path.
    pub base_output_directories: BTreeSet<Utf8PathBuf>,

    /// A list of linked paths, relative to the target directory. These directories are
    /// added to the dynamic library path.
    pub linked_paths: BTreeSet<Utf8PathBuf>,

    state: PhantomData<State>,
}

impl RustBuildMeta<BinaryListState> {
    /// Creates a new [`RustBuildMeta`].
    pub fn new(target_directory: impl Into<Utf8PathBuf>) -> Self {
        Self {
            target_directory: target_directory.into(),
            base_output_directories: BTreeSet::new(),
            linked_paths: BTreeSet::new(),
            state: PhantomData,
        }
    }

    /// Maps paths using a [`PathMapper`] to convert this to [`TestListState`].
    pub fn map_paths(&self, path_mapper: &PathMapper) -> RustBuildMeta<TestListState> {
        RustBuildMeta {
            target_directory: path_mapper
                .new_target_dir()
                .unwrap_or(&self.target_directory)
                .to_path_buf(),
            // Since these are relative paths, they don't need to be mapped.
            base_output_directories: self.base_output_directories.clone(),
            linked_paths: self.linked_paths.clone(),
            state: PhantomData,
        }
    }
}

impl RustBuildMeta<TestListState> {
    /// Empty metadata for tests.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            target_directory: Utf8PathBuf::new(),
            base_output_directories: BTreeSet::new(),
            linked_paths: BTreeSet::new(),
            state: PhantomData,
        }
    }

    /// Returns the dynamic library paths corresponding to this metadata.
    ///
    /// [See this Cargo documentation for more.](https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths)
    ///
    /// These paths are prepended to the dynamic library environment variable for the current
    /// platform (e.g. `LD_LIBRARY_PATH` on non-Apple Unix platforms).
    pub fn dylib_paths(&self) -> Vec<Utf8PathBuf> {
        // TODO: rustc sysroot library path? is this even possible to get?

        // Cargo puts linked paths before base output directories.
        self.linked_paths
            .iter()
            .map(|rel_path| self.target_directory.join(rel_path))
            .chain(self.base_output_directories.iter().flat_map(|base_output| {
                let abs_base = self.target_directory.join(base_output);
                let with_deps = abs_base.join("deps");
                // This is the order paths are added in by Cargo.
                [with_deps, abs_base]
            }))
            .collect()
    }
}

impl<State> RustBuildMeta<State> {
    /// Creates a `RustBuildMeta` from a serializable summary.
    pub fn from_summary(summary: RustBuildMetaSummary) -> Self {
        Self {
            target_directory: summary.target_directory,
            base_output_directories: summary.base_output_directories,
            linked_paths: summary.linked_paths,
            state: PhantomData,
        }
    }

    /// Converts self to a serializable form.
    pub fn to_summary(&self) -> RustBuildMetaSummary {
        RustBuildMetaSummary {
            target_directory: self.target_directory.clone(),
            base_output_directories: self.base_output_directories.clone(),
            linked_paths: self.linked_paths.clone(),
        }
    }
}
