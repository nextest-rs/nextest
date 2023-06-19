// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{TargetDefinitionLocation, TargetTriple, TargetTripleSource},
    config::{CustomTestGroup, TestGroup},
    platform::BuildPlatforms,
};
use camino::{Utf8Path, Utf8PathBuf};
use guppy::{graph::PackageGraph, MetadataCommand};
use std::{io::Write, path::PathBuf, process::Command};
use target_spec::{Platform, TargetFeatures};

pub(super) fn temp_workspace(temp_dir: &Utf8Path, config_contents: &str) -> PackageGraph {
    Command::new(cargo_path())
        .args(["init", "--lib", "--name=test-package", "--vcs=none"])
        .current_dir(temp_dir)
        .status()
        .expect("error initializing cargo project");

    let config_dir = temp_dir.join(".config");
    std::fs::create_dir(&config_dir).expect("error creating config dir");

    let config_path = config_dir.join("nextest.toml");
    let mut config_file = std::fs::File::create(config_path).unwrap();
    config_file.write_all(config_contents.as_bytes()).unwrap();

    PackageGraph::from_command(MetadataCommand::new().current_dir(temp_dir))
        .expect("error creating package graph")
}

pub(super) fn cargo_path() -> Utf8PathBuf {
    match std::env::var_os("CARGO") {
        Some(cargo_path) => PathBuf::from(cargo_path)
            .try_into()
            .expect("CARGO env var is not valid UTF-8"),
        None => Utf8PathBuf::from("cargo"),
    }
}

pub(super) fn build_platforms() -> BuildPlatforms {
    BuildPlatforms::new_with_host(
        Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap(),
        Some(TargetTriple {
            platform: Platform::new("aarch64-apple-darwin", TargetFeatures::Unknown).unwrap(),
            source: TargetTripleSource::Env,
            location: TargetDefinitionLocation::Builtin,
        }),
    )
}

pub(super) fn test_group(name: &str) -> TestGroup {
    TestGroup::Custom(custom_test_group(name))
}

pub(super) fn custom_test_group(name: &str) -> CustomTestGroup {
    CustomTestGroup::new(name.into())
        .unwrap_or_else(|error| panic!("invalid custom test group {name}: {error}"))
}
