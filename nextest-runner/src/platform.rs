// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

use crate::{
    cargo_config::{CargoTargetArg, TargetTriple},
    errors::{TargetTripleError, UnknownHostPlatform},
};
use target_spec::summaries::PlatformSummary;
pub use target_spec::Platform;

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
    /// Returns an error if the host platform could not be determined.
    pub fn new(target: Option<TargetTriple>) -> Result<Self, UnknownHostPlatform> {
        let host = Platform::current().map_err(|error| UnknownHostPlatform { error })?;
        Ok(Self { host, target })
    }

    /// Creates a new `BuildPlatform` where the host is specified.
    ///
    /// This is intended for testing situations. Most users should call [`Self::new`] instead.
    pub fn new_with_host(host: Platform, target: Option<TargetTriple>) -> Self {
        Self { host, target }
    }

    /// Returns the argument to pass into `cargo metadata --filter-platform <triple>`.
    pub fn to_cargo_target_arg(&self) -> Result<CargoTargetArg, TargetTripleError> {
        match &self.target {
            Some(target) => target.to_cargo_target_arg(),
            None => {
                // If there's no target, use the host platform.
                Ok(CargoTargetArg::Builtin(self.host.triple_str().to_owned()))
            }
        }
    }

    /// Converts a target triple to a `String` that can be stored in the build-metadata.
    ///
    /// cargo-nextest represents the host triple with `None` during runtime. However the
    /// build-metadata might be used on a system with a different host triple. Therefore, the host
    /// triple is detected if `target_triple` is `None`.
    ///
    /// XXX: This isn't quite correct -- instead, we should serialize this into our own
    /// `BuildPlatformsSummary`.
    pub fn to_summary(&self) -> PlatformSummary {
        if let Some(target) = &self.target {
            target.platform.to_summary()
        } else {
            self.host.to_summary()
        }
    }
}
