// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::{CargoConfigError, CargoConfigParseError, InvalidCargoCliConfigReason};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use std::collections::BTreeMap;
use toml_edit::Item;
use tracing::debug;

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

impl CargoConfigSource {
    /// Returns the directory against which relative paths should be resolved.
    pub(crate) fn resolve_dir<'a>(&'a self, cwd: &'a Utf8Path) -> &'a Utf8Path {
        match self {
            CargoConfigSource::CliOption => {
                // Use the cwd as specified.
                cwd
            }
            CargoConfigSource::File(file) => {
                // The file is e.g. .cargo/config.toml -- go up two levels.
                file.parent()
                    .expect("got to .cargo")
                    .parent()
                    .expect("got to cwd")
            }
        }
    }
}

/// A store for Cargo config files discovered from disk.
///
/// This is required by [`TargetRunner`](crate::target_runner::TargetRunner) and for target triple
/// discovery.
#[derive(Debug)]
pub struct CargoConfigs {
    cli_configs: Vec<(CargoConfigSource, CargoConfig)>,
    cwd: Utf8PathBuf,
    discovered: Vec<(CargoConfigSource, CargoConfig)>,
    target_paths: Vec<Utf8PathBuf>,
}

impl CargoConfigs {
    /// Discover Cargo config files using the same algorithm that Cargo uses.
    pub fn new(
        cli_configs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, CargoConfigError> {
        let cwd = std::env::current_dir()
            .map_err(CargoConfigError::GetCurrentDir)
            .and_then(|cwd| {
                Utf8PathBuf::try_from(cwd).map_err(CargoConfigError::CurrentDirInvalidUtf8)
            })?;
        let cli_configs = parse_cli_configs(&cwd, cli_configs.into_iter())?;
        let discovered = discover_impl(&cwd, None)?;

        // Used for target discovery.
        let mut target_paths = Vec::new();
        let target_path_env = std::env::var_os("RUST_TARGET_PATH").unwrap_or_default();
        for path in std::env::split_paths(&target_path_env) {
            match Utf8PathBuf::try_from(path) {
                Ok(path) => target_paths.push(path),
                Err(error) => {
                    debug!("for RUST_TARGET_PATH, {error}");
                }
            }
        }

        Ok(Self {
            cli_configs,
            cwd,
            discovered,
            target_paths,
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
        target_paths: Vec<Utf8PathBuf>,
    ) -> Result<Self, CargoConfigError> {
        let cli_configs = parse_cli_configs(cwd, cli_configs.into_iter())?;
        let discovered = discover_impl(cwd, Some(terminate_search_at))?;

        Ok(Self {
            cli_configs,
            cwd: cwd.to_owned(),
            discovered,
            target_paths,
        })
    }

    pub(crate) fn cwd(&self) -> &Utf8Path {
        &self.cwd
    }

    pub(crate) fn discovered_configs(
        &self,
    ) -> impl DoubleEndedIterator<Item = DiscoveredConfig<'_>> + '_ {
        // NOTE: The order is:
        // 1. --config k=v
        // 2. --config <file>
        // 3. Environment variables
        // 4. .cargo/configs.
        //
        // 2 and 3 used to be reversed in older versions of Rust, but this has been fixed as of Rust
        // 1.68 (https://github.com/rust-lang/cargo/pull/11077).
        let cli_option_iter = self
            .cli_configs
            .iter()
            .filter(|(source, _)| matches!(source, CargoConfigSource::CliOption))
            .map(|(source, config)| DiscoveredConfig::CliOption { config, source });

        let cli_file_iter = self
            .cli_configs
            .iter()
            .filter(|(source, _)| matches!(source, CargoConfigSource::File(_)))
            .map(|(source, config)| DiscoveredConfig::File { config, source });

        let cargo_config_file_iter = self
            .discovered
            .iter()
            .map(|(source, config)| DiscoveredConfig::File { config, source });

        cli_option_iter
            .chain(cli_file_iter)
            .chain(std::iter::once(DiscoveredConfig::Env))
            .chain(cargo_config_file_iter)
    }

    pub(crate) fn target_paths(&self) -> &[Utf8PathBuf] {
        &self.target_paths
    }
}

pub(crate) enum DiscoveredConfig<'a> {
    CliOption {
        config: &'a CargoConfig,
        source: &'a CargoConfigSource,
    },
    // Sentinel value to indicate to users that they should look up their config in the environment.
    Env,
    File {
        config: &'a CargoConfig,
        source: &'a CargoConfigSource,
    },
}

fn parse_cli_configs(
    cwd: &Utf8Path,
    cli_configs: impl Iterator<Item = impl AsRef<str>>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigError> {
    cli_configs
        .into_iter()
        .map(|config_str| {
            // Each cargo config is expected to be a valid TOML file.
            let config_str = config_str.as_ref();

            let as_path = cwd.join(config_str);
            if as_path.exists() {
                // Read this config as a file.
                load_file(as_path)
            } else {
                let config = parse_cli_config(config_str)?;
                Ok((CargoConfigSource::CliOption, config))
            }
        })
        .collect()
}

fn parse_cli_config(config_str: &str) -> Result<CargoConfig, CargoConfigError> {
    // This implementation is copied over from https://github.com/rust-lang/cargo/pull/10176.

    // We only want to allow "dotted key" (see https://toml.io/en/v1.0.0#keys)
    // expressions followed by a value that's not an "inline table"
    // (https://toml.io/en/v1.0.0#inline-table). Easiest way to check for that is to
    // parse the value as a toml_edit::DocumentMut, and check that the (single)
    // inner-most table is set via dotted keys.
    let doc: toml_edit::DocumentMut =
        config_str
            .parse()
            .map_err(|error| CargoConfigError::CliConfigParseError {
                config_str: config_str.to_owned(),
                error,
            })?;

    fn non_empty(d: Option<&toml_edit::RawString>) -> bool {
        d.is_some_and(|p| !p.as_str().unwrap_or_default().trim().is_empty())
    }
    fn non_empty_decor(d: &toml_edit::Decor) -> bool {
        non_empty(d.prefix()) || non_empty(d.suffix())
    }
    fn non_empty_key_decor(k: &toml_edit::Key) -> bool {
        non_empty_decor(k.leaf_decor()) || non_empty_decor(k.dotted_decor())
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
                    if table.key(k).is_some_and(non_empty_key_decor) || non_empty_decor(nt.decor())
                    {
                        return Err(CargoConfigError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        });
                    }
                    table = nt;
                }
                Item::Value(v) if v.is_inline_table() => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToInlineTable,
                    });
                }
                Item::Value(v) => {
                    if table
                        .key(k)
                        .is_some_and(|k| non_empty(k.leaf_decor().prefix()))
                        || non_empty_decor(v.decor())
                    {
                        return Err(CargoConfigError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        });
                    }
                    got_to_value = true;
                    break;
                }
                Item::ArrayOfTables(_) => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToArrayOfTables,
                    });
                }
                Item::None => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::DoesntProvideValue,
                    });
                }
            }
        }
        got_to_value
    };
    if !ok {
        return Err(CargoConfigError::InvalidCliConfig {
            config_str: config_str.to_owned(),
            reason: InvalidCargoCliConfigReason::NotDottedKv,
        });
    }

    let cargo_config: CargoConfig =
        toml_edit::de::from_document(doc).map_err(|error| CargoConfigError::CliConfigDeError {
            config_str: config_str.to_owned(),
            error,
        })?;

    // Note: environment variables parsed from CLI configs can't be relative. However, this isn't
    // necessary to check because the only way to specify that is as an inline table, which is
    // rejected above.

    Ok(cargo_config)
}

fn discover_impl(
    start_search_at: &Utf8Path,
    terminate_search_at: Option<&Utf8Path>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigError> {
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
        CargoConfigError::FailedPathCanonicalization {
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
            .map_err(CargoConfigError::GetCargoHome)
            .and_then(|home| Utf8PathBuf::try_from(home).map_err(CargoConfigError::NonUtf8Path))?;

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
        .map(load_file)
        .collect::<Result<Vec<_>, CargoConfigError>>()?;

    Ok(configs)
}

fn load_file(
    path: impl Into<Utf8PathBuf>,
) -> Result<(CargoConfigSource, CargoConfig), CargoConfigError> {
    let path = path.into();
    let path = path
        .canonicalize_utf8()
        .map_err(|error| CargoConfigError::FailedPathCanonicalization { path, error })?;

    let config_contents =
        std::fs::read_to_string(&path).map_err(|error| CargoConfigError::ConfigReadError {
            path: path.clone(),
            error,
        })?;
    let config: CargoConfig = toml::from_str(&config_contents).map_err(|error| {
        CargoConfigError::from(Box::new(CargoConfigParseError {
            path: path.clone(),
            error,
        }))
    })?;
    Ok((CargoConfigSource::File(path), config))
}

#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum CargoConfigEnv {
    Value(String),
    Fields {
        value: String,
        force: Option<bool>,
        relative: Option<bool>,
    },
}

impl CargoConfigEnv {
    pub(super) fn into_value(self) -> String {
        match self {
            Self::Value(v) => v,
            Self::Fields { value, .. } => value,
        }
    }

    pub(super) fn force(&self) -> Option<bool> {
        match self {
            Self::Value(_) => None,
            Self::Fields { force, .. } => *force,
        }
    }

    pub(super) fn relative(&self) -> Option<bool> {
        match self {
            Self::Value(_) => None,
            Self::Fields { relative, .. } => *relative,
        }
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfig {
    #[serde(default)]
    pub(crate) build: CargoConfigBuild,
    pub(crate) target: Option<BTreeMap<String, CargoConfigRunner>>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, CargoConfigEnv>,
    #[serde(default)]
    pub(crate) term: CargoConfigTerm,
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

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoConfigTerm {
    #[serde(default)]
    pub(crate) progress: CargoConfigTermProgress,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoConfigTermProgress {
    #[serde(default)]
    pub(crate) term_integration: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
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
            CargoConfigError::InvalidCliConfig { reason, .. } => reason,
            other => panic!(
                "expected input {arg} to fail with InvalidCliConfig, actual failure: {other}"
            ),
        };

        assert_eq!(
            expected_reason, actual_reason,
            "expected reason for failure doesn't match actual reason"
        );
    }
}
