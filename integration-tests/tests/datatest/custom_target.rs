// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::helpers::bind_insta_settings;
use camino::Utf8Path;
use camino_tempfile_ext::prelude::*;
use integration_tests::{env::set_env_vars_for_test, nextest_cli::CargoNextestCli};
use nextest_metadata::NextestExitCode;

pub(crate) fn custom_invalid(path: &Utf8Path, contents: String) -> datatest_stable::Result<()> {
    let env_info = set_env_vars_for_test();
    let (_guard, insta_prefix) = bind_insta_settings(path, "snapshots/custom-invalid");

    let dir = Utf8TempDir::with_prefix("nextest-custom-target-")?;
    let json_path = dir.child(path.file_name().unwrap());
    json_path.write_str(&contents)?;

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            // Use color in snapshots to ensure that it is correctly passed
            // through.
            //
            // It might be nice to use snapbox in the future, because it has
            // really nice color support.
            "--color",
            "always",
            "debug",
            "build-platforms",
            "--target",
            json_path.as_str(),
        ])
        .unchecked(true)
        .output();

    // We expect this to fail with a setup error.
    assert!(!output.exit_status.success());
    assert_eq!(
        output.exit_status.code(),
        Some(NextestExitCode::SETUP_ERROR),
        "exit code matches"
    );

    // Print the output.
    insta::assert_snapshot!(format!("{insta_prefix}-stderr"), output.stderr_as_str());

    Ok(())
}
