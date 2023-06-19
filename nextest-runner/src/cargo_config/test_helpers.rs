// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use color_eyre::eyre::{Context, Result};

pub(super) fn setup_temp_dir() -> Result<Utf8TempDir> {
    let dir = camino_tempfile::Builder::new()
        .tempdir()
        .wrap_err("error creating tempdir")?;

    std::fs::create_dir_all(dir.path().join("foo/.cargo"))
        .wrap_err("error creating foo/.cargo subdir")?;
    std::fs::create_dir_all(dir.path().join("foo/bar/.cargo"))
        .wrap_err("error creating foo/bar/.cargo subdir")?;
    std::fs::create_dir_all(dir.path().join("foo/bar/custom1/.cargo"))
        .wrap_err("error creating foo/bar/custom1/.cargo subdir")?;
    std::fs::create_dir_all(dir.path().join("foo/bar/custom2/.cargo"))
        .wrap_err("error creating foo/bar/custom2/.cargo subdir")?;

    std::fs::create_dir_all(dir.path().join("custom-target"))
        .wrap_err("error creating custom-target")?;
    let custom_target_path = custom_target_path();
    println!("{custom_target_path}");

    std::fs::copy(
        &custom_target_path,
        dir.path().join("custom-target/my-target.json"),
    )
    .wrap_err("error copying custom target")?;

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
        dir.path().join("foo/bar/custom1/.cargo/config.toml"),
        FOO_BAR_CUSTOM1_CARGO_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/bar/custom1/.cargo/config.toml")?;
    std::fs::write(
        dir.path().join("foo/bar/custom2/.cargo/config.toml"),
        FOO_BAR_CUSTOM2_CARGO_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/bar/custom2/.cargo/config.toml")?;
    std::fs::write(
        dir.path().join("foo/extra-config.toml"),
        FOO_EXTRA_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/extra-config.toml")?;
    std::fs::write(
        dir.path().join("foo/extra-custom-config.toml"),
        FOO_EXTRA_CUSTOM_CONFIG_CONTENTS,
    )
    .wrap_err("error writing foo/extra-custom-config.toml")?;

    Ok(dir)
}

pub(super) fn custom_target_path() -> Utf8PathBuf {
    Utf8PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .join("fixtures/custom-target/my-target.json")
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

static FOO_BAR_CUSTOM1_CARGO_CONFIG_CONTENTS: &str = r#"
[build]
target = "my-target"
"#;

static FOO_BAR_CUSTOM2_CARGO_CONFIG_CONTENTS: &str = r#"
[build]
target = "../../../custom-target/my-target.json"
"#;

static FOO_EXTRA_CONFIG_CONTENTS: &str = r#"
[build]
target = "aarch64-unknown-linux-gnu"
"#;

static FOO_EXTRA_CUSTOM_CONFIG_CONTENTS: &str = r#"
[build]
target = "custom-target/my-target.json"
"#;
