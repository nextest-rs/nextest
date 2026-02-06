// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared test helpers for `list` module tests.

use super::*;
use crate::{
    cargo_config::TargetTriple,
    platform::{BuildPlatforms, HostPlatform, PlatformLibdir},
    reuse_build::PathMapper,
};
use guppy::{
    CargoMetadata, PackageId,
    graph::{PackageGraph, PackageMetadata},
};
use nextest_filtering::{CompiledExpr, EvalContext};
use nextest_metadata::{BuildPlatform, RustBinaryId, RustTestBinaryKind};
use std::{collections::BTreeSet, sync::LazyLock};

pub(super) static PACKAGE_GRAPH_FIXTURE: LazyLock<PackageGraph> = LazyLock::new(|| {
    static FIXTURE_JSON: &str = include_str!("../../../fixtures/cargo-metadata.json");
    let metadata = CargoMetadata::parse_json(FIXTURE_JSON).expect("fixture is valid JSON");
    metadata
        .build_graph()
        .expect("fixture is valid PackageGraph")
});

pub(super) static PACKAGE_METADATA_ID: &str =
    "metadata-helper 0.1.0 (path+file:///Users/fakeuser/local/testcrates/metadata/metadata-helper)";

pub(super) fn package_metadata() -> PackageMetadata<'static> {
    PACKAGE_GRAPH_FIXTURE
        .metadata(&PackageId::new(PACKAGE_METADATA_ID))
        .expect("package ID is valid")
}

/// Creates a test artifact with the given binary ID and sensible defaults.
pub(super) fn make_test_artifact(binary_id: &str) -> RustTestArtifact<'static> {
    let binary_name = binary_id.rsplit("::").next().unwrap_or(binary_id);
    RustTestArtifact {
        binary_path: format!("/fake/{binary_name}").into(),
        cwd: "/fake/cwd".into(),
        package: package_metadata(),
        binary_name: binary_name.to_owned(),
        binary_id: RustBinaryId::new(binary_id),
        kind: RustTestBinaryKind::LIB,
        non_test_binaries: BTreeSet::new(),
        build_platform: BuildPlatform::Target,
    }
}

/// Creates a minimal build meta for tests that don't need cross-compilation.
pub(super) fn simple_build_meta() -> RustBuildMeta<TestListState> {
    let build_platforms = BuildPlatforms {
        host: HostPlatform {
            platform: TargetTriple::x86_64_unknown_linux_gnu().platform,
            libdir: PlatformLibdir::Available("/fake/libdir".into()),
        },
        target: None,
    };
    RustBuildMeta::new("/fake", build_platforms).map_paths(&PathMapper::noop())
}

/// Creates a default eval context using `CompiledExpr::ALL`.
pub(super) fn simple_ecx() -> EvalContext<'static> {
    EvalContext {
        default_filter: &CompiledExpr::ALL,
    }
}

/// Collects the names of tests that match filters from a suite.
pub(super) fn collect_matching_tests<'a>(suite: &'a RustTestSuite<'_>) -> Vec<&'a str> {
    suite
        .status
        .test_cases()
        .filter(|tc| tc.test_info.filter_match.is_match())
        .map(|tc| tc.name.as_str())
        .collect()
}
