// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Converting Buck2 test targets into nextest's types.
//!
//! Nextest models a test suite as a binary belonging to a package, which is a
//! Cargo shape. Buck2's equivalent is a configured target label, so each target
//! becomes both a [`RustTestBinary`] and the [`PackageInfo`] it refers to.

use crate::spec::Buck2TestTarget;
use camino::{Utf8Path, Utf8PathBuf};
use guppy::PackageId;
use iddqd::IdOrdMap;
use nextest_metadata::{BuildPlatform, RustBinaryId, RustTestBinaryKind};
use nextest_runner::{
    list::{
        BinaryList, BinaryListState, PackageInfo, RustBuildMeta, RustTestBinary,
        TestBinaryInvocation,
    },
    platform::BuildPlatforms,
};
use semver::Version;

/// The name Buck2 uses for its build files.
///
/// `PackageInfo::manifest_path` points at the file the target was declared in,
/// the way a Cargo package's manifest path points at its `Cargo.toml`.
const BUCK_FILE_NAME: &str = "BUCK";

/// A [`BinaryList`] plus the packages its binaries refer to.
///
/// The two travel together because `RustTestArtifact::from_binary_list` looks
/// packages up by ID, and the map must outlive the resulting `TestList`.
#[derive(Debug)]
pub struct Buck2BinaryList {
    /// The binaries to run.
    pub binary_list: BinaryList,

    /// One entry per binary, keyed by the target label.
    pub packages: IdOrdMap<PackageInfo>,
}

/// Converts validated Buck2 targets into a binary list.
///
/// `project_root` is the directory relative paths in the spec resolve against,
/// i.e. the Buck2 project root.
pub fn to_binary_list(
    targets: &[Buck2TestTarget],
    project_root: &Utf8Path,
    build_platforms: BuildPlatforms,
) -> Buck2BinaryList {
    let mut rust_binaries = Vec::with_capacity(targets.len());
    let mut packages = IdOrdMap::new();

    for target in targets {
        let package_id = PackageId::new(target.label.clone());
        let package_dir = package_dir(project_root, &target.target.package);

        // Buck2 targets are identified by their label. Using it as the binary ID
        // keeps nextest's output and `binary_id()` filtersets speaking Buck2's
        // vocabulary rather than a synthesized Cargo-style name.
        let binary_id = RustBinaryId::new(&target.label);

        rust_binaries.push(RustTestBinary {
            id: binary_id,
            path: resolve_path(project_root, &target.program),
            package_id: target.label.clone(),
            // Buck2 has no lib/bin/test distinction of Cargo's sort. Reporting
            // these as `test` keeps `kind(test)` filtersets meaningful.
            kind: RustTestBinaryKind::TEST,
            name: target.target.target.clone(),
            build_platform: BuildPlatform::Target,
            invocation: TestBinaryInvocation {
                leading_args: target.leading_args.clone(),
                env: target.env.clone(),
                // Buck2 states the working directory when it is asked over
                // gRPC; a spec file does not carry one, so fall back to the
                // target's package directory. Either way it is stated here
                // rather than left to fall out of `manifest_path`, which
                // describes where the target was declared -- not where it runs.
                cwd: Some(target.cwd.clone().unwrap_or_else(|| package_dir.clone())),
            },
        });

        // `insert_unique` would panic on a duplicate; `parse_spec` already
        // rejects duplicate labels, so `insert_overwrite` is unreachable in
        // practice and merely avoids a panic path.
        packages.insert_overwrite(PackageInfo {
            id: package_id,
            name: target.target.target.clone(),
            version: Version::new(0, 0, 0),
            authors: Vec::new(),
            description: None,
            homepage: None,
            license: None,
            license_file: None,
            repository: None,
            minimum_rust_version: None,
            manifest_path: package_dir.join(BUCK_FILE_NAME),
        });
    }

    // Sort by binary ID, as Cargo's `BinaryListBuilder::finish` does. This is
    // the authoritative ordering: `RustBinaryId`'s `Ord` is component-based
    // rather than bytewise, so it does not necessarily agree with the label
    // ordering `parse_spec` applies.
    rust_binaries.sort_by(|a, b| a.id.cmp(&b.id));

    // Buck2 does not uplift artifacts or run Cargo build scripts, so the build
    // metadata is empty apart from the platforms. `dylib_paths()` then yields
    // just the rustc libdirs, which is what a Buck2-built test binary needs.
    let rust_build_meta: RustBuildMeta<BinaryListState> =
        RustBuildMeta::new(project_root, project_root, build_platforms);

    Buck2BinaryList {
        binary_list: BinaryList {
            rust_build_meta,
            rust_binaries,
        },
        packages,
    }
}

/// Returns the directory a Buck2 package lives in.
///
/// The root package's path is the empty string. Joining that on would leave a
/// trailing separator, which is harmless to run in but shows up verbatim in
/// nextest's output, so it is special-cased.
fn package_dir(project_root: &Utf8Path, package: &str) -> Utf8PathBuf {
    if package.is_empty() {
        project_root.to_owned()
    } else {
        project_root.join(package)
    }
}

/// Resolves a possibly-relative path from the spec against the project root.
fn resolve_path(project_root: &Utf8Path, path: &str) -> Utf8PathBuf {
    let path = Utf8Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        project_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::parse_spec;
    use indoc::indoc;
    fn fake_build_platforms() -> BuildPlatforms {
        BuildPlatforms::new_with_no_target().expect("host platform is detectable")
    }

    static SPEC: &str = indoc! {r#"
        {
          "targets": [
            {
              "target": {"cell": "fbcode", "package": "app/tests", "target": "zebra"},
              "test_type": "rust",
              "command": ["buck-out/gen/zebra", "--flag"],
              "env": {"KEY": "value"}
            },
            {
              "target": {"cell": "fbcode", "package": "app", "target": "aardvark"},
              "test_type": "rust",
              "command": ["/abs/path/aardvark"]
            }
          ]
        }
    "#};

    fn convert() -> Buck2BinaryList {
        let targets = parse_spec(SPEC, "test input").expect("valid spec");
        to_binary_list(&targets, Utf8Path::new("/project"), fake_build_platforms())
    }

    /// The order is whatever `RustBinaryId`'s `Ord` gives; what matters is that
    /// it is deterministic regardless of the order Buck2 emitted targets in.
    ///
    /// Note that `RustBinaryId` sorts by its Cargo-shaped components
    /// (`package::kind/target`). A Buck2 label has no `::`, so the whole label
    /// becomes the package component and the comparison is bytewise -- which
    /// puts `/` (0x2f) before `:` (0x3a), i.e. `app/tests:zebra` before
    /// `app:aardvark`.
    #[test]
    fn binaries_are_sorted_by_id() {
        let converted = convert();
        let ids: Vec<_> = converted
            .binary_list
            .rust_binaries
            .iter()
            .map(|bin| bin.id.as_str())
            .collect();
        assert_eq!(ids, vec!["fbcode//app/tests:zebra", "fbcode//app:aardvark"]);

        // Reversing the input must not change the output.
        let reversed: Vec<_> = {
            let mut targets = parse_spec(SPEC, "test input").expect("valid spec");
            targets.reverse();
            let converted =
                to_binary_list(&targets, Utf8Path::new("/project"), fake_build_platforms());
            converted
                .binary_list
                .rust_binaries
                .iter()
                .map(|bin| bin.id.to_string())
                .collect()
        };
        assert_eq!(reversed, ids, "order does not depend on input order");
    }

    #[test]
    fn relative_paths_resolve_against_the_project_root() {
        let converted = convert();
        let zebra = converted
            .binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == "fbcode//app/tests:zebra")
            .expect("zebra is present");
        assert_eq!(zebra.path, "/project/buck-out/gen/zebra");
    }

    #[test]
    fn absolute_paths_are_left_alone() {
        let converted = convert();
        let aardvark = converted
            .binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == "fbcode//app:aardvark")
            .expect("aardvark is present");
        assert_eq!(aardvark.path, "/abs/path/aardvark");
    }

    #[test]
    fn command_args_and_env_land_in_the_invocation() {
        let converted = convert();
        let zebra = converted
            .binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == "fbcode//app/tests:zebra")
            .expect("zebra is present");
        assert_eq!(zebra.invocation.leading_args, vec!["--flag"]);
        assert_eq!(
            zebra.invocation.env.get("KEY").map(String::as_str),
            Some("value")
        );

        let aardvark = converted
            .binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == "fbcode//app:aardvark")
            .expect("aardvark is present");
        // The working directory is always set, so the invocation is never
        // empty; a bare command contributes nothing else to it.
        assert!(aardvark.invocation.leading_args.is_empty());
        assert!(aardvark.invocation.env.is_empty());
    }

    /// Every binary must have a matching package entry, or building the test
    /// list fails with a lookup error.
    #[test]
    fn every_binary_has_a_package() {
        let converted = convert();
        for binary in &converted.binary_list.rust_binaries {
            let package_id = PackageId::new(binary.package_id.clone());
            let package = converted
                .packages
                .get(&package_id)
                .unwrap_or_else(|| panic!("package for {} is present", binary.id));
            assert_eq!(package.name, binary.name);
        }
    }

    /// Tests run in the target's package directory, and the invocation says so
    /// outright rather than leaving it to be derived from the manifest path.
    #[test]
    fn cwd_is_the_target_package_directory() {
        let converted = convert();
        let zebra = converted
            .binary_list
            .rust_binaries
            .iter()
            .find(|bin| bin.id.as_str() == "fbcode//app/tests:zebra")
            .expect("zebra is present");
        assert_eq!(
            zebra.invocation.cwd.as_deref(),
            Some(Utf8Path::new("/project/app/tests"))
        );

        // The manifest path independently points at the target's build file.
        let package_id = PackageId::new("fbcode//app/tests:zebra".to_owned());
        let package = converted.packages.get(&package_id).expect("present");
        assert_eq!(package.manifest_path, "/project/app/tests/BUCK");
    }

    /// A target in the root package runs in the project root.
    #[test]
    fn root_package_cwd_is_the_project_root() {
        let targets = parse_spec(
            indoc! {r#"
                {
                  "targets": [
                    {
                      "target": {"cell": "root", "package": "", "target": "demo"},
                      "test_type": "rust",
                      "command": ["buck-out/demo"]
                    }
                  ]
                }
            "#},
            "test input",
        )
        .expect("valid spec");
        let converted = to_binary_list(&targets, Utf8Path::new("/project"), fake_build_platforms());
        let binary = &converted.binary_list.rust_binaries[0];
        assert_eq!(
            binary.invocation.cwd.as_deref(),
            Some(Utf8Path::new("/project")),
            "no trailing separator for the root package"
        );

        let package_id = PackageId::new("root//:demo".to_owned());
        let package = converted.packages.get(&package_id).expect("present");
        assert_eq!(package.manifest_path, "/project/BUCK");
    }
}
