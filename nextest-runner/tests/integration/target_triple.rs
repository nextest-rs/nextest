// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::*;
use color_eyre::Result;
use nextest_runner::cargo_config::{CargoConfigs, TargetTriple, TargetTripleSource};

#[test]
fn parses_target_cli_option() {
    let triple = with_env(
        [("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl")],
        || target_triple(Some("aarch64-unknown-linux-gnu")),
    )
    .unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            triple: "aarch64-unknown-linux-gnu".to_owned(),
            source: TargetTripleSource::CliOption,
        })
    )
}

#[test]
fn parses_cargo_env() {
    let triple = with_env(
        [("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl")],
        || target_triple(None),
    )
    .unwrap();

    assert_eq!(
        triple,
        Some(TargetTriple {
            triple: "x86_64-unknown-linux-musl".to_owned(),
            source: TargetTripleSource::Env,
        })
    )
}

// TODO: tests involving Cargo configs -- ensure the current dir is used for that.

fn target_triple(target_cli_option: Option<&str>) -> Result<Option<TargetTriple>> {
    let configs = CargoConfigs::new_with_isolation(&workspace_root(), &workspace_root()).unwrap();
    let triple = TargetTriple::find(&configs, target_cli_option)?;
    Ok(triple)
}
