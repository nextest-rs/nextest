// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use camino::Utf8PathBuf;
use color_eyre::Result;
use nextest_runner::cargo_config::{
    CargoConfigs, TargetDefinitionLocation, TargetTriple, TargetTripleSource,
};
use target_spec::{Platform, TargetFeatures};

#[test]
fn parses_target_cli_option() {
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl") };
    let triple = target_triple(Some("aarch64-unknown-linux-gnu"), Vec::new()).unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform: platform("aarch64-unknown-linux-gnu"),
            source: TargetTripleSource::CliOption,
            location: TargetDefinitionLocation::Builtin,
        })
    )
}

#[test]
fn parses_cargo_env() {
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl") };
    let triple = target_triple(None, Vec::new()).unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform: platform("x86_64-unknown-linux-musl"),
            source: TargetTripleSource::Env,
            location: TargetDefinitionLocation::Builtin,
        })
    )
}

static MY_TARGET_TRIPLE_STR: &str = "my-target";
static MY_TARGET_2_TRIPLE_STR: &str = "my-target-2";
static MY_TARGET_JSON_PATH: &str = "../custom-target/my-target.json";
static MY_TARGET_2_JSON_PATH: &str = "../custom-target/my-target-2.json";
static MY_TARGET_PATHS: &[(&str, &str)] = &[
    (MY_TARGET_JSON_PATH, MY_TARGET_TRIPLE_STR),
    (MY_TARGET_2_JSON_PATH, MY_TARGET_2_TRIPLE_STR),
];

#[test]
fn parses_custom_target_cli() {
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl") };
    for (target_path, expected_triple) in MY_TARGET_PATHS {
        eprintln!("** testing: {}", target_path);
        let expected_path = fixture_project_dir()
            .join(target_path)
            .canonicalize_utf8()
            .expect("canonicalization succeeded");
        let triple = target_triple(Some(target_path), Vec::new())
            .unwrap()
            .expect("platform found");
        assert_eq!(
            triple.platform.triple_str(),
            *expected_triple,
            "custom platform name"
        );

        assert!(triple.platform.is_custom(), "custom platform");
        assert_eq!(triple.source, TargetTripleSource::CliOption);
        assert_eq!(
            triple.location,
            TargetDefinitionLocation::DirectPath(expected_path)
        );
    }
}

#[test]
fn parses_custom_target_env() {
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var("CARGO_BUILD_TARGET", MY_TARGET_JSON_PATH) };
    for (target_path, expected_triple) in MY_TARGET_PATHS {
        eprintln!("** testing: {}", target_path);
        unsafe { std::env::set_var("CARGO_BUILD_TARGET", target_path) };
        let expected_path = fixture_project_dir()
            .join(target_path)
            .canonicalize_utf8()
            .expect("canonicalization succeeded");
        let triple = target_triple(None, Vec::new())
            .unwrap()
            .expect("platform found");
        assert_eq!(
            triple.platform.triple_str(),
            *expected_triple,
            "custom platform name"
        );

        assert!(triple.platform.is_custom(), "custom platform");
        assert_eq!(triple.source, TargetTripleSource::Env);
        assert_eq!(
            triple.location,
            TargetDefinitionLocation::DirectPath(expected_path)
        );
    }
}

#[test]
fn parses_custom_target_cli_from_rust_target_path() {
    let target_paths = vec![fixture_project_dir().join("../custom-target")];
    for (target_path, expected_triple) in MY_TARGET_PATHS {
        eprintln!("** testing: {}", expected_triple);
        let expected_path = fixture_project_dir()
            .join(target_path)
            .canonicalize_utf8()
            .expect("canonicalization succeeded");
        let triple = target_triple(Some(expected_triple), target_paths.clone())
            .unwrap()
            .expect("platform found");
        assert_eq!(
            triple.platform.triple_str(),
            *expected_triple,
            "custom platform name"
        );

        assert!(triple.platform.is_custom(), "custom platform");
        assert_eq!(triple.source, TargetTripleSource::CliOption);
        assert_eq!(
            triple.location,
            TargetDefinitionLocation::RustTargetPath(expected_path)
        );
    }
}

#[test]
fn parses_custom_target_env_from_rust_target_path() {
    let target_paths = vec![fixture_project_dir().join("../custom-target")];
    for (target_path, expected_triple) in MY_TARGET_PATHS {
        eprintln!("** testing: {}", expected_triple);
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::set_var("CARGO_BUILD_TARGET", expected_triple) };
        let expected_path = fixture_project_dir()
            .join(target_path)
            .canonicalize_utf8()
            .expect("canonicalization succeeded");
        let triple = target_triple(None, target_paths.clone())
            .unwrap()
            .expect("platform found");
        assert_eq!(
            triple.platform.triple_str(),
            *expected_triple,
            "custom platform name"
        );

        assert!(triple.platform.is_custom(), "custom platform");
        assert_eq!(triple.source, TargetTripleSource::Env);
        assert_eq!(
            triple.location,
            TargetDefinitionLocation::RustTargetPath(expected_path)
        );
    }
}

#[test]
fn parses_custom_target_cli_heuristic() {
    // This target is never going to exist.
    let triple = target_triple(Some("armv5te-unknown-linux-musl"), Vec::new()).unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform: platform("armv5te-unknown-linux-musl"),
            source: TargetTripleSource::CliOption,
            location: TargetDefinitionLocation::Heuristic,
        })
    )
}

/// Tests the use of a custom target triple that gets resolved via `rustc --print=cfg`,
/// because it is not part of the builtin rustc targets.
/// e.g. targets in custom rustc builds
///
/// To test this with the default rustc, the script `rustc-cfg-test.sh` is used as RUSTC wrapper,
/// and returns the `cfg_text` string if `--print=cfg` is given as first argument.
/// All other invocations are forwarded to `rustc`.
///
/// Note: Only runs on *unix* targets due to the shell script. Since the behavior is OS independent,
/// testing it for Unix is enough.
#[cfg(target_family = "unix")]
#[test]
fn parses_custom_target_rustc_cfg() {
    let script_path = fixture_project_dir()
        .join("scripts")
        .join("rustc-cfg-test.sh");

    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var("RUSTC", script_path) };

    // This target is never going to exist.
    let triple_str = "some-custom-cfg-target";
    let triple = target_triple(Some(triple_str), Vec::new()).unwrap();

    let cfg_text = r#"debug_assertions
panic="abort"
target_abi="eabihf"
target_arch="arm"
target_endian="little"
target_env="musl"
target_family="unix"
target_has_atomic="16"
target_has_atomic="32"
target_has_atomic="8"
target_has_atomic="ptr"
target_os="linux"
target_pointer_width="32"
target_vendor="unknown"
unix
"#;

    let platform = Platform::new_custom_cfg(triple_str, cfg_text, TargetFeatures::Unknown)
        .expect("Failed to get platform from valid rustc cfg");

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform,
            source: TargetTripleSource::CliOption,
            location: TargetDefinitionLocation::RustcCfgCustom(cfg_text.to_owned()),
        })
    )
}

fn target_triple(
    target_cli_option: Option<&str>,
    target_paths: Vec<Utf8PathBuf>,
) -> Result<Option<TargetTriple>> {
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &fixture_project_dir(),
        &fixture_project_dir(),
        target_paths,
    )
    .unwrap();
    let host_platform = dummy_host_platform();
    let triple = TargetTriple::find(&configs, target_cli_option, &host_platform)?;
    Ok(triple)
}

fn platform(triple_str: &str) -> Platform {
    Platform::new(triple_str.to_owned(), TargetFeatures::Unknown).unwrap()
}

fn dummy_host_platform() -> Platform {
    Platform::new(
        "x86_64-unknown-linux-gnu".to_owned(),
        TargetFeatures::Unknown,
    )
    .unwrap()
}
