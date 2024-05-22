// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::TargetTriple;
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use color_eyre::eyre::{Context, Result};
use target_spec::{Platform, TargetFeatures};

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

impl TargetTriple {
    /// Creates an x86_64-unknown-linux-gnu [`TargetTriple`]. Useful for testing.
    pub(crate) fn x86_64_unknown_linux_gnu() -> Self {
        TargetTriple::deserialize_str(Some("x86_64-unknown-linux-gnu".to_owned()))
            .expect("creating TargetTriple from linux gnu triple string should succeed")
            .expect("the output of deserialize_str shouldn't be None")
    }
}
pub(super) fn custom_target_path() -> Utf8PathBuf {
    Utf8PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .join("fixtures/custom-target/my-target.json")
}

pub(super) fn custom_platform() -> Platform {
    let custom_target_json = std::fs::read_to_string(custom_target_path())
        .expect("custom target json read successfully");
    Platform::new_custom("my-target", &custom_target_json, TargetFeatures::Unknown)
        .expect("custom target is valid")
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
