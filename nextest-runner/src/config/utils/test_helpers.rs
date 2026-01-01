// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Test helpers for configuration.

use crate::{
    cargo_config::{CargoConfigs, TargetDefinitionLocation, TargetTriple, TargetTripleSource},
    config::elements::{CustomTestGroup, TestGroup},
    platform::{BuildPlatforms, HostPlatform, PlatformLibdir, TargetPlatform},
};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use camino_tempfile_ext::prelude::*;
use guppy::{
    MetadataCommand, PackageId,
    graph::{PackageGraph, cargo::BuildPlatform},
};
use nextest_filtering::BinaryQuery;
use nextest_metadata::{RustBinaryId, RustTestBinaryKind};
use serde::Deserialize;
use std::{path::PathBuf, process::Command};
use target_spec::{Platform, TargetFeatures};

pub(in crate::config) fn temp_workspace(
    temp_dir: &Utf8TempDir,
    config_contents: &str,
) -> PackageGraph {
    Command::new(cargo_path())
        .args(["init", "--lib", "--name=test-package", "--vcs=none"])
        .current_dir(temp_dir)
        .status()
        .expect("error initializing cargo project");

    temp_dir
        .child(".config/nextest.toml")
        .write_str(config_contents)
        .expect("error writing config file");

    PackageGraph::from_command(MetadataCommand::new().current_dir(temp_dir.path()))
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

pub(in crate::config) struct BinaryQueryCreator<'a> {
    package_id: &'a PackageId,
    binary_id: RustBinaryId,
    kind: RustTestBinaryKind,
    binary_name: &'a str,
    platform: BuildPlatform,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::config) enum ConfigErrorKind {
    NotFound,
    Message,
}

impl BinaryQueryCreator<'_> {
    pub(in crate::config) fn to_query(&self) -> BinaryQuery<'_> {
        BinaryQuery {
            package_id: self.package_id,
            binary_id: &self.binary_id,
            kind: &self.kind,
            binary_name: self.binary_name,
            platform: self.platform,
        }
    }
}

pub(in crate::config) fn binary_query<'a>(
    graph: &'a PackageGraph,
    package_id: &'a PackageId,
    kind: &str,
    binary_name: &'a str,
    platform: BuildPlatform,
) -> BinaryQueryCreator<'a> {
    let package_name = graph.metadata(package_id).unwrap().name();
    let kind = RustTestBinaryKind::new(kind.to_owned());
    let binary_id = RustBinaryId::from_parts(package_name, &kind, binary_name);
    BinaryQueryCreator {
        package_id,
        binary_id,
        kind,
        binary_name,
        platform,
    }
}

pub(in crate::config) fn build_platforms() -> BuildPlatforms {
    BuildPlatforms {
        host: HostPlatform {
            platform: Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap(),
            libdir: PlatformLibdir::Available(Utf8PathBuf::from(
                "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib",
            )),
        },
        target: Some(TargetPlatform {
            triple: TargetTriple {
                platform: Platform::new("aarch64-apple-darwin", TargetFeatures::Unknown).unwrap(),
                source: TargetTripleSource::Env,
                location: TargetDefinitionLocation::Builtin,
            },
            libdir: PlatformLibdir::Available(Utf8PathBuf::from(
                "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/aarch64-apple-darwin/lib",
            )),
        }),
    }
}

// XXX: do we need workspace_dir at all? Seems unnecessary.
pub(in crate::config) fn custom_build_platforms(workspace_dir: &Utf8Path) -> BuildPlatforms {
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        workspace_dir,
        workspace_dir,
        Vec::new(),
    )
    .unwrap();

    let host_platform = Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap();

    let mut fixture =
        Utf8PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir is available"));
    fixture.pop();
    fixture.push("fixtures/custom-target/my-target.json");

    let triple = TargetTriple::find(&configs, Some(fixture.as_str()), &host_platform)
        .expect("custom platform parsed")
        .expect("custom platform found");
    assert!(
        triple.platform.is_custom(),
        "returned triple should be custom (was: {triple:?}"
    );
    assert_eq!(
        triple.platform.triple_str(),
        "my-target",
        "triple_str matches"
    );

    let host = HostPlatform {
        platform: host_platform,
        libdir: PlatformLibdir::Available(Utf8PathBuf::from(
            "/home/fake/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib",
        )),
    };
    let target = TargetPlatform {
        triple,
        libdir: PlatformLibdir::Available(Utf8PathBuf::from(
            "/home/fake/.rustup/toolchains/my-target/lib/rustlib/my-target/lib",
        )),
    };

    BuildPlatforms {
        host,
        target: Some(target),
    }
}

pub(in crate::config) fn test_group(name: &str) -> TestGroup {
    TestGroup::Custom(custom_test_group(name))
}

pub(in crate::config) fn custom_test_group(name: &str) -> CustomTestGroup {
    CustomTestGroup::new(name.into())
        .unwrap_or_else(|error| panic!("invalid custom test group {name}: {error}"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::config) struct MietteJsonReport {
    pub(in crate::config) message: String,
    pub(in crate::config) labels: Vec<MietteJsonLabel>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::config) struct MietteJsonLabel {
    pub(in crate::config) label: String,
    pub(in crate::config) span: MietteJsonSpan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::config) struct MietteJsonSpan {
    pub(in crate::config) offset: usize,
    pub(in crate::config) length: usize,
}
