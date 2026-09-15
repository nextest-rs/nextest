// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Config discovery and diagnostics from different invocation directories.

use super::{
    TempProject,
    fixtures::{normalize_nextest_stderr, redact_temp_root},
};
use integration_tests::{env::set_env_vars_for_test, nextest_cli::CargoNextestCli};
use nextest_metadata::NextestExitCode;
use std::fs;

#[test]
fn experimental_feature_hints_use_invocation_paths() {
    let env_info = set_env_vars_for_test();
    let project = TempProject::new(&env_info).unwrap();
    let config = project.workspace_root().join(".config/nextest.toml");
    let member = project.workspace_root().join("nested/child");
    fs::create_dir_all(&member).unwrap();
    fs::write(&config, "").unwrap();

    let mut blocks = Vec::new();
    for (cwd, config_file) in [
        (project.workspace_root(), None),
        (member.as_path(), None),
        (member.as_path(), Some("../../.config/nextest.toml")),
        (project.workspace_root(), Some(config.as_str())),
        (project.temp_root(), None),
    ] {
        let mut cli = CargoNextestCli::for_test(&env_info);
        cli.current_dir(cwd)
            .env_remove("NEXTEST_EXPERIMENTAL_BENCHMARKS")
            .args(["bench", "--manifest-path", project.manifest_path().as_str()]);
        if let Some(config_file) = config_file {
            cli.args(["--config-file", config_file]);
        }
        let output = cli.unchecked(true).output();
        assert_eq!(
            output.exit_status.code(),
            Some(NextestExitCode::EXPERIMENTAL_FEATURE_NOT_ENABLED),
            "{output}"
        );

        let stderr = output.stderr_as_str();
        let error_start = stderr
            .find("error:")
            .unwrap_or_else(|| panic!("stderr has an error line\n{output}"));
        let temp_root = project.temp_root();
        blocks.push(format!(
            "cwd: {}\n--config-file: {}\n{}",
            redact_temp_root(cwd.as_str(), temp_root),
            config_file.map_or_else(
                || "(none)".to_owned(),
                |config_file| redact_temp_root(config_file, temp_root),
            ),
            redact_temp_root(stderr[error_start..].trim_end(), temp_root),
        ));
    }

    insta::assert_snapshot!(blocks.join("\n\n"));
}

#[test]
fn tool_config_diagnostics_use_invocation_paths() {
    let env_info = set_env_vars_for_test();
    let project = TempProject::new(&env_info).unwrap();
    let temp_root = project.temp_root();
    let tool_config = project.workspace_root().join(".config/tool.toml");
    let member = project.workspace_root().join("nested/child");
    fs::create_dir_all(&member).unwrap();

    let mut blocks = Vec::new();
    for (scenario, contents) in [
        ("deserialize", "[profile.default]\nretries = 'bad'\n"),
        (
            "unknown-groups",
            "unknown-key = true\n[[profile.default.overrides]]\nfilter = 'all()'\ntest-group = 'missing-group'\n",
        ),
    ] {
        fs::write(&tool_config, contents).unwrap();
        for cwd in [project.workspace_root(), member.as_path()] {
            let output = CargoNextestCli::for_test(&env_info)
                .current_dir(cwd)
                .args(["list", "--manifest-path", project.manifest_path().as_str()])
                .arg("--tool-config-file")
                .arg(format!("my-tool:{tool_config}"))
                .unchecked(true)
                .output();
            assert_eq!(
                output.exit_status.code(),
                Some(NextestExitCode::SETUP_ERROR),
                "{output}"
            );
            blocks.push(format!(
                "cwd: {}\nscenario: {scenario}\n{}",
                redact_temp_root(cwd.as_str(), temp_root),
                normalize_nextest_stderr(&output.stderr_as_str(), temp_root),
            ));
        }
    }

    insta::assert_snapshot!(blocks.join("\n\n"));
}

#[test]
fn config_diagnostics_use_invocation_paths() {
    let env_info = set_env_vars_for_test();
    let project = TempProject::new(&env_info).unwrap();
    let temp_root = project.temp_root();
    let config = project.workspace_root().join(".config/nextest.toml");
    let member = project.workspace_root().join("nested/child");
    fs::create_dir_all(&member).unwrap();

    let mut blocks = Vec::new();
    for (cwd, relative) in [
        (project.workspace_root(), ".config/nextest.toml"),
        (member.as_path(), "../../.config/nextest.toml"),
        (temp_root, config.strip_prefix(temp_root).unwrap().as_str()),
    ] {
        for config_file in [None, Some(relative), Some(config.as_str())] {
            let cwd_label = redact_temp_root(cwd.as_str(), temp_root);
            let config_file_label = config_file.map_or_else(
                || "(none)".to_owned(),
                |config_file| redact_temp_root(config_file, temp_root),
            );
            for (scenario, contents) in [
                ("version-syntax", "nextest-version = ["),
                ("experimental", "experimental = ['not-a-feature']"),
                ("deserialize", "[profile.default]\nretries = 'bad'\n"),
                (
                    "unknown-groups",
                    "unknown-key = true\n[profile.default-foo]\n[[profile.default.overrides]]\nfilter = 'all()'\ntest-group = 'missing-group'\n",
                ),
            ] {
                fs::write(&config, contents).unwrap();
                let mut cli = CargoNextestCli::for_test(&env_info);
                cli.current_dir(cwd).args([
                    "list",
                    "--manifest-path",
                    project.manifest_path().as_str(),
                ]);
                if let Some(config_file) = config_file {
                    cli.args(["--config-file", config_file]);
                }
                let output = cli.unchecked(true).output();
                assert_eq!(
                    output.exit_status.code(),
                    Some(NextestExitCode::SETUP_ERROR),
                    "{output}"
                );
                blocks.push(format!(
                    "cwd: {cwd_label}\n--config-file: {config_file_label}\nscenario: {scenario}\n{}",
                    normalize_nextest_stderr(&output.stderr_as_str(), temp_root),
                ));
            }
            fs::write(&config, "nextest-version = '999.0.0'").unwrap();
            for (command, stream) in [
                (&["list"][..], CapturedStream::Stderr),
                (&["show-config", "version"][..], CapturedStream::Stdout),
            ] {
                let mut cli = CargoNextestCli::for_test(&env_info);
                cli.current_dir(cwd)
                    .env("__NEXTEST_TEST_VERSION", "0.9.100")
                    .args(["--manifest-path", project.manifest_path().as_str()])
                    .args(command.iter().copied());
                if let Some(config_file) = config_file {
                    cli.args(["--config-file", config_file]);
                }
                let output = cli.unchecked(true).output();
                assert_eq!(
                    output.exit_status.code(),
                    Some(NextestExitCode::REQUIRED_VERSION_NOT_MET),
                    "{output}"
                );
                let (stream_name, captured) = match stream {
                    CapturedStream::Stderr => (
                        "stderr",
                        normalize_nextest_stderr(&output.stderr_as_str(), temp_root),
                    ),
                    CapturedStream::Stdout => (
                        "stdout",
                        redact_temp_root(output.stdout_as_str().trim_end(), temp_root),
                    ),
                };
                blocks.push(format!(
                    "cwd: {cwd_label}\n--config-file: {config_file_label}\nscenario: version-requirement\ncommand: {} ({stream_name})\n{captured}",
                    command.join(" "),
                ));
            }
        }
    }

    insta::assert_snapshot!(blocks.join("\n\n"));
}

#[derive(Clone, Copy)]
enum CapturedStream {
    Stderr,
    Stdout,
}
