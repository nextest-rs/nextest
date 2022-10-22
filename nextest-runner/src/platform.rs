// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

pub use target_spec::Platform;

use crate::cargo_config::TargetTriple;

/// A representation of host and target platforms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatforms {
    /// The host platform.
    pub host: Platform,

    /// The target platform, if specified.
    ///
    /// In the future, this will become a list of target triples once multiple `--target` arguments
    /// are supported.
    pub target: Option<TargetTriple>,
}

impl BuildPlatforms {
    /// Creates a new `BuildPlatform`.
    ///
    /// The host platform should typically be set to `Platform::current()`.
    pub fn new(host: Platform, target: Option<TargetTriple>) -> Self {
        Self { host, target }
    }
}
