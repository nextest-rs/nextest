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
    std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl");
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
    std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl");
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
static MY_TARGET_JSON_PATH: &str = "../custom-target/my-target.json";

#[test]
fn parses_custom_target_cli() {
    std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl");
    let expected_path = workspace_root()
        .join(MY_TARGET_JSON_PATH)
        .canonicalize_utf8()
        .expect("canonicalization succeeded");
    let triple = target_triple(Some(MY_TARGET_JSON_PATH), Vec::new())
        .unwrap()
        .expect("platform found");
    assert_eq!(
        triple.platform.triple_str(),
        MY_TARGET_TRIPLE_STR,
        "custom platform name"
    );

    assert!(triple.platform.is_custom(), "custom platform");
    assert_eq!(triple.source, TargetTripleSource::CliOption);
    assert_eq!(
        triple.location,
        TargetDefinitionLocation::DirectPath(expected_path)
    );
}

#[test]
fn parses_custom_target_env() {
    std::env::set_var("CARGO_BUILD_TARGET", MY_TARGET_JSON_PATH);
    let expected_path = workspace_root()
        .join(MY_TARGET_JSON_PATH)
        .canonicalize_utf8()
        .expect("canonicalization succeeded");
    let triple = target_triple(None, Vec::new())
        .unwrap()
        .expect("platform found");
    assert_eq!(
        triple.platform.triple_str(),
        MY_TARGET_TRIPLE_STR,
        "custom platform name"
    );

    assert!(triple.platform.is_custom(), "custom platform");
    assert_eq!(triple.source, TargetTripleSource::Env);
    assert_eq!(
        triple.location,
        TargetDefinitionLocation::DirectPath(expected_path)
    );
}

#[test]
fn parses_custom_target_cli_from_rust_target_path() {
    let target_paths = vec![workspace_root().join("../custom-target")];
    let expected_path = workspace_root()
        .join(MY_TARGET_JSON_PATH)
        .canonicalize_utf8()
        .expect("canonicalization succeeded");
    let triple = target_triple(Some(MY_TARGET_TRIPLE_STR), target_paths)
        .unwrap()
        .expect("platform found");
    assert_eq!(
        triple.platform.triple_str(),
        MY_TARGET_TRIPLE_STR,
        "custom platform name"
    );

    assert!(triple.platform.is_custom(), "custom platform");
    assert_eq!(triple.source, TargetTripleSource::CliOption);
    assert_eq!(
        triple.location,
        TargetDefinitionLocation::RustTargetPath(expected_path)
    );
}

#[test]
fn parses_custom_target_env_from_rust_target_path() {
    std::env::set_var("CARGO_BUILD_TARGET", MY_TARGET_TRIPLE_STR);
    let target_paths = vec![workspace_root().join("../custom-target")];
    let expected_path = workspace_root()
        .join(MY_TARGET_JSON_PATH)
        .canonicalize_utf8()
        .expect("canonicalization succeeded");
    let triple = target_triple(None, target_paths)
        .unwrap()
        .expect("platform found");
    assert_eq!(
        triple.platform.triple_str(),
        MY_TARGET_TRIPLE_STR,
        "custom platform name"
    );

    assert!(triple.platform.is_custom(), "custom platform");
    assert_eq!(triple.source, TargetTripleSource::Env);
    assert_eq!(
        triple.location,
        TargetDefinitionLocation::RustTargetPath(expected_path)
    );
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

fn target_triple(
    target_cli_option: Option<&str>,
    target_paths: Vec<Utf8PathBuf>,
) -> Result<Option<TargetTriple>> {
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root(),
        &workspace_root(),
        target_paths,
    )
    .unwrap();
    let triple = TargetTriple::find(&configs, target_cli_option)?;
    Ok(triple)
}

fn platform(triple_str: &str) -> Platform {
    Platform::new(triple_str.to_owned(), TargetFeatures::Unknown).unwrap()
}
