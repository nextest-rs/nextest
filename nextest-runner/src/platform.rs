// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-related data structures.

use crate::{
    cargo_config::{CargoTargetArg, TargetTriple},
    errors::{RustBuildMetaParseError, TargetTripleError, UnknownHostPlatform},
};
use camino::Utf8PathBuf;
use nextest_metadata::{BuildPlatformsSummary, HostPlatformSummary, TargetPlatformSummary};
use std::io;
use target_spec::summaries::PlatformSummary;
pub use target_spec::Platform;

fn read_first_line_as_path(reader: impl io::BufRead) -> Option<Utf8PathBuf> {
    // We will print warn logs later when we are adding the path to the dynamic linker search paths,
    // so we don't print the warn log here to avoid spammy log.
    match reader.lines().next() {
        Some(Ok(line)) => {
            let original_line = line.as_str();
            let line = line.trim();
            if line.is_empty() {
                log::debug!("empty input found: {:#?}", original_line);
                return None;
            }
            Some(Utf8PathBuf::from(line))
        }
        Some(Err(e)) => {
            log::debug!("failed to read the input: {:#?}", e);
            None
        }
        None => {
            log::debug!("empty input");
            None
        }
    }
}

/// The target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatformsTarget {
    /// The target triplet, which consists of machine, vendor and OS.
    pub triple: TargetTriple,

    /// The target libdir.
    pub libdir: Option<Utf8PathBuf>,
}

impl BuildPlatformsTarget {
    /// Creates a new [`BuildPlatformsTarget`] and set the [`Self::triple`] to the imput `triple`.
    pub fn new(triple: TargetTriple) -> Self {
        Self {
            triple,
            libdir: None,
        }
    }

    /// Try to parse the rustc output and set [`Self::libdir`]. If the we fail to parse the input
    /// [`Self::libdir`] will be set to [`None`].
    ///
    /// Used to set the dynamic linker search path when running the test executables.
    pub fn set_libdir_from_rustc_output(&mut self, reader: impl io::BufRead) {
        self.libdir = read_first_line_as_path(reader);
    }
}

/// A representation of host and target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildPlatforms {
    /// The host platform.
    pub host: Platform,

    /// The host libdir.
    pub host_libdir: Option<Utf8PathBuf>,

    /// The target platform, if specified.
    pub target: Option<BuildPlatformsTarget>,
}

impl BuildPlatforms {
    /// Creates a new [`BuildPlatforms`].
    ///
    /// Returns an error if the host platform could not be determined.
    pub fn new() -> Result<Self, UnknownHostPlatform> {
        let host = Platform::current().map_err(|error| UnknownHostPlatform { error })?;
        Ok(Self {
            host,
            host_libdir: None,
            target: None,
        })
    }

    /// Try to parse the rustc output and set [`Self::host_libdir`]. If the we fail to parse the
    /// input [`Self::host_libdir`] will be set to [`None`].
    ///
    /// Used to set the dynamic linker search path when running the test executables.
    pub fn set_host_libdir_from_rustc_output(&mut self, reader: impl io::BufRead) {
        self.host_libdir = read_first_line_as_path(reader);
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
            build_platforms.target = Some(BuildPlatformsTarget::new(triple));
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
            libdir: self.host_libdir.clone(),
        }
    }
}

impl ToSummary<TargetPlatformSummary> for BuildPlatformsTarget {
    fn to_summary(&self) -> TargetPlatformSummary {
        TargetPlatformSummary {
            platform: self.triple.platform.to_summary(),
            libdir: self.libdir.clone(),
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
            build_platforms.target = Some(BuildPlatformsTarget::new(triple));
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
            host_libdir: summary.host.libdir,
            target: {
                if summary.targets.len() > 1 {
                    return Err(RustBuildMetaParseError::Unsupported {
                        message: "multiple build targets is not supported".to_owned(),
                    });
                }
                summary
                    .targets
                    .first()
                    .map(|target| {
                        Ok::<_, Self::Error>(BuildPlatformsTarget {
                            triple: TargetTriple::deserialize(Some(target.platform.clone()))?
                                .expect("the input is not None, so the output must not be None"),
                            libdir: target.libdir.clone(),
                        })
                    })
                    .transpose()?
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::io::Cursor;
    use test_case::test_case;

    #[test]
    fn test_read_from_rustc_output_failed() {
        struct ReadMock;
        impl io::Read for ReadMock {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("test error"))
            }
        }

        let reader = io::BufReader::new(ReadMock);
        let mut build_platforms = BuildPlatforms::new().expect("default ctor should succeed");
        build_platforms.set_host_libdir_from_rustc_output(reader);
        assert_eq!(build_platforms.host_libdir, None);

        let reader = io::BufReader::new(ReadMock);
        let mut build_platforms_target =
            BuildPlatformsTarget::new(TargetTriple::x86_64_unknown_linux_gnu());
        build_platforms_target.set_libdir_from_rustc_output(reader);
        assert_eq!(build_platforms_target.libdir, None);
    }

    #[test]
    fn test_read_from_rustc_output_empty_input() {
        let mut build_platforms = BuildPlatforms::new().expect("default ctor should succeed");
        build_platforms.set_host_libdir_from_rustc_output(io::empty());
        assert_eq!(build_platforms.host_libdir, None);

        let mut build_platforms_target =
            BuildPlatformsTarget::new(TargetTriple::x86_64_unknown_linux_gnu());
        build_platforms_target.set_libdir_from_rustc_output(io::empty());
        assert_eq!(build_platforms_target.libdir, None);
    }

    #[test_case("/fake/libdir/22548", Some("/fake/libdir/22548"); "single line")]
    #[test_case(
        indoc! {r#"
            /fake/libdir/1
            /fake/libdir/2
        "#},
        Some("/fake/libdir/1");
        "multiple lines"
    )]
    #[test_case(
        "\t /fake/libdir\t \n\r",
        Some("/fake/libdir");
        "with leading or trailing whitespace"
    )]
    #[test_case("\t \r\n", None; "empty content with whitespaces")]
    fn test_read_from_rustc_output_not_empty_input(input: &str, actual: Option<&str>) {
        let mut build_platforms = BuildPlatforms::new().expect("default ctor should succeed");
        build_platforms.set_host_libdir_from_rustc_output(Cursor::new(input));
        assert_eq!(build_platforms.host_libdir, actual.map(Utf8PathBuf::from));

        let mut build_platforms_target =
            BuildPlatformsTarget::new(TargetTriple::x86_64_unknown_linux_gnu());
        build_platforms_target.set_libdir_from_rustc_output(Cursor::new(input));
        assert_eq!(build_platforms_target.libdir, actual.map(Utf8PathBuf::from));
    }

    #[test]
    fn test_build_platform_new() {
        let build_platforms = BuildPlatforms::new().expect("default ctor should succeed");
        assert_eq!(
            build_platforms,
            BuildPlatforms {
                host: Platform::current().expect("should detect the current platform successfully"),
                host_libdir: None,
                target: None,
            }
        );
    }

    #[test]
    fn test_build_platforms_target_new() {
        let triple = TargetTriple::x86_64_unknown_linux_gnu();
        let build_platforms_target = BuildPlatformsTarget::new(triple.clone());
        assert_eq!(
            build_platforms_target,
            BuildPlatformsTarget {
                triple,
                libdir: None,
            }
        );
    }
}
