// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::fixtures::{test_init, workspace_root};
use color_eyre::Result;
use nextest_runner::{
    RustcCli,
    cargo_config::{CargoConfigs, TargetTriple},
    platform::{BuildPlatforms, HostPlatform, PlatformLibdir, TargetPlatform},
    target_runner::{PlatformRunner, TargetRunner},
};
use target_spec::Platform;

fn runner_for_target(triple: Option<&str>) -> Result<(BuildPlatforms, TargetRunner)> {
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root(),
        &workspace_root(),
        Vec::new(),
    )
    .unwrap();

    let build_platforms = {
        let host = HostPlatform::detect(PlatformLibdir::from_rustc_stdout(
            RustcCli::print_host_libdir().read(),
        ))?;
        let target = if let Some(triple) = TargetTriple::find(&configs, triple, &host.platform)? {
            let libdir =
                PlatformLibdir::from_rustc_stdout(RustcCli::print_target_libdir(&triple).read());
            Some(TargetPlatform::new(triple, libdir))
        } else {
            None
        };
        BuildPlatforms { host, target }
    };

    let target_runner = TargetRunner::new(&configs, &build_platforms)?;
    Ok((build_platforms, target_runner))
}

#[test]
fn parses_cargo_env() {
    test_init();
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe { std::env::set_var(current_runner_env_var(), "cargo_with_default --arg --arg2") };

    let (_, def_runner) = runner_for_target(None).unwrap();

    for (_, platform_runner) in def_runner.all_build_platforms() {
        let platform_runner = platform_runner.expect("env var means runner should be defined");
        assert_eq!("cargo_with_default", platform_runner.binary());
        assert_eq!(
            vec!["--arg", "--arg2"],
            platform_runner.args().collect::<Vec<_>>()
        );
    }

    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        std::env::set_var(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER",
            "cargo_with_specific",
        )
    };

    let (_, specific_runner) = runner_for_target(Some("aarch64-linux-android")).unwrap();

    let platform_runner = specific_runner.target().unwrap();
    assert_eq!("cargo_with_specific", platform_runner.binary());
    assert_eq!(0, platform_runner.args().count());
}

fn parse_triple(triple: &'static str) -> target_spec::Platform {
    target_spec::Platform::new(triple, target_spec::TargetFeatures::Unknown).unwrap()
}

#[test]
fn parses_cargo_config_exact() {
    let workspace_root = workspace_root();
    let windows = parse_triple("x86_64-pc-windows-gnu");
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root,
        &workspace_root,
        Vec::new(),
    )
    .unwrap();
    let runner = PlatformRunner::find_config(&configs, &windows)
        .unwrap()
        .unwrap();

    assert_eq!("wine", runner.binary());
    assert_eq!(0, runner.args().count());
}

#[test]
fn disregards_non_matching() {
    let workspace_root = workspace_root();
    let windows = parse_triple("x86_64-unknown-linux-gnu");
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root,
        &workspace_root,
        Vec::new(),
    )
    .unwrap();
    assert!(
        PlatformRunner::find_config(&configs, &windows)
            .unwrap()
            .is_none()
    );
}

#[test]
fn parses_cargo_config_cfg() {
    let workspace_root = workspace_root();
    let android = parse_triple("aarch64-linux-android");
    let configs = CargoConfigs::new_with_isolation(
        Vec::<String>::new(),
        &workspace_root,
        &workspace_root,
        Vec::new(),
    )
    .unwrap();
    let runner = PlatformRunner::find_config(&configs, &android)
        .unwrap()
        .unwrap();

    assert_eq!("android-runner", runner.binary());
    assert_eq!(vec!["-x"], runner.args().collect::<Vec<_>>());

    let linux = parse_triple("x86_64-unknown-linux-musl");
    let runner = PlatformRunner::find_config(&configs, &linux)
        .unwrap()
        .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}

#[test]
fn falls_back_to_cargo_config() {
    let linux = parse_triple("x86_64-unknown-linux-musl");
    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        std::env::set_var(
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
            "cargo-runner-windows",
        )
    };

    let (_, target_runner) = runner_for_target(Some(linux.triple_str())).unwrap();

    let platform_runner = target_runner.target().unwrap();

    assert_eq!("passthrough", platform_runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        platform_runner.args().collect::<Vec<_>>()
    );
}

fn current_runner_env_var() -> String {
    PlatformRunner::runner_env_var(
        &Platform::build_target().expect("current platform is known to target-spec"),
    )
}
