// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Best-effort detection of custom test harnesses for listing diagnostics.

use camino::Utf8Path;
use nextest_metadata::RustTestBinaryKind;
use serde::Deserialize;
use std::fs;

pub(super) fn is_custom_harness(
    cwd: &Utf8Path,
    package_name: &str,
    binary_name: &str,
    kind: &RustTestBinaryKind,
) -> bool {
    // Use the execution directory rather than the original manifest path, since
    // source paths can be remapped when reusing builds.
    let Ok(manifest) = fs::read_to_string(cwd.join("Cargo.toml")) else {
        return false;
    };
    manifest_has_custom_harness(&manifest, package_name, binary_name, kind)
}

fn manifest_has_custom_harness(
    manifest: &str,
    package_name: &str,
    binary_name: &str,
    kind: &RustTestBinaryKind,
) -> bool {
    let Ok(manifest) = toml::from_str::<Manifest>(manifest) else {
        return false;
    };
    // A remapped or modified manifest might describe a different package.
    if manifest
        .package
        .or(manifest.project)
        .as_ref()
        .map(|package| package.name.as_str())
        != Some(package_name)
    {
        return false;
    }

    if *kind == RustTestBinaryKind::LIB || *kind == RustTestBinaryKind::PROC_MACRO {
        manifest.lib.is_some_and(|target| {
            let name = target
                .name
                .unwrap_or_else(|| package_name.replace('-', "_"));
            name == binary_name && target.harness == Some(false)
        })
    } else {
        let targets = match kind.as_str() {
            "bin" => manifest.bin,
            "test" => manifest.test,
            "bench" => manifest.bench,
            "example" => manifest.example,
            _ => return false,
        };
        // Cargo requires an explicit name for non-library target tables. Targets
        // discovered from paths use the default harness.
        let mut matching_targets = targets
            .iter()
            .filter(|target| target.name.as_deref() == Some(binary_name));
        matching_targets.next().is_some_and(|target| {
            target.harness == Some(false) && matching_targets.next().is_none()
        })
    }
}

#[derive(Deserialize)]
struct Manifest {
    package: Option<Package>,
    project: Option<Package>,
    lib: Option<Target>,
    #[serde(default)]
    bin: Vec<Target>,
    #[serde(default)]
    test: Vec<Target>,
    #[serde(default)]
    bench: Vec<Target>,
    #[serde(default)]
    example: Vec<Target>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

#[derive(Deserialize)]
struct Target {
    name: Option<String>,
    harness: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino_tempfile::Utf8TempDir;
    use test_case::test_case;

    fn detect(
        manifest: &str,
        package_name: &str,
        binary_name: &str,
        kind: &RustTestBinaryKind,
    ) -> bool {
        let manifest = format!("[package]\nname = '{package_name}'\n{manifest}");
        manifest_has_custom_harness(&manifest, package_name, binary_name, kind)
    }

    #[test_case("lib", RustTestBinaryKind::LIB)]
    #[test_case("lib", RustTestBinaryKind::PROC_MACRO)]
    #[test_case("bin", RustTestBinaryKind::BIN)]
    #[test_case("test", RustTestBinaryKind::TEST)]
    #[test_case("bench", RustTestBinaryKind::BENCH)]
    #[test_case("example", RustTestBinaryKind::EXAMPLE)]
    fn test_target_kinds(table: &str, kind: RustTestBinaryKind) {
        let header = if table == "lib" {
            "[lib]".to_owned()
        } else {
            format!("[[{table}]]")
        };
        for (setting, expected) in [
            ("harness = false", true),
            ("harness = true", false),
            ("", false),
        ] {
            let manifest =
                format!("{header}\nname = 'custom_name'\n{setting}\npath = 'arbitrary/main.rs'");
            assert_eq!(
                detect(&manifest, "package-name", "custom_name", &kind),
                expected,
                "{manifest}"
            );
            assert!(!detect(&manifest, "package-name", "different-name", &kind,));
        }
    }

    #[test]
    fn test_default_library_name() {
        for kind in [RustTestBinaryKind::LIB, RustTestBinaryKind::PROC_MACRO] {
            assert!(detect(
                "[lib]\nharness = false",
                "package-name",
                "package_name",
                &kind,
            ));
            assert!(!detect(
                "[lib]\nharness = false",
                "package-name",
                "package-name",
                &kind,
            ));
        }
    }

    #[test]
    fn test_non_library_names_are_not_normalized() {
        assert!(detect(
            "[[bin]]\nname = 'custom-name'\nharness = false",
            "package-name",
            "custom-name",
            &RustTestBinaryKind::BIN,
        ));
        assert!(!detect(
            "[[bin]]\nname = 'custom-name'\nharness = false",
            "package-name",
            "custom_name",
            &RustTestBinaryKind::BIN,
        ));
    }

    #[test]
    fn test_manifest_read_failures() {
        let dir = Utf8TempDir::new().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        let detect =
            || is_custom_harness(dir.path(), "package", "custom", &RustTestBinaryKind::TEST);
        assert!(!detect(), "a missing manifest does not produce a hint");

        fs::write(&manifest, b"\xff").unwrap();
        assert!(!detect(), "a non-UTF-8 manifest does not produce a hint");

        fs::write(
            &manifest,
            "[package]\nname = 'package'\n[[test]]\nname = 'custom'\nharness = false",
        )
        .unwrap();
        assert!(detect());

        fs::remove_file(&manifest).unwrap();
        fs::create_dir(&manifest).unwrap();
        assert!(!detect(), "an unreadable manifest does not produce a hint");
    }

    #[test]
    fn test_target_identity() {
        let manifest = "\
            [lib]\nname = 'shared'\nharness = true\n\
            [[test]]\nname = 'other'\nharness = false\n\
            [[test]]\nname = 'shared'\nharness = false\n\
            [[bin]]\nname = 'shared'\nharness = true\n";
        for (kind, expected) in [
            (RustTestBinaryKind::LIB, false),
            (RustTestBinaryKind::TEST, true),
            (RustTestBinaryKind::BIN, false),
            (RustTestBinaryKind::BENCH, false),
            (RustTestBinaryKind::EXAMPLE, false),
            (RustTestBinaryKind::new("unknown"), false),
        ] {
            assert_eq!(
                detect(manifest, "package", "shared", &kind),
                expected,
                "{kind}"
            );
        }
    }

    #[test_case("")]
    #[test_case("not TOML")]
    #[test_case("[[test]]\nname = 'custom'\nharness = 'false'"; "string harness")]
    #[test_case("[[test]]\nname = 1\nharness = false"; "numeric name")]
    #[test_case("[[test]]\npath = 'tests/custom.rs'\nharness = false")]
    #[test_case("[test]\nname = 'custom'\nharness = false"; "wrong section shape")]
    #[test_case("[package]\nharness = false")]
    #[test_case("[[test]]\nname = 'custom'\nharness = false\n[[test]]\nname = 'custom'"; "duplicate name")]
    #[test_case("[[test]]\nname = 'custom'\nharness = false\n[[test]]\nname = 'custom'\nharness = false"; "duplicate custom harness")]
    fn test_unknown_manifest(manifest: &str) {
        assert!(!detect(
            manifest,
            "package",
            "custom",
            &RustTestBinaryKind::TEST,
        ));
    }

    #[test]
    fn test_package_identity() {
        for table in ["package", "project"] {
            let manifest = format!("[{table}]\nname = 'package-name'\n[lib]\nharness = false");
            assert!(manifest_has_custom_harness(
                &manifest,
                "package-name",
                "package_name",
                &RustTestBinaryKind::LIB
            ));
            assert!(!manifest_has_custom_harness(
                &manifest,
                "different",
                "different",
                &RustTestBinaryKind::LIB
            ));
        }
        assert!(!manifest_has_custom_harness(
            "[lib]\nharness = false",
            "package",
            "package",
            &RustTestBinaryKind::LIB
        ));
        let manifest =
            "[package]\nname = 'package'\n[project]\nname = 'legacy'\n[lib]\nharness = false";
        assert!(manifest_has_custom_harness(
            manifest,
            "package",
            "package",
            &RustTestBinaryKind::LIB
        ));
        assert!(!manifest_has_custom_harness(
            manifest,
            "legacy",
            "legacy",
            &RustTestBinaryKind::LIB
        ));
    }
}
