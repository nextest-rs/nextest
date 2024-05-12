// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

use crate::{
    cargo_config::{CargoTargetArg, TargetTriple},
    errors::{RustBuildMetaParseError, TargetTripleError, UnknownHostPlatform},
};
use nextest_metadata::{BuildPlatformsSummary, HostPlatformSummary, TargetPlatformSummary};
use target_spec::summaries::PlatformSummary;
pub use target_spec::Platform;

/// The target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatformsTarget {
    /// The target triplet, which consists of machine, vendor and OS.
    pub triple: TargetTriple,
}

/// A representation of host and target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatforms {
    /// The host platform.
    pub host: Platform,

    /// The target platform, if specified.
    pub target: Option<BuildPlatformsTarget>,
}

impl BuildPlatforms {
    /// Creates a new [`BuildPlatforms`].
    ///
    /// Returns an error if the host platform could not be determined.
    pub fn new() -> Result<Self, UnknownHostPlatform> {
        let host = Platform::current().map_err(|error| UnknownHostPlatform { error })?;
        Ok(Self { host, target: None })
    }

    /// Returns the argument to pass into `cargo metadata --filter-platform <triple>`.
    pub fn to_cargo_target_arg(&self) -> Result<CargoTargetArg, TargetTripleError> {
        match &self.target {
            Some(target) => target.triple.to_cargo_target_arg(),
            None => {
                // If there's no target, use the host platform.
                Ok(CargoTargetArg::Builtin(self.host.triple_str().to_owned()))
            }
        }
    }

    /// Converts a target triple to a [`String`] that can be stored in the build-metadata.
    ///
    /// Only for backward compatibility. Deprecated in favor of [`BuildPlatformsSummary`].
    pub fn to_summary_str(&self) -> Option<String> {
        self.target
            .as_ref()
            .map(|triple| triple.triple.platform.triple_str().to_owned())
    }

    /// Creates a [`BuildPlatforms`] from a serializable summary.
    ///
    /// Only for backward compatibility. Deprecated in favor of [`BuildPlatformsSummary`].
    pub fn from_summary_str(summary: Option<String>) -> Result<Self, RustBuildMetaParseError> {
        let mut build_platforms = BuildPlatforms::new()
            .map_err(|error| RustBuildMetaParseError::UnknownHostPlatform(error.error))?;
        if let Some(triple) = TargetTriple::deserialize_str(summary)? {
            build_platforms.target = Some(BuildPlatformsTarget { triple });
        }
        Ok(build_platforms)
    }
}

/// A value-to-value conversion that consumes the input value and generate a serialized summary
/// type. The opposite of [`FromSummary`].
pub trait ToSummary<T> {
    /// Converts this type into the (usually inferred) input type.
    fn to_summary(&self) -> T;
}

/// Simple and safe conversions from a serialized summary type that may fail in a controlled way
/// under some circumstances. The serialized summary will be stored in the build-metadata, It is the
/// reciprocal of [`ToSummary`].
pub trait FromSummary<T>: Sized {
    /// The type returned in the event of a conversion error.
    type Error: Sized;

    /// Performs the conversion.
    fn from_summary(summary: T) -> Result<Self, Self::Error>;
}

/// Deprecated in favor of [`BuildPlatformsSummary`].
impl ToSummary<PlatformSummary> for BuildPlatforms {
    fn to_summary(&self) -> PlatformSummary {
        if let Some(target) = &self.target {
            target.triple.platform.to_summary()
        } else {
            self.host.to_summary()
        }
    }
}

impl ToSummary<HostPlatformSummary> for BuildPlatforms {
    fn to_summary(&self) -> HostPlatformSummary {
        HostPlatformSummary {
            platform: self.host.to_summary(),
        }
    }
}

impl ToSummary<TargetPlatformSummary> for BuildPlatformsTarget {
    fn to_summary(&self) -> TargetPlatformSummary {
        TargetPlatformSummary {
            platform: self.triple.platform.to_summary(),
        }
    }
}

/// Creates a [`BuildPlatforms`] from a serializable summary for backwards compatibility.
impl FromSummary<PlatformSummary> for BuildPlatforms {
    type Error = RustBuildMetaParseError;

    fn from_summary(summary: PlatformSummary) -> Result<Self, Self::Error> {
        let mut build_platforms = BuildPlatforms::new()
            .map_err(|error| RustBuildMetaParseError::UnknownHostPlatform(error.error))?;
        if let Some(triple) = TargetTriple::deserialize(Some(summary))? {
            build_platforms.target = Some(BuildPlatformsTarget { triple });
        }
        Ok(build_platforms)
    }
}

/// Creates a [`BuildPlatforms`] from a serializable summary.
impl FromSummary<BuildPlatformsSummary> for BuildPlatforms {
    type Error = RustBuildMetaParseError;

    fn from_summary(summary: BuildPlatformsSummary) -> Result<Self, Self::Error> {
        Ok(BuildPlatforms {
            host: summary.host.platform.to_platform()?,
            target: {
                if summary.targets.len() > 1 {
                    return Err(RustBuildMetaParseError::Unsupported {
                        message: "multiple build targets is not supported".to_owned(),
                    });
                }
                let target_platform_summary = summary
                    .targets
                    .first()
                    .map(|target| &target.platform)
                    .cloned();
                TargetTriple::deserialize(target_platform_summary)?
                    .map(|triple| BuildPlatformsTarget { triple })
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_platform_new() {
        let build_platforms = BuildPlatforms::new().expect("default ctor should succeed");
        assert_eq!(
            build_platforms,
            BuildPlatforms {
                host: Platform::current().expect("should detect the current platform successfully"),
                target: None,
            }
        );
    }
}
