// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for emulating Cargo's configuration file discovery.
//!
//! Since `cargo config get` is not stable as of Rust 1.61, nextest must do its own config file
//! search.

use crate::errors::{
    CargoConfigSearchError, CargoConfigsConstructError, InvalidCargoCliConfigReason,
    TargetTripleError,
};
use camino::{Utf8Path, Utf8PathBuf};
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::{collections::BTreeMap, fmt};
use toml_edit::Item;

/// Represents a target triple that's being cross-compiled against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetTriple {
    /// The target triple being built.
    pub triple: String,

    /// The source the triple came from.
    pub source: TargetTripleSource,
}

impl TargetTriple {
    /// Find the target triple being built.
    ///
    /// This does so by looking at, in order:
    ///
    /// 1. the passed in --target CLI option
    /// 2. the CARGO_BUILD_TARGET env var
    /// 3. build.target in Cargo config files
    ///
    /// Note that currently this only supports triples, not JSON files.
    pub fn find(
        cargo_configs: &CargoConfigs,
        target_cli_option: Option<&str>,
    ) -> Result<Option<Self>, TargetTripleError> {
        // First, look at the CLI option passed in.
        if let Some(triple) = target_cli_option {
            return Ok(Some(TargetTriple {
                triple: triple.to_owned(),
                source: TargetTripleSource::CliOption,
            }));
        }

        // Next, look at the CARGO_BUILD_TARGET env var.
        if let Some(triple) = Self::from_env()? {
            return Ok(Some(triple));
        }

        // Finally, look at the cargo configs.
        Self::from_cargo_configs(cargo_configs)
    }

    /// The environment variable used for target searches
    pub const CARGO_BUILD_TARGET_ENV: &'static str = "CARGO_BUILD_TARGET";

    fn from_env() -> Result<Option<Self>, TargetTripleError> {
        if let Some(triple_val) = std::env::var_os(Self::CARGO_BUILD_TARGET_ENV) {
            let triple = triple_val
                .into_string()
                .map_err(|_osstr| TargetTripleError::InvalidEnvironmentVar)?;
            Ok(Some(Self {
                triple,
                source: TargetTripleSource::Env,
            }))
        } else {
            Ok(None)
        }
    }

    fn from_cargo_configs(cargo_configs: &CargoConfigs) -> Result<Option<Self>, TargetTripleError> {
        for (source, config) in cargo_configs.discovered_configs()? {
            if let Some(triple) = &config.build.target {
                return Ok(Some(TargetTriple {
                    triple: triple.to_owned(),
                    source: TargetTripleSource::CargoConfig {
                        source: source.clone(),
                    },
                }));
            }
        }

        Ok(None)
    }
}

/// The place where a target triple's configuration was picked up from.
///
/// This is the type of [`TargetTriple::source`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetTripleSource {
    /// The target triple was defined by the --target CLI option.
    CliOption,

    /// The target triple was defined by the `CARGO_BUILD_TARGET` env var.
    Env,

    /// The platform runner was defined through a `.cargo/config.toml` or `.cargo/config` file, or a
    /// `--config` CLI option.
    CargoConfig {
        /// The source of the configuration.
        source: CargoConfigSource,
    },
}

impl fmt::Display for TargetTripleSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliOption => {
                write!(f, "--target option")
            }
            Self::Env => {
                write!(f, "environment variable `CARGO_BUILD_TARGET`")
            }
            Self::CargoConfig {
                source: CargoConfigSource::CliOption,
            } => {
                write!(f, "`build.target` specified by `--config`")
            }

            Self::CargoConfig {
                source: CargoConfigSource::File(path),
            } => {
                write!(f, "`build.target` within `{path}`")
            }
        }
    }
}

/// The source of a Cargo config.
///
/// A Cargo config can be specified as a CLI option (unstable) or a `.cargo/config.toml` file on
/// disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CargoConfigSource {
    /// A Cargo config provided as a CLI option.
    CliOption,

    /// A Cargo config provided as a file on disk.
    File(Utf8PathBuf),
}

/// A store for Cargo config files discovered from disk.
///
/// This is required by [`TargetRunner`](crate::target_runner::TargetRunner) and for target triple
/// discovery.
#[derive(Debug)]
pub struct CargoConfigs {
    cli_configs: Vec<(CargoConfigSource, CargoConfig)>,
    cwd: Utf8PathBuf,
    terminate_search_at: Option<Utf8PathBuf>,
    discovered: OnceCell<Vec<(CargoConfigSource, CargoConfig)>>,
}

impl CargoConfigs {
    /// Discover Cargo config files using the same algorithm that Cargo uses.
    pub fn new(
        cli_configs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, CargoConfigsConstructError> {
        let cli_configs = parse_cli_configs(cli_configs.into_iter())?;
        let cwd = std::env::current_dir()
            .map_err(CargoConfigsConstructError::GetCurrentDir)
            .and_then(|cwd| {
                Utf8PathBuf::try_from(cwd)
                    .map_err(CargoConfigsConstructError::CurrentDirInvalidUtf8)
            })?;

        Ok(Self {
            cli_configs,
            cwd,
            terminate_search_at: None,
            discovered: OnceCell::new(),
        })
    }

    /// Discover Cargo config files with isolation.
    ///
    /// Not part of the public API, for testing only.
    #[doc(hidden)]
    pub fn new_with_isolation(
        cli_configs: impl IntoIterator<Item = impl AsRef<str>>,
        cwd: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Result<Self, CargoConfigsConstructError> {
        let cli_configs = parse_cli_configs(cli_configs.into_iter())?;

        Ok(Self {
            cli_configs,
            cwd: cwd.to_owned(),
            terminate_search_at: Some(terminate_search_at.to_owned()),
            discovered: OnceCell::new(),
        })
    }

    pub(crate) fn cwd(&self) -> &Utf8Path {
        &self.cwd
    }

    pub(crate) fn discovered_configs(
        &self,
    ) -> Result<
        impl Iterator<Item = &(CargoConfigSource, CargoConfig)> + DoubleEndedIterator + '_,
        CargoConfigSearchError,
    > {
        let cli_iter = self.cli_configs.iter();
        let file_iter = self
            .discovered
            .get_or_try_init(|| discover_impl(&self.cwd, self.terminate_search_at.as_deref()))?
            .iter();
        Ok(cli_iter.chain(file_iter))
    }
}

fn parse_cli_configs(
    cli_configs: impl Iterator<Item = impl AsRef<str>>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigsConstructError> {
    cli_configs
        .into_iter()
        .map(|config_str| {
            // Each cargo config is expected to be a valid TOML file.
            let config_str = config_str.as_ref();
            let config = parse_cli_config(config_str)?;
            Ok((CargoConfigSource::CliOption, config))
        })
        .collect()
}

fn parse_cli_config(config_str: &str) -> Result<CargoConfig, CargoConfigsConstructError> {
    // This implementation is copied over from https://github.com/rust-lang/cargo/pull/10176.

    // We only want to allow "dotted key" (see https://toml.io/en/v1.0.0#keys)
    // expressions followed by a value that's not an "inline table"
    // (https://toml.io/en/v1.0.0#inline-table). Easiest way to check for that is to
    // parse the value as a toml_edit::Document, and check that the (single)
    // inner-most table is set via dotted keys.
    let doc: toml_edit::Document =
        config_str
            .parse()
            .map_err(|error| CargoConfigsConstructError::CliConfigParseError {
                config_str: config_str.to_owned(),
                error,
            })?;

    fn non_empty_decor(d: &toml_edit::Decor) -> bool {
        d.prefix().map_or(false, |p| !p.trim().is_empty())
            || d.suffix().map_or(false, |s| !s.trim().is_empty())
    }

    let ok = {
        let mut got_to_value = false;
        let mut table = doc.as_table();
        let mut is_root = true;
        while table.is_dotted() || is_root {
            is_root = false;
            if table.len() != 1 {
                break;
            }
            let (k, n) = table.iter().next().expect("len() == 1 above");
            match n {
                Item::Table(nt) => {
                    if table.key_decor(k).map_or(false, non_empty_decor)
                        || non_empty_decor(nt.decor())
                    {
                        return Err(CargoConfigsConstructError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        })?;
                    }
                    table = nt;
                }
                Item::Value(v) if v.is_inline_table() => {
                    return Err(CargoConfigsConstructError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToInlineTable,
                    })?;
                }
                Item::Value(v) => {
                    if non_empty_decor(v.decor()) {
                        return Err(CargoConfigsConstructError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        })?;
                    }
                    got_to_value = true;
                    break;
                }
                Item::ArrayOfTables(_) => {
                    return Err(CargoConfigsConstructError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToArrayOfTables,
                    })?;
                }
                Item::None => {
                    return Err(CargoConfigsConstructError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::DoesntProvideValue,
                    })?;
                }
            }
        }
        got_to_value
    };
    if !ok {
        return Err(CargoConfigsConstructError::InvalidCliConfig {
            config_str: config_str.to_owned(),
            reason: InvalidCargoCliConfigReason::NotDottedKv,
        })?;
    }

    let cargo_config: CargoConfig = toml_edit::easy::from_document(doc).map_err(|error| {
        CargoConfigsConstructError::CliConfigDeError {
            config_str: config_str.to_owned(),
            error,
        }
    })?;
    Ok(cargo_config)
}

fn discover_impl(
    start_search_at: &Utf8Path,
    terminate_search_at: Option<&Utf8Path>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigSearchError> {
    fn read_config_dir(dir: &mut Utf8PathBuf) -> Option<Utf8PathBuf> {
        // Check for config before config.toml, same as cargo does
        dir.push("config");

        if !dir.exists() {
            dir.set_extension("toml");
        }

        let ret = if dir.exists() {
            Some(dir.clone())
        } else {
            None
        };

        dir.pop();
        ret
    }

    let mut dir = start_search_at.canonicalize_utf8().map_err(|error| {
        CargoConfigSearchError::FailedPathCanonicalization {
            path: start_search_at.to_owned(),
            error,
        }
    })?;

    let mut config_paths = Vec::new();

    for _ in 0..dir.ancestors().count() {
        dir.push(".cargo");

        if !dir.exists() {
            dir.pop();
            dir.pop();
            continue;
        }

        if let Some(path) = read_config_dir(&mut dir) {
            config_paths.push(path);
        }

        dir.pop();
        if Some(dir.as_path()) == terminate_search_at {
            break;
        }
        dir.pop();
    }

    if terminate_search_at.is_none() {
        // Attempt lookup the $CARGO_HOME directory from the cwd, as that can
        // contain a default config.toml
        let mut cargo_home_path = home::cargo_home_with_cwd(start_search_at.as_std_path())
            .map_err(CargoConfigSearchError::GetCargoHome)
            .and_then(|home| {
                Utf8PathBuf::try_from(home).map_err(CargoConfigSearchError::NonUtf8Path)
            })?;

        if let Some(home_config) = read_config_dir(&mut cargo_home_path) {
            // Ensure we don't add a duplicate if the current directory is underneath
            // the same root as $CARGO_HOME
            if !config_paths.iter().any(|path| path == &home_config) {
                config_paths.push(home_config);
            }
        }
    }

    let configs = config_paths
        .into_iter()
        .map(|path| {
            let config_contents = std::fs::read_to_string(&path).map_err(|error| {
                CargoConfigSearchError::ConfigReadError {
                    path: path.clone(),
                    error,
                }
            })?;
            let config: CargoConfig =
                toml_edit::easy::from_str(&config_contents).map_err(|error| {
                    CargoConfigSearchError::ConfigParseError {
                        path: path.clone(),
                        error,
                    }
                })?;
            Ok((CargoConfigSource::File(path), config))
        })
        .collect::<Result<Vec<_>, CargoConfigSearchError>>()?;

    Ok(configs)
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfig {
    #[serde(default)]
    pub(crate) build: CargoConfigBuild,
    pub(crate) target: Option<BTreeMap<String, CargoConfigRunner>>,
}

#[derive(Deserialize, Default, Debug)]
pub(crate) struct CargoConfigBuild {
    pub(crate) target: Option<String>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfigRunner {
    #[serde(default)]
    pub(crate) runner: Option<Runner>,
}

#[derive(Clone, Deserialize, Debug, Eq, PartialEq)]
#[serde(untagged)]
pub(crate) enum Runner {
    Simple(String),
    List(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;
    use color_eyre::eyre::{Context, Result};
    use tempfile::TempDir;
    use test_case::test_case;

    #[test]
    fn test_cli_kv_accepted() {
        // These dotted key expressions should all be fine.
        let config = parse_cli_config("build.target=\"aarch64-unknown-linux-gnu\"")
            .expect("dotted config should parse correctly");
        assert_eq!(
            config.build.target.as_deref(),
            Some("aarch64-unknown-linux-gnu")
        );

        let config = parse_cli_config(" target.\"aarch64-unknown-linux-gnu\".runner = 'test' ")
            .expect("dotted config should parse correctly");
        assert_eq!(
            config.target.as_ref().unwrap()["aarch64-unknown-linux-gnu"].runner,
            Some(Runner::Simple("test".to_owned()))
        );

        // But anything that's not a dotted key expression should be disallowed.
        let _ = parse_cli_config("[a] foo=true").unwrap_err();
        let _ = parse_cli_config("a = true\nb = true").unwrap_err();

        // We also disallow overwriting with tables since it makes merging unclear.
        let _ = parse_cli_config("a = { first = true, second = false }").unwrap_err();
        let _ = parse_cli_config("a = { first = true }").unwrap_err();
    }

    #[test_case(
        "",
        InvalidCargoCliConfigReason::NotDottedKv

        ; "empty input")]
    #[test_case(
        "a.b={c = \"d\"}",
        InvalidCargoCliConfigReason::SetsValueToInlineTable

        ; "no inline table value")]
    #[test_case(
        "[[a.b]]\nc = \"d\"",
        InvalidCargoCliConfigReason::NotDottedKv

        ; "no array of tables")]
    #[test_case(
        "a.b = \"c\" # exactly",
        InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration

        ; "no comments after")]
    #[test_case(
        "# exactly\na.b = \"c\"",
        InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration

        ; "no comments before")]
    fn test_invalid_cli_config_reason(arg: &str, expected_reason: InvalidCargoCliConfigReason) {
        // Disallow inline tables
        let err = parse_cli_config(arg).unwrap_err();
        let actual_reason = match err {
            CargoConfigsConstructError::InvalidCliConfig { reason, .. } => reason,
            other => panic!(
                "expected input {arg} to fail with InvalidCliConfig, actual failure: {other}"
            ),
        };

        assert_eq!(
            expected_reason, actual_reason,
            "expected reason for failure doesn't match actual reason"
        );
    }

    #[test]
    fn test_find_target_triple() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        assert_eq!(
            find_target_triple(&[], &dir_foo_bar_path, &dir_path),
            Some(TargetTriple {
                triple: "x86_64-unknown-linux-gnu".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/bar/.cargo/config.toml")),
                },
            }),
        );

        assert_eq!(
            find_target_triple(&[], &dir_foo_path, &dir_path),
            Some(TargetTriple {
                triple: "x86_64-pc-windows-msvc".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/.cargo/config")),
                },
            }),
        );

        assert_eq!(
            find_target_triple(
                &["build.target=\"aarch64-unknown-linux-gnu\""],
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "aarch64-unknown-linux-gnu".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
            })
        );

        // --config arguments are followed left to right.
        assert_eq!(
            find_target_triple(
                &[
                    "build.target=\"aarch64-unknown-linux-gnu\"",
                    "build.target=\"x86_64-unknown-linux-musl\""
                ],
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "aarch64-unknown-linux-gnu".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
            })
        );

        assert_eq!(find_target_triple(&[], &dir_path, &dir_path), None);
    }

    fn setup_temp_dir() -> Result<TempDir> {
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

        Ok(dir)
    }

    fn find_target_triple(
        cli_configs: &[&str],
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Option<TargetTriple> {
        let configs =
            CargoConfigs::new_with_isolation(cli_configs, start_search_at, terminate_search_at)
                .unwrap();
        TargetTriple::from_cargo_configs(&configs).unwrap()
    }

    static FOO_CARGO_CONFIG_CONTENTS: &str = r#"
    [build]
    target = "x86_64-pc-windows-msvc"
    "#;

    static FOO_BAR_CARGO_CONFIG_CONTENTS: &str = r#"
    [build]
    target = "x86_64-unknown-linux-gnu"
    "#;
}
