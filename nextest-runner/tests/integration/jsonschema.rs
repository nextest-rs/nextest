// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use color_eyre::eyre::{self, Context};
use std::fs;

fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::var("NEXTEST_WORKSPACE_ROOT")
            .expect("NEXTEST_WORKSPACE_ROOT is set (running under cargo nextest run)"),
    )
}

#[test]
fn test_validate_nextest_config() -> eyre::Result<()> {
    let schema_path = workspace_root().join("jsonschemas/nextest.json");
    let schema_text = fs::read_to_string(&schema_path)
        .wrap_err_with(|| format!("error reading schema file at {schema_path}"))?;
    let schema: serde_json::Value = serde_json::from_str(&schema_text)
        .wrap_err_with(|| format!("error parsing schema at {schema_path}"))?;
    let validator = jsonschema::validator_for(&schema)
        .wrap_err_with(|| format!("error building validator from {schema_path}"))?;

    let config_path = workspace_root().join(".config/nextest.toml");
    let config_text = fs::read_to_string(&config_path)
        .wrap_err_with(|| format!("error reading config file at {config_path}"))?;
    let config: serde_json::Value = toml::from_str(&config_text)
        .wrap_err_with(|| format!("error parsing config at {config_path}"))?;

    let errors: Vec<_> = validator.iter_errors(&config).collect();
    assert!(
        errors.is_empty(),
        "{config_path} does not validate against {schema_path}:\n{errors:#?}",
    );

    Ok(())
}
