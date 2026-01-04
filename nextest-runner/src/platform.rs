// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

use crate::{
    RustcCli,
    cargo_config::{CargoTargetArg, TargetTriple},
    errors::{
        DisplayErrorChain, HostPlatformDetectError, RustBuildMetaParseError, TargetTripleError,
    },
    indenter::DisplayIndented,
    reuse_build::{LibdirMapper, PlatformLibdirMapper},
};
use camino::{Utf8Path, Utf8PathBuf};
use nextest_metadata::{
    BuildPlatformsSummary, HostPlatformSummary, PlatformLibdirSummary, PlatformLibdirUnavailable,
    TargetPlatformSummary,
};
pub use target_spec::Platform;
use target_spec::{
    TargetFeatures, errors::RustcVersionVerboseParseError, summaries::PlatformSummary,
};
use tracing::{debug, warn};

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
    pub fn new_with_no_target() -> Result<Self, HostPlatformDetectError> {
        Ok(Self {
            host: HostPlatform {
                // Because this is for testing, we just use the build target
                // rather than `rustc -vV` output.
                platform: Platform::build_target().map_err(|build_target_error| {
                    HostPlatformDetectError::BuildTargetError {
                        build_target_error: Box::new(build_target_error),
                    }
                })?,
                libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::new_const("test")),
            },
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
        let host = HostPlatform {
            // We don't necessarily have `rustc` available, so we use the build
            // target instead.
            platform: Platform::build_target()
                .map_err(RustBuildMetaParseError::DetectBuildTargetError)?,
            libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
        };

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
        let host = HostPlatform {
            // We don't necessarily have `rustc` available, so we use the build
            // target instead.
            platform: Platform::build_target()
                .map_err(RustBuildMetaParseError::DetectBuildTargetError)?,
            libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
        };

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
    /// Creates a new `HostPlatform` representing the current platform by
    /// querying rustc.
    ///
    /// This may fall back to the build target if `rustc -vV` fails.
    pub fn detect(libdir: PlatformLibdir) -> Result<Self, HostPlatformDetectError> {
        let platform = detect_host_platform()?;
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

/// Detect the host platform by using `rustc -vV`, and falling back to the build
/// target.
///
/// Returns an error if both of those methods fail, and produces a warning if
/// `rustc -vV` fails.
fn detect_host_platform() -> Result<Platform, HostPlatformDetectError> {
    // A test-only environment variable to always make the build target a fixed
    // value, or to error out.
    const FORCE_BUILD_TARGET_VAR: &str = "__NEXTEST_FORCE_BUILD_TARGET";

    enum ForceBuildTarget {
        Triple(String),
        Error,
    }

    let force_build_target = match std::env::var(FORCE_BUILD_TARGET_VAR).as_deref() {
        Ok("error") => Some(ForceBuildTarget::Error),
        Ok(triple) => Some(ForceBuildTarget::Triple(triple.to_owned())),
        Err(_) => None,
    };

    let build_target = match force_build_target {
        Some(ForceBuildTarget::Triple(triple)) => Platform::new(triple, TargetFeatures::Unknown),
        Some(ForceBuildTarget::Error) => Err(target_spec::Error::RustcVersionVerboseParse(
            RustcVersionVerboseParseError::MissingHostLine {
                output: format!(
                    "({FORCE_BUILD_TARGET_VAR} set to \"error\", forcibly failing build target detection)\n"
                ),
            },
        )),
        None => Platform::build_target(),
    };

    let rustc_vv = RustcCli::version_verbose()
        .to_expression()
        .stdout_capture()
        .stderr_capture()
        .unchecked();
    match rustc_vv.run() {
        Ok(output) => {
            if output.status.success() {
                // Neither `rustc` nor `cargo` tell us what target features are
                // enabled for the host, so we must use
                // `TargetFeatures::Unknown`.
                match Platform::from_rustc_version_verbose(output.stdout, TargetFeatures::Unknown) {
                    Ok(platform) => Ok(platform),
                    Err(host_platform_error) => {
                        match build_target {
                            Ok(build_target) => {
                                warn!(
                                    "for host platform, parsing `rustc -vV` failed; \
                                     falling back to build target `{}`\n\
                                     - host platform error:\n{}",
                                    build_target.triple().as_str(),
                                    DisplayErrorChain::new_with_initial_indent(
                                        "  ",
                                        host_platform_error
                                    ),
                                );
                                Ok(build_target)
                            }
                            Err(build_target_error) => {
                                // In this case, we can't do anything.
                                Err(HostPlatformDetectError::HostPlatformParseError {
                                    host_platform_error: Box::new(host_platform_error),
                                    build_target_error: Box::new(build_target_error),
                                })
                            }
                        }
                    }
                }
            } else {
                match build_target {
                    Ok(build_target) => {
                        warn!(
                            "for host platform, `rustc -vV` failed with {}; \
                             falling back to build target `{}`\n\
                             - `rustc -vV` stdout:\n{}\n\
                             - `rustc -vV` stderr:\n{}",
                            output.status,
                            build_target.triple().as_str(),
                            DisplayIndented {
                                item: String::from_utf8_lossy(&output.stdout),
                                indent: "  "
                            },
                            DisplayIndented {
                                item: String::from_utf8_lossy(&output.stderr),
                                indent: "  "
                            },
                        );
                        Ok(build_target)
                    }
                    Err(build_target_error) => {
                        // If the build target isn't available either, we
                        // can't do anything.
                        Err(HostPlatformDetectError::RustcVvFailed {
                            status: output.status,
                            stdout: output.stdout,
                            stderr: output.stderr,
                            build_target_error: Box::new(build_target_error),
                        })
                    }
                }
            }
        }
        Err(error) => {
            match build_target {
                Ok(build_target) => {
                    warn!(
                        "for host platform, failed to spawn `rustc -vV`; \
                         falling back to build target `{}`\n\
                         - host platform error:\n{}",
                        build_target.triple().as_str(),
                        DisplayErrorChain::new_with_initial_indent("  ", error),
                    );
                    Ok(build_target)
                }
                Err(build_target_error) => {
                    // If the build target isn't available either, we
                    // can't do anything.
                    Err(HostPlatformDetectError::RustcVvSpawnError {
                        error,
                        build_target_error: Box::new(build_target_error),
                    })
                }
            }
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
                debug!("failed to convert the output to a string: {e}");
                PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR
            })?;

            let mut lines = s.lines();
            let Some(out) = lines.next() else {
                debug!("empty output");
                return Err(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR);
            };

            let trimmed = out.trim();
            if trimmed.is_empty() {
                debug!("empty output");
                return Err(PlatformLibdirUnavailable::RUSTC_OUTPUT_ERROR);
            }

            // If there's another line, it must be empty.
            for line in lines {
                if !line.trim().is_empty() {
                    debug!("unexpected additional output: {line}");
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

/// Detects the host platform for use in tests.
///
/// Prefer this over `Platform::build_target()` in tests to ensure we're testing
/// with the same platform detection logic used in production.
#[cfg(test)]
pub(crate) fn detect_host_platform_for_tests() -> Platform {
    use crate::RustcCli;

    HostPlatform::detect(PlatformLibdir::from_rustc_stdout(
        RustcCli::print_host_libdir().read(),
    ))
    .expect("host platform detection should succeed in tests")
    .platform
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
