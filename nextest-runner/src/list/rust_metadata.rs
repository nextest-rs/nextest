// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::list::{BinaryListState, PathMapper, TestListState};
use camino::Utf8PathBuf;
use nextest_metadata::RustMetadataSummary;
use std::marker::PhantomData;

/// Rust-related metadata used for builds and test runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustMetadata<State> {
    /// The target directory for build artifacts.
    pub target_directory: Utf8PathBuf,
    state: PhantomData<State>,
}

impl RustMetadata<BinaryListState> {
    /// Creates a new [`RustMetadata`].
    pub fn new(target_directory: impl Into<Utf8PathBuf>) -> Self {
        Self {
            target_directory: target_directory.into(),
            state: PhantomData,
        }
    }

    /// Maps paths using a [`PathMapper`] to convert this to [`TestListState`].
    pub fn map_paths(&self, path_mapper: &PathMapper) -> RustMetadata<TestListState> {
        RustMetadata {
            target_directory: path_mapper
                .new_target_dir()
                .unwrap_or(&self.target_directory)
                .to_path_buf(),
            state: PhantomData,
        }
    }
}

impl RustMetadata<TestListState> {
    /// Empty metadata for tests.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            target_directory: Utf8PathBuf::new(),
            state: PhantomData,
        }
    }
}

impl<State> RustMetadata<State> {
    /// Creates a `RustMetadata` from a serializable summary.
    pub fn from_summary(summary: RustMetadataSummary) -> Self {
        Self {
            target_directory: summary.target_directory,
            state: PhantomData,
        }
    }

    /// Converts self to a serializable form.
    pub fn to_summary(&self) -> RustMetadataSummary {
        RustMetadataSummary {
            target_directory: self.target_directory.clone(),
        }
    }
}
