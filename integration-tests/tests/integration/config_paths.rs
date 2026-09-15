// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Config discovery and diagnostics from different invocation directories.

use super::{TempProject, fixtures::redact_temp_root};
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
