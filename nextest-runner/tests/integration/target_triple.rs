// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use color_eyre::Result;
use nextest_runner::cargo_config::{CargoConfigs, TargetTriple, TargetTripleSource};
use target_spec::{Platform, TargetFeatures};

#[test]
fn parses_target_cli_option() {
    std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl");
    let triple = target_triple(Some("aarch64-unknown-linux-gnu")).unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform: platform("aarch64-unknown-linux-gnu"),
            source: TargetTripleSource::CliOption,
        })
    )
}

#[test]
fn parses_cargo_env() {
    std::env::set_var("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl");
    let triple = target_triple(None).unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            platform: platform("x86_64-unknown-linux-musl"),
            source: TargetTripleSource::Env,
        })
    )
}

fn target_triple(target_cli_option: Option<&str>) -> Result<Option<TargetTriple>> {
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root(),
        &workspace_root(),
    )
    .unwrap();
    let triple = TargetTriple::find(&configs, target_cli_option)?;
    Ok(triple)
}

fn platform(triple_str: &str) -> Platform {
    Platform::new(triple_str.to_owned(), TargetFeatures::Unknown).unwrap()
}
