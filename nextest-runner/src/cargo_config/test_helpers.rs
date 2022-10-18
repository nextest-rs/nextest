// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use color_eyre::eyre::{Context, Result};
use tempfile::TempDir;

pub(super) fn setup_temp_dir() -> Result<TempDir> {
    let dir = tempfile::Builder::new()
        .tempdir()
        .wrap_err("error creating tempdir")?;

    std::fs::create_dir_all(dir.path().join("foo/.cargo"))
        .wrap_err("error creating foo/.cargo subdir")?;
    std::fs::create_dir_all(dir.path().join("foo/bar/.cargo"))
        .wrap_err("error creating foo/bar/.cargo subdir")?;

    std::fs::write(
        dir.path().join("foo/.cargo/config"),
        FOO_CARGO_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/.cargo/config")?;
    std::fs::write(
        dir.path().join("foo/bar/.cargo/config.toml"),
        FOO_BAR_CARGO_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/bar/.cargo/config.toml")?;
    std::fs::write(
        dir.path().join("foo/extra-config.toml"),
        FOO_EXTRA_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/extra-config.toml")?;

    Ok(dir)
}

static FOO_CARGO_CONFIG_CONTENTS: &str = r#"
[build]
target = "x86_64-pc-windows-msvc"

[env]
SOME_VAR = { value = "foo-config", force = true }
"#;

static FOO_BAR_CARGO_CONFIG_CONTENTS: &str = r#"
[build]
target = "x86_64-unknown-linux-gnu"

[env]
SOME_VAR = { value = "foo-bar-config", force = true }
"#;

static FOO_EXTRA_CONFIG_CONTENTS: &str = r#"
[build]
target = "aarch64-unknown-linux-gnu"
"#;
