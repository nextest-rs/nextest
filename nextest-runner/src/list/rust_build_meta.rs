// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::RustBuildMetaParseError,
    helpers::convert_rel_path_to_main_sep,
    list::{BinaryListState, TestListState},
    platform::{BuildPlatforms, TargetPlatform},
    reuse_build::PathMapper,
};
use camino::Utf8PathBuf;
use itertools::Itertools;
use nextest_metadata::{BuildPlatformsSummary, RustBuildMetaSummary, RustNonTestBinarySummary};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};
use tracing::warn;

/// Rust-related metadata used for builds and test runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustBuildMeta<State> {
    /// The target directory for build artifacts.
    pub target_directory: Utf8PathBuf,

    /// A list of base output directories, relative to the target directory. These directories
    /// and their "deps" subdirectories are added to the dynamic library path.
    pub base_output_directories: BTreeSet<Utf8PathBuf>,

    /// Information about non-test executables, keyed by package ID.
    pub non_test_binaries: BTreeMap<String, BTreeSet<RustNonTestBinarySummary>>,

    /// Build script output directory, relative to the target directory and keyed by package ID.
    /// Only present for workspace packages that have build scripts.
    pub build_script_out_dirs: BTreeMap<String, Utf8PathBuf>,

    /// A list of linked paths, relative to the target directory. These directories are
    /// added to the dynamic library path.
    ///
    /// The values are the package IDs of the libraries that requested the linked paths.
    ///
    /// Note that the serialized metadata only has the paths for now, not the libraries that
    /// requested them. We might consider adding a new field with metadata about that.
    pub linked_paths: BTreeMap<Utf8PathBuf, BTreeSet<String>>,

    /// The build platforms: host and target triple
    pub build_platforms: BuildPlatforms,

    /// A type marker for the state.
    pub state: PhantomData<State>,
}

impl RustBuildMeta<BinaryListState> {
    /// Creates a new [`RustBuildMeta`].
    pub fn new(target_directory: impl Into<Utf8PathBuf>, build_platforms: BuildPlatforms) -> Self {
        Self {
            target_directory: target_directory.into(),
            base_output_directories: BTreeSet::new(),
            non_test_binaries: BTreeMap::new(),
            build_script_out_dirs: BTreeMap::new(),
            linked_paths: BTreeMap::new(),
            state: PhantomData,
            build_platforms,
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
            non_test_binaries: self.non_test_binaries.clone(),
            build_script_out_dirs: self.build_script_out_dirs.clone(),
            linked_paths: self.linked_paths.clone(),
            state: PhantomData,
            build_platforms: self.build_platforms.map_libdir(path_mapper.libdir_mapper()),
        }
    }
}

impl RustBuildMeta<TestListState> {
    /// Creates empty metadata.
    ///
    /// Used for replay and testing where actual build metadata is not needed.
    pub fn empty() -> Self {
        Self {
            target_directory: Utf8PathBuf::new(),
            base_output_directories: BTreeSet::new(),
            non_test_binaries: BTreeMap::new(),
            build_script_out_dirs: BTreeMap::new(),
            linked_paths: BTreeMap::new(),
            state: PhantomData,
            build_platforms: BuildPlatforms::new_with_no_target().unwrap(),
        }
    }

    /// Returns the dynamic library paths corresponding to this metadata.
    ///
    /// [See this Cargo documentation for
    /// more.](https://doc.rust-lang.org/cargo/reference/environment-variables.html#dynamic-library-paths)
    ///
    /// These paths are prepended to the dynamic library environment variable for the current
    /// platform (e.g. `LD_LIBRARY_PATH` on non-Apple Unix platforms).
    pub fn dylib_paths(&self) -> Vec<Utf8PathBuf> {
        // Add rust libdirs to the path if available, so we can run test binaries that depend on
        // libstd.
        //
        // We could be smarter here and only add the host libdir for host binaries and the target
        // libdir for target binaries, but it's simpler to just add both for now.
        let libdirs = self
            .build_platforms
            .host
            .libdir
            .as_path()
            .into_iter()
            .chain(
                self.build_platforms
                    .target
                    .as_ref()
                    .and_then(|target| target.libdir.as_path()),
            )
            .map(|libdir| libdir.to_path_buf())
            .collect::<Vec<_>>();
        if libdirs.is_empty() {
            warn!("failed to detect the rustc libdir, may fail to list or run tests");
        }

        // Cargo puts linked paths before base output directories.
        self.linked_paths
            .keys()
            .filter_map(|rel_path| {
                let join_path = self
                    .target_directory
                    .join(convert_rel_path_to_main_sep(rel_path));
                // Only add the directory to the path if it exists on disk.
                join_path.exists().then_some(join_path)
            })
            .chain(self.base_output_directories.iter().flat_map(|base_output| {
                let abs_base = self
                    .target_directory
                    .join(convert_rel_path_to_main_sep(base_output));
                let with_deps = abs_base.join("deps");
                // This is the order paths are added in by Cargo.
                [with_deps, abs_base]
            }))
            .chain(libdirs)
            .unique()
            .collect()
    }
}

impl<State> RustBuildMeta<State> {
    /// Creates a `RustBuildMeta` from a serializable summary.
    pub fn from_summary(summary: RustBuildMetaSummary) -> Result<Self, RustBuildMetaParseError> {
        let build_platforms = if let Some(summary) = summary.platforms {
            BuildPlatforms::from_summary(summary.clone())?
        } else if let Some(summary) = summary.target_platforms.first() {
            // Compatibility with metadata generated by older versions of nextest.
            BuildPlatforms::from_target_summary(summary.clone())?
        } else {
            // Compatibility with metadata generated by older versions of nextest.
            BuildPlatforms::from_summary_str(summary.target_platform.clone())?
        };

        Ok(Self {
            target_directory: summary.target_directory,
            base_output_directories: summary.base_output_directories,
            build_script_out_dirs: summary.build_script_out_dirs,
            non_test_binaries: summary.non_test_binaries,
            linked_paths: summary
                .linked_paths
                .into_iter()
                .map(|linked_path| (linked_path, BTreeSet::new()))
                .collect(),
            state: PhantomData,
            build_platforms,
        })
    }

    /// Converts self to a serializable form.
    pub fn to_summary(&self) -> RustBuildMetaSummary {
        RustBuildMetaSummary {
            target_directory: self.target_directory.clone(),
            base_output_directories: self.base_output_directories.clone(),
            non_test_binaries: self.non_test_binaries.clone(),
            build_script_out_dirs: self.build_script_out_dirs.clone(),
            linked_paths: self.linked_paths.keys().cloned().collect(),
            target_platform: self.build_platforms.to_summary_str(),
            target_platforms: vec![self.build_platforms.to_target_or_host_summary()],
            // TODO: support multiple --target options
            platforms: Some(BuildPlatformsSummary {
                host: self.build_platforms.host.to_summary(),
                targets: self
                    .build_platforms
                    .target
                    .as_ref()
                    .into_iter()
                    .map(TargetPlatform::to_summary)
                    .collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cargo_config::TargetTriple,
        platform::{BuildPlatforms, HostPlatform, PlatformLibdir, TargetPlatform},
    };
    use nextest_metadata::{
        BuildPlatformsSummary, HostPlatformSummary, PlatformLibdirSummary,
        PlatformLibdirUnavailable,
    };
    use target_spec::{Platform, summaries::PlatformSummary};
    use test_case::test_case;

    impl Default for RustBuildMeta<BinaryListState> {
        fn default() -> Self {
            RustBuildMeta::<BinaryListState>::new(
                Utf8PathBuf::default(),
                BuildPlatforms::new_with_no_target()
                    .expect("creating BuildPlatforms without target triple should succeed"),
            )
        }
    }

    fn x86_64_pc_windows_msvc_triple() -> TargetTriple {
        TargetTriple::deserialize_str(Some("x86_64-pc-windows-msvc".to_owned()))
            .expect("creating TargetTriple should succeed")
            .expect("the output of deserialize_str shouldn't be None")
    }

    fn host_current() -> HostPlatform {
        HostPlatform {
            platform: Platform::build_target()
                .expect("should detect the build target successfully"),
            libdir: PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
        }
    }

    fn host_current_with_libdir(libdir: &str) -> HostPlatform {
        HostPlatform {
            platform: Platform::build_target()
                .expect("should detect the build target successfully"),
            libdir: PlatformLibdir::Available(libdir.into()),
        }
    }

    fn host_not_current_with_libdir(libdir: &str) -> HostPlatform {
        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                let triple = TargetTriple::x86_64_unknown_linux_gnu();
            } else {
                let triple = x86_64_pc_windows_msvc_triple();
            }
        };

        HostPlatform {
            platform: triple.platform,
            libdir: PlatformLibdir::Available(libdir.into()),
        }
    }

    fn target_linux() -> TargetPlatform {
        TargetPlatform::new(
            TargetTriple::x86_64_unknown_linux_gnu(),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
        )
    }

    fn target_linux_with_libdir(libdir: &str) -> TargetPlatform {
        TargetPlatform::new(
            TargetTriple::x86_64_unknown_linux_gnu(),
            PlatformLibdir::Available(libdir.into()),
        )
    }

    fn target_windows() -> TargetPlatform {
        TargetPlatform::new(
            x86_64_pc_windows_msvc_triple(),
            PlatformLibdir::Unavailable(PlatformLibdirUnavailable::OLD_SUMMARY),
        )
    }

    #[test_case(RustBuildMetaSummary {
        ..Default::default()
    }, RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current(),
            target: None,
        },
        ..Default::default()
    }; "no target platforms")]
    #[test_case(RustBuildMetaSummary {
        target_platform: Some("x86_64-unknown-linux-gnu".to_owned()),
        ..Default::default()
    }, RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current(),
            target: Some(target_linux()),
        },
        ..Default::default()
    }; "only target platform field")]
    #[test_case(RustBuildMetaSummary {
        target_platform: Some("x86_64-unknown-linux-gnu".to_owned()),
        // target_platforms should be preferred over target_platform
        target_platforms: vec![PlatformSummary::new("x86_64-pc-windows-msvc")],
        ..Default::default()
    }, RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current(),
            target: Some(target_windows()),
        },
        ..Default::default()
    }; "target platform and target platforms field")]
    #[test_case(RustBuildMetaSummary {
        target_platform: Some("aarch64-unknown-linux-gnu".to_owned()),
        target_platforms: vec![PlatformSummary::new("x86_64-pc-windows-msvc")],
        // platforms should be preferred over both target_platform and target_platforms
        platforms: Some(BuildPlatformsSummary {
            host: host_not_current_with_libdir("/fake/test/libdir/281").to_summary(),
            targets: vec![target_linux_with_libdir("/fake/test/libdir/837").to_summary()],
        }),
        ..Default::default()
    }, RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_not_current_with_libdir("/fake/test/libdir/281"),
            target: Some(target_linux_with_libdir("/fake/test/libdir/837")),
        },
        ..Default::default()
    }; "target platform and target platforms and platforms field")]
    #[test_case(RustBuildMetaSummary {
        platforms: Some(BuildPlatformsSummary {
            host: host_current().to_summary(),
            targets: vec![],
        }),
        ..Default::default()
    }, RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current(),
            target: None,
        },
        ..Default::default()
    }; "platforms with zero targets")]
    fn test_from_summary(summary: RustBuildMetaSummary, expected: RustBuildMeta<BinaryListState>) {
        let actual = RustBuildMeta::<BinaryListState>::from_summary(summary)
            .expect("RustBuildMeta should deserialize from summary with success.");
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_from_summary_error_multiple_targets() {
        let summary = RustBuildMetaSummary {
            platforms: Some(BuildPlatformsSummary {
                host: host_current().to_summary(),
                targets: vec![target_linux().to_summary(), target_windows().to_summary()],
            }),
            ..Default::default()
        };
        let actual = RustBuildMeta::<BinaryListState>::from_summary(summary);
        assert!(
            matches!(actual, Err(RustBuildMetaParseError::Unsupported { .. })),
            "Expect the parse result to be an error of RustBuildMetaParseError::Unsupported, actual {actual:?}"
        );
    }

    #[test]
    fn test_from_summary_error_invalid_host_platform_summary() {
        let summary = RustBuildMetaSummary {
            platforms: Some(BuildPlatformsSummary {
                host: HostPlatformSummary {
                    platform: PlatformSummary::new("invalid-platform-triple"),
                    libdir: PlatformLibdirSummary::Unavailable {
                        reason: PlatformLibdirUnavailable::RUSTC_FAILED,
                    },
                },
                targets: vec![],
            }),
            ..Default::default()
        };
        let actual = RustBuildMeta::<BinaryListState>::from_summary(summary);
        actual.expect_err("parse result should be an error");
    }

    #[test_case(RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current(),
            target: None,
        },
        ..Default::default()
    }, RustBuildMetaSummary {
        target_platform: None,
        target_platforms: vec![host_current().to_summary().platform],
        platforms: Some(BuildPlatformsSummary {
            host: host_current().to_summary(),
            targets: vec![],
        }),
        ..Default::default()
    }; "build platforms without target")]
    #[test_case(RustBuildMeta::<BinaryListState> {
        build_platforms: BuildPlatforms {
            host: host_current_with_libdir("/fake/test/libdir/736"),
            target: Some(target_linux_with_libdir("/fake/test/libdir/873")),
        },
        ..Default::default()
    }, RustBuildMetaSummary {
        target_platform: Some(
            target_linux_with_libdir("/fake/test/libdir/873")
                .triple
                .platform
                .triple_str()
                .to_owned(),
        ),
        target_platforms: vec![target_linux_with_libdir("/fake/test/libdir/873").triple.platform.to_summary()],
        platforms: Some(BuildPlatformsSummary {
            host: host_current_with_libdir("/fake/test/libdir/736").to_summary(),
            targets: vec![target_linux_with_libdir("/fake/test/libdir/873").to_summary()],
        }),
        ..Default::default()
    }; "build platforms with target")]
    fn test_to_summary(meta: RustBuildMeta<BinaryListState>, expected: RustBuildMetaSummary) {
        let actual = meta.to_summary();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_dylib_paths_should_include_rustc_dir() {
        let host_libdir = Utf8PathBuf::from("/fake/rustc/host/libdir");
        let target_libdir = Utf8PathBuf::from("/fake/rustc/target/libdir");

        let rust_build_meta = RustBuildMeta {
            build_platforms: BuildPlatforms {
                host: host_current_with_libdir(host_libdir.as_ref()),
                target: Some(TargetPlatform::new(
                    TargetTriple::x86_64_unknown_linux_gnu(),
                    PlatformLibdir::Available(target_libdir.clone()),
                )),
            },
            ..RustBuildMeta::empty()
        };
        let dylib_paths = rust_build_meta.dylib_paths();

        assert!(
            dylib_paths.contains(&host_libdir),
            "{dylib_paths:?} should contain {host_libdir}"
        );
        assert!(
            dylib_paths.contains(&target_libdir),
            "{dylib_paths:?} should contain {target_libdir}"
        );
    }

    #[test]
    fn test_dylib_paths_should_not_contain_duplicate_paths() {
        let tmpdir = camino_tempfile::tempdir().expect("should create temp dir successfully");
        let host_libdir = tmpdir.path().to_path_buf();
        let target_libdir = host_libdir.clone();
        let fake_target_dir = tmpdir
            .path()
            .parent()
            .expect("tmp directory should have a parent");
        let tmpdir_dirname = tmpdir
            .path()
            .file_name()
            .expect("tmp directory should have a file name");

        let rust_build_meta = RustBuildMeta {
            target_directory: fake_target_dir.to_path_buf(),
            linked_paths: [(Utf8PathBuf::from(tmpdir_dirname), Default::default())].into(),
            base_output_directories: [Utf8PathBuf::from(tmpdir_dirname)].into(),
            build_platforms: BuildPlatforms {
                host: host_current_with_libdir(host_libdir.as_ref()),
                target: Some(TargetPlatform::new(
                    TargetTriple::x86_64_unknown_linux_gnu(),
                    PlatformLibdir::Available(target_libdir.clone()),
                )),
            },
            ..RustBuildMeta::empty()
        };
        let dylib_paths = rust_build_meta.dylib_paths();

        assert!(
            dylib_paths.clone().into_iter().all_unique(),
            "{dylib_paths:?} should not contain duplicate paths"
        );
    }
}
