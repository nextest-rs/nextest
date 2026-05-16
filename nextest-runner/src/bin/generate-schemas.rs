// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use miette::{Context, IntoDiagnostic, Result};
use nextest_runner::{config::core::nextest_config_schema, user_config::user_config_schema};
use std::fs;

fn main() -> Result<()> {
    let schema_dir = schema_dir();
    fs::create_dir_all(&schema_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create schema directory {schema_dir}"))?;

    write_schema(
        &schema_dir,
        "repo-config.json",
        &nextest_config_schema(),
        "repo config",
    )?;
    write_schema(
        &schema_dir,
        "user-config.json",
        &user_config_schema(),
        "user config",
    )?;

    Ok(())
}

fn write_schema(
    schema_dir: &Utf8Path,
    file_name: &str,
    schema: &schemars::Schema,
    label: &str,
) -> Result<()> {
    let schema_path = schema_dir.join(file_name);

    let mut schema_json = serde_json::to_vec_pretty(schema)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to serialize nextest {label} schema"))?;
    schema_json.push(b'\n');

    fs::write(&schema_path, schema_json)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write schema to {schema_path}"))?;

    println!("{schema_path}");
    Ok(())
}

fn schema_dir() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("jsonschemas")
}
