// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use miette::{Context, IntoDiagnostic, Result};
use nextest_runner::config::core::nextest_config_schema;
use std::fs;

fn main() -> Result<()> {
    let repository_root = repository_root();
    let schema_path = schema_path(&repository_root);
    let schema_dir = schema_path
        .parent()
        .expect("schema path always has a parent directory");

    fs::create_dir_all(schema_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create schema directory {schema_dir}"))?;

    let mut schema_json = serde_json::to_vec_pretty(&nextest_config_schema())
        .into_diagnostic()
        .wrap_err("failed to serialize nextest config schema")?;
    schema_json.push(b'\n');

    fs::write(&schema_path, schema_json)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write schema to {schema_path}"))?;

    println!("{schema_path}");
    Ok(())
}

fn repository_root() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("nextest-runner should be within the repository root")
        .to_path_buf()
}

fn schema_path(repository_root: &Utf8Path) -> Utf8PathBuf {
    repository_root.join("jsonschemas/nextest.json")
}
