// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

use crate::{
    cargo_config::{CargoTargetArg, TargetTriple},
    errors::{RustBuildMetaParseError, TargetTripleError, UnknownHostPlatform},
    reuse_build::{LibdirMapper, PlatformLibdirMapper},
};
use camino::{Utf8Path, Utf8PathBuf};
use nextest_metadata::{
    BuildPlatformsSummary, HostPlatformSummary, PlatformLibdirSummary, PlatformLibdirUnavailable,
    TargetPlatformSummary,
};
use target_spec::summaries::PlatformSummary;
pub use target_spec::Platform;

/// A representation of host and target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatforms {
    /// The host platform.
    pub host: HostPlatform,

    /// The target platform, if specified.
    ///
    /// In the future, this will support multiple targets.
    pub target: Option<TargetPlatform>,
}

impl BuildPlatforms {
    /// Creates a new `BuildPlatforms` with no libdirs or targets.
    ///
    /// Used for testing.
    pub fn new_with_no_target() -> Result<Self, UnknownHostPlatform> {
        Ok(Self {
            host: HostPlatform::current(PlatformLibdir::Unavailable(
                PlatformLibdirUnavailable::new_const("test"),
            ))?,
            target: None,
        })
    }

    /// Maps libdir paths.
    pub fn map_libdir(&self, mapper: &LibdirMapper) -> Self {
        Self {
            host: self.host.map_libdir(&mapper.host),
            target: self
                .target
                .as_ref()
                .map(|target| target.map_libdir(&mapper.target)),
        }
    }

    /// Returns the argument to pass into `cargo metadata --filter-platform <triple>`.
    pub fn to_cargo_target_arg(&self) -> Result<CargoTargetArg, TargetTripleError> {
        match &self.target {
            Some(target) => target.triple.to_cargo_target_arg(),
            None => {
                // If there's no target, use the host platform.
                Ok(CargoTargetArg::Builtin(
                    self.host.platform.triple_str().to_owned(),
                ))
            }
        }
    }

    /// Converts self to a summary.
    pub fn to_summary(&self) -> BuildPlatformsSummary {
        BuildPlatformsSummary {
            host: self.host.to_summary(),
            targets: self
                .target
                .as_ref()
                .map(|target| vec![target.to_summary()])
                .unwrap_or_default(),
        }
    }

    /// Converts self to a single summary.
    ///
    /// Pairs with [`Self::from_target_summary`]. Deprecated in favor of [`BuildPlatformsSummary`].
    pub fn to_target_or_host_summary(&self) -> PlatformSummary {
        if let Some(target) = &self.target {
            target.triple.platform.to_summary()
        } else {
            self.host.platform.to_summary()
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

    /// Converts a summary to a [`BuildPlatforms`].
    pub fn from_summary(summary: BuildPlatformsSummary) -> Result<Self, RustBuildMetaParseError> {
        Ok(BuildPlatforms {
            host: HostPlatform::from_summary(summary.host)?,
            target: {
                if summary.targets.len() > 1 {
                    return Err(RustBuildMetaParseError::Unsupported {
                        message: "multiple build targets is not supported".to_owned(),
                    });
                }
                summary
                    .targets
                    .first()
                    .map(|target| TargetPlatform::from_summary(target.clone()))
                    .transpose()?
            },
        })
    }

    /// Creates a [`BuildPlatforms`] from a single `PlatformSummary`.
    ///
    /// Only for backwards compatibility. Deprecated in favor of [`BuildPlatformsSummary`].
    pub fn from_target_summary(summary: PlatformSummary) -> Result<Self, RustBuildMetaParseError> {
        // In this case:
        //
        // * no libdirs are available
        // * the host might be serialized as the target platform as well (we can't detect this case
        //   reliably, so we treat it as the target platform as well, which isn't a problem in
        //   practice).
        let host = HostPlatform::current(PlatformLibdir::Unavailable(
            PlatformLibdirUnavailable::OLD_SUMMARY,
        ))
        .map_err(|error| RustBuildMetaParseError::UnknownHostPlatform(error.error))?;

        let target = TargetTriple::deserialize(Some(summary))?.map(|triple| {
            TargetPlatform::new(
                triple,
                PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
            )
        });

        Ok(Self { host, target })
    }

    /// Creates a [`BuildPlatforms`] from a target triple.
    ///
    /// Only for backward compatibility. Deprecated in favor of [`BuildPlatformsSummary`].
    pub fn from_summary_str(summary: Option<String>) -> Result<Self, RustBuildMetaParseError> {
        // In this case:
        //
        // * no libdirs are available
        // * can't represent custom platforms
        // * the host might be serialized as the target platform as well (we can't detect this case
        //   reliably, so we treat it as the target platform as well, which isn't a problem in
        //   practice).
        let host = HostPlatform::current(PlatformLibdir::Unavailable(
            PlatformLibdirUnavailable::OLD_SUMMARY,
        ))
        .map_err(|error| RustBuildMetaParseError::UnknownHostPlatform(error.error))?;

        let target = TargetTriple::deserialize_str(summary)?.map(|triple| {
            TargetPlatform::new(
                triple,
                PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
            )
        });

        Ok(Self { host, target })
    }
}

/// A representation of a host platform during a build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPlatform {
    /// The platform.
    pub platform: Platform,

    /// The host libdir.
    pub libdir: PlatformLibdir,
}

impl HostPlatform {
    /// Creates a new `HostPlatform` representing the current platform.
    pub fn current(libdir: PlatformLibdir) -> Result<Self, UnknownHostPlatform> {
        let platform = Platform::current().map_err(|error| UnknownHostPlatform { error })?;
        Ok(Self { platform, libdir })
    }

    /// Converts self to a summary.
    pub fn to_summary(&self) -> HostPlatformSummary {
        HostPlatformSummary {
            platform: self.platform.to_summary(),
            libdir: self.libdir.to_summary(),
        }
    }

    /// Converts a summary to a [`HostPlatform`].
    pub fn from_summary(summary: HostPlatformSummary) -> Result<Self, RustBuildMetaParseError> {
        let platform = summary
            .platform
            .to_platform()
            .map_err(RustBuildMetaParseError::PlatformDeserializeError)?;
        Ok(Self {
            platform,
            libdir: PlatformLibdir::from_summary(summary.libdir),
        })
    }

    fn map_libdir(&self, mapper: &PlatformLibdirMapper) -> Self {
        Self {
            platform: self.platform.clone(),
            libdir: mapper.map(&self.libdir),
        }
    }
}

/// The target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetPlatform {
    /// The target triple: the platform, along with its source and where it was obtained from.
    pub triple: TargetTriple,

    /// The target libdir.
    pub libdir: PlatformLibdir,
}

impl TargetPlatform {
    /// Creates a new [`TargetPlatform`].
    pub fn new(triple: TargetTriple, libdir: PlatformLibdir) -> Self {
        Self { triple, libdir }
    }

    /// Converts self to a summary.
    pub fn to_summary(&self) -> TargetPlatformSummary {
        TargetPlatformSummary {
            platform: self.triple.platform.to_summary(),
            libdir: self.libdir.to_summary(),
        }
    }

    /// Converts a summary to a [`TargetPlatform`].
    pub fn from_summary(summary: TargetPlatformSummary) -> Result<Self, RustBuildMetaParseError> {
        Ok(Self {
            triple: TargetTriple::deserialize(Some(summary.platform))
                .map_err(RustBuildMetaParseError::PlatformDeserializeError)?
                .expect("the input is not None, so the output must not be None"),
            libdir: PlatformLibdir::from_summary(summary.libdir),
        })
    }

    fn map_libdir(&self, mapper: &PlatformLibdirMapper) -> Self {
        Self {
            triple: self.triple.clone(),
            libdir: mapper.map(&self.libdir),
        }
    }
}

/// A platform libdir.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlatformLibdir {
    /// The libdir is known and available.
    Available(Utf8PathBuf),

    /// The libdir is unavailable.
    Unavailable(PlatformLibdirUnavailable),
}

impl PlatformLibdir {
    /// Constructs a new `PlatformLibdir` from a `Utf8PathBuf`.
    pub fn from_path(path: Utf8PathBuf) -> Self {
        Self::Available(path)
    }

    /// Constructs a new `PlatformLibdir` from rustc's standard output.
    ///
    /// None implies that rustc failed, and will be stored as such.
    pub fn from_rustc_stdout(rustc_output: Option<Vec<u8>>) -> Self {
        fn inner(v: Option<Vec<u8>>) -> Result<Utf8PathBuf, PlatformLibdirUnavailable> {
            let v = v.ok_or(PlatformLibdirUnavailable::RUSTC_FAILED)?;

            let s = String::from_utf8(v).map_err(|e| {
                log::debug!("failed to convert the output to a string: {e}");
                PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR
            })?;

            let mut lines = s.lines();
            let Some(out) = lines.next() else {
                log::debug!("empty output");
                return Err(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR);
            };

            let trimmed = out.trim();
            if trimmed.is_empty() {
                log::debug!("empty output");
                return Err(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR);
            }

            // If there's another line, it must be empty.
            for line in lines {
                if !line.trim().is_empty() {
                    log::debug!("unexpected additional output: {line}");
                    return Err(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR);
                }
            }

            Ok(Utf8PathBuf::from(trimmed))
        }

        match inner(rustc_output) {
            Ok(path) => Self::Available(path),
            Err(error) => Self::Unavailable(error),
        }
    }

    /// Constructs a new `PlatformLibdir` from a `PlatformLibdirUnavailable`.
    pub fn from_unavailable(error: PlatformLibdirUnavailable) -> Self {
        Self::Unavailable(error)
    }

    /// Returns self as a path if available.
    pub fn as_path(&self) -> Option<&Utf8Path> {
        match self {
            Self::Available(path) => Some(path),
            Self::Unavailable(_) => None,
        }
    }

    /// Converts self to a summary.
    pub fn to_summary(&self) -> PlatformLibdirSummary {
        match self {
            Self::Available(path) => PlatformLibdirSummary::Available { path: path.clone() },
            Self::Unavailable(reason) => PlatformLibdirSummary::Unavailable {
                reason: reason.clone(),
            },
        }
    }

    /// Converts a summary to a [`PlatformLibdir`].
    pub fn from_summary(summary: PlatformLibdirSummary) -> Self {
        match summary {
            PlatformLibdirSummary::Available { path: libdir } => Self::Available(libdir),
            PlatformLibdirSummary::Unavailable { reason } => Self::Unavailable(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_from_rustc_output_invalid() {
        // None.
        assert_eq!(
            PlatformLibdir::from_rustc_stdout(None),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::RUSTC_FAILED),
        );

        // Empty input.
        assert_eq!(
            PlatformLibdir::from_rustc_stdout(Some(Vec::new())),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR),
        );

        // A single empty line.
        assert_eq!(
            PlatformLibdir::from_rustc_stdout(Some(b"\n".to_vec())),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR),
        );

        // Multiple lines.
        assert_eq!(
            PlatformLibdir::from_rustc_stdout(Some(b"/fake/libdir/1\n/fake/libdir/2\n".to_vec())),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR),
        );
    }

    #[test_case(b"/fake/libdir/22548", "/fake/libdir/22548"; "single line")]
    #[test_case(
        b"\t /fake/libdir\t \n\r",
        "/fake/libdir";
        "with leading or trailing whitespace"
    )]
    #[test_case(
        b"/fake/libdir/1\n\n",
        "/fake/libdir/1";
        "trailing newlines"
    )]
    fn test_read_from_rustc_output_valid(input: &[u8], actual: &str) {
        assert_eq!(
            PlatformLibdir::from_rustc_stdout(Some(input.to_vec())),
            PlatformLibdir::Available(actual.into()),
        );
    }
}
