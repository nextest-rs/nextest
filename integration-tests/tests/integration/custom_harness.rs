// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Custom-harness diagnostics use a temporary target so intentionally invalid
//! listings do not interfere with discovery of the integration suite itself.

use crate::{
    fixtures::{save_binaries_metadata, save_cargo_metadata},
    temp_project::TempProject,
};
use camino::Utf8PathBuf;
use integration_tests::{
    env::{TestEnvInfo, set_env_vars_for_test},
    nextest_cli::{CargoNextestCli, CargoNextestOutput},
};
use nextest_metadata::NextestExitCode;
use std::fs;

const HARNESS_NAME: &str = "harness-diagnostic";
const BINARY_ID: &str = "with-build-script::harness-diagnostic";
const MODE_ENV: &str = "__NEXTEST_CUSTOM_HARNESS_MODE";
const HINT: &str = "this binary uses a custom test harness";
const DOCS_URL: &str = "https://nexte.st/docs/design/custom-test-harnesses/";

const HARNESS_SOURCE: &str = r#"
use std::{env, io::{self, Write}, process};

fn main() {
    let mode = env::var("__NEXTEST_CUSTOM_HARNESS_MODE").unwrap();
    let ignored = env::args().any(|arg| arg == "--ignored");
    if mode != "success" && mode.ends_with("-ignored") == ignored {
        if mode.starts_with("nonzero-") {
            println!("harness stdout marker");
            eprintln!("harness stderr marker");
            process::exit(23);
        } else if mode.starts_with("malformed-") {
            println!("malformed listing: {mode}");
            return;
        } else if mode.starts_with("non-utf8-") {
            io::stdout().write_all(&[0xff]).unwrap();
            eprintln!("harness UTF-8 marker: {mode}");
            return;
        }
    }
    if !ignored {
        println!("regular_test: test");
    }
    println!("ignored_test: test");
}
"#;

#[test]
fn test_custom_harness_listing_failures() {
    let env_info = set_env_vars_for_test();
    let p = HarnessProject::new(&env_info);

    let fresh = p
        .list_cli(&env_info, "nonzero-normal")
        .args([
            "--package",
            "with-build-script",
            "--test",
            HARNESS_NAME,
            "--target-dir",
            p.project.target_dir().as_str(),
        ])
        .output();
    assert_listing_failure(&fresh, "nonzero-normal", true);

    p.save_metadata(&env_info);
    for mode in [
        "nonzero-normal",
        "nonzero-ignored",
        "malformed-normal",
        "malformed-ignored",
        "non-utf8-normal",
        "non-utf8-ignored",
    ] {
        let output = p.reused_list_cli(&env_info, mode).output();
        assert_listing_failure(&output, mode, true);
    }
}

#[test]
fn test_custom_harness_compatible_listing() {
    let env_info = set_env_vars_for_test();
    let p = HarnessProject::new(&env_info);
    p.save_metadata(&env_info);

    let output = p.reused_list_cli(&env_info, "success").output();
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::OK),
        "{output}"
    );
    assert_hint(&output, false);

    let summary = output.decode_test_list_json().unwrap();
    assert_eq!(summary.test_count, 2, "{output}");
    let suite = summary
        .rust_suites
        .values()
        .find(|suite| suite.binary.binary_name == HARNESS_NAME)
        .expect("custom harness is listed");
    let cases: Vec<_> = suite
        .test_cases
        .iter()
        .map(|(name, case)| (name.as_str(), case.ignored))
        .collect();
    assert_eq!(cases, [("ignored_test", true), ("regular_test", false)]);
}

#[test]
fn test_custom_harness_reused_manifest_fail_open() {
    let env_info = set_env_vars_for_test();
    let p = HarnessProject::new(&env_info);
    p.save_metadata(&env_info);

    // Both metadata files are reused so Cargo does not reject or rebuild the
    // edited manifest before nextest's best-effort detector can inspect it.
    for (scenario, contents) in [
        (
            "explicit true",
            Some(
                p.manifest_contents
                    .replace("harness = false", "harness = true"),
            ),
        ),
        (
            "default",
            Some(p.manifest_contents.replace("harness = false", "")),
        ),
        (
            "invalid type",
            Some(
                p.manifest_contents
                    .replace("harness = false", "harness = 'false'"),
            ),
        ),
        ("invalid TOML", Some("[package".to_owned())),
        ("missing member manifest", None),
    ] {
        match contents {
            Some(contents) => fs::write(p.manifest_path(), contents).unwrap(),
            None => fs::remove_file(p.manifest_path()).unwrap(),
        }
        let output = p.reused_list_cli(&env_info, "nonzero-normal").output();
        eprintln!("manifest scenario: {scenario}");
        assert_listing_failure(&output, "nonzero-normal", false);
    }
}

#[test]
fn test_custom_harness_workspace_remap() {
    let env_info = set_env_vars_for_test();
    let original = HarnessProject::new(&env_info);
    original.save_metadata(&env_info);
    let remapped = HarnessProject::new(&env_info);

    // Opposing manifest values prove that detection reads the remapped source
    // rather than the original Cargo metadata's manifest path.
    for remapped_is_custom in [true, false] {
        let (original_contents, remapped_contents) = if remapped_is_custom {
            (
                original
                    .manifest_contents
                    .replace("harness = false", "harness = true"),
                remapped.manifest_contents.clone(),
            )
        } else {
            (
                original.manifest_contents.clone(),
                remapped
                    .manifest_contents
                    .replace("harness = false", "harness = true"),
            )
        };
        fs::write(original.manifest_path(), original_contents).unwrap();
        fs::write(remapped.manifest_path(), remapped_contents).unwrap();

        let output = original
            .reused_list_cli(&env_info, "nonzero-normal")
            .args([
                "--workspace-remap",
                remapped.project.workspace_root().as_str(),
            ])
            .output();
        assert_listing_failure(&output, "nonzero-normal", remapped_is_custom);
    }
}

struct HarnessProject {
    project: TempProject,
    manifest_contents: String,
}

impl HarnessProject {
    fn new(env_info: &TestEnvInfo) -> Self {
        let project = TempProject::new(env_info).unwrap();
        let package_dir = project.workspace_root().join("with-build-script");
        let manifest_path = package_dir.join("Cargo.toml");
        let contents = fs::read_to_string(&manifest_path).unwrap();
        let manifest_contents = format!(
            "{contents}\n[[test]]\nname = \"{HARNESS_NAME}\"\n\
             path = \"tests/harness-diagnostic.rs\"\nharness = false\n"
        );
        fs::write(&manifest_path, &manifest_contents).unwrap();
        fs::create_dir_all(package_dir.join("tests")).unwrap();
        fs::write(
            package_dir.join("tests/harness-diagnostic.rs"),
            HARNESS_SOURCE,
        )
        .unwrap();
        Self {
            project,
            manifest_contents,
        }
    }

    fn manifest_path(&self) -> Utf8PathBuf {
        self.project
            .workspace_root()
            .join("with-build-script/Cargo.toml")
    }

    fn save_metadata(&self, env_info: &TestEnvInfo) {
        save_binaries_metadata(env_info, &self.project);
        save_cargo_metadata(&self.project);
    }

    fn list_cli(&self, env_info: &TestEnvInfo, mode: &str) -> CargoNextestCli {
        let mut cli = CargoNextestCli::for_test(env_info);
        cli.args([
            "--manifest-path",
            self.project.manifest_path().as_str(),
            "--color",
            "never",
            "list",
            "--message-format",
            "json",
        ])
        .env(MODE_ENV, mode)
        .unchecked(true);
        cli
    }

    fn reused_list_cli(&self, env_info: &TestEnvInfo, mode: &str) -> CargoNextestCli {
        let mut cli = self.list_cli(env_info, mode);
        cli.args([
            "--binaries-metadata",
            self.project.binaries_metadata_path().as_str(),
            "--cargo-metadata",
            self.project.cargo_metadata_path().as_str(),
            "-E",
            "binary(=harness-diagnostic)",
        ]);
        cli
    }
}

#[track_caller]
fn assert_hint(output: &CargoNextestOutput, expected: bool) {
    let stderr = output.stderr_as_str();
    assert_eq!(stderr.contains(HINT), expected, "{output}");
    assert_eq!(stderr.contains(DOCS_URL), expected, "{output}");
    assert_eq!(
        stderr.matches(HINT).count(),
        usize::from(expected),
        "{output}"
    );
}

#[track_caller]
fn assert_listing_failure(output: &CargoNextestOutput, mode: &str, hint: bool) {
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::TEST_LIST_CREATION_FAILED),
        "{mode}: {output}",
    );
    let stderr = output.stderr_as_str();
    assert!(stderr.contains(BINARY_ID), "{mode}: {output}");
    assert_hint(output, hint);
    if mode.starts_with("nonzero-") {
        for detail in [
            "exit code 23",
            "harness stdout marker",
            "harness stderr marker",
        ] {
            assert!(
                stderr.contains(detail),
                "{mode} is missing {detail}: {output}"
            );
        }
    } else if mode.starts_with("malformed-") {
        assert!(
            stderr.contains("did not end with the string"),
            "{mode}: {output}"
        );
        assert!(
            stderr.contains(&format!("malformed listing: {mode}")),
            "{output}"
        );
    } else {
        assert!(
            stderr.contains("produced non-UTF-8 output"),
            "{mode}: {output}"
        );
        assert!(
            stderr.contains(&format!("harness UTF-8 marker: {mode}")),
            "{output}"
        );
    }
    if !mode.starts_with("malformed-") {
        assert!(stderr.contains("--list --format terse"), "{mode}: {output}");
        assert_eq!(
            stderr.contains("--ignored"),
            mode.ends_with("-ignored"),
            "{mode}: {output}"
        );
    }
}
