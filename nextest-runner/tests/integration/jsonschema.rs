// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use color_eyre::eyre::{self, Context};
use nextest_runner::config::core::NextestConfig;
use std::fs;

const SCHEMA_LABEL: &str =
    "embedded repo-config schema (nextest-runner/jsonschemas/repo-config.json)";

fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::var("NEXTEST_WORKSPACE_ROOT")
            .expect("NEXTEST_WORKSPACE_ROOT is set (running under cargo nextest run)"),
    )
}

fn build_validator() -> eyre::Result<jsonschema::Validator> {
    let schema: serde_json::Value = serde_json::from_str(NextestConfig::SCHEMA)
        .wrap_err_with(|| format!("error parsing {SCHEMA_LABEL}"))?;
    jsonschema::validator_for(&schema)
        .wrap_err_with(|| format!("error building validator from {SCHEMA_LABEL}"))
}

fn assert_validates(
    validator: &jsonschema::Validator,
    label: &str,
    toml_text: &str,
) -> eyre::Result<()> {
    let config: serde_json::Value =
        toml::from_str(toml_text).wrap_err_with(|| format!("error parsing {label}"))?;
    let errors: Vec<_> = validator.iter_errors(&config).collect();
    assert!(
        errors.is_empty(),
        "{label} does not validate against {SCHEMA_LABEL}:\n{errors:#?}",
    );
    Ok(())
}

#[test]
fn test_validate_nextest_config() -> eyre::Result<()> {
    let validator = build_validator()?;
    let config_path = workspace_root().join(".config/nextest.toml");
    let config_text = fs::read_to_string(&config_path)
        .wrap_err_with(|| format!("error reading config file at {config_path}"))?;
    assert_validates(&validator, config_path.as_str(), &config_text)
}

#[test]
fn test_validate_default_config() -> eyre::Result<()> {
    let validator = build_validator()?;
    assert_validates(
        &validator,
        "default config (nextest-runner/default-config.toml)",
        NextestConfig::DEFAULT_CONFIG,
    )
}
