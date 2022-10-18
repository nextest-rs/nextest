// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for emulating Cargo's configuration file discovery.
//!
//! Since `cargo config get` is not stable as of Rust 1.61, nextest must do its own config file
//! search.

use crate::errors::{CargoConfigError, InvalidCargoCliConfigReason, TargetTripleError};
use camino::{Utf8Path, Utf8PathBuf};
use nextest_metadata::{CargoEnvironmentVariable, EnvironmentMap};
use serde::Deserialize;
use std::{collections::BTreeMap, fmt};
use target_spec::Platform;
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
    /// Converts a target triple to a `String` that can be stored in the build-metadata.
    /// cargo-nextest represents the host triple with `None` during runtime.
    /// However the build-metadata might be used on a system with a different host triple.
    /// Therefore the host triple is detected if `target_triple` is `None`
    pub fn serialize(target_triple: Option<&TargetTriple>) -> Option<String> {
        if let Some(target) = &target_triple {
            Some(target.triple.clone())
        } else {
            match Platform::current() {
                Ok(host) => Some(host.triple_str().to_owned()),
                Err(err) => {
                    log::warn!(
                        "failed to detect host target: {err}!\n cargo nextest may use the wrong test runner for this archive."
                    );
                    None
                }
            }
        }
    }

    /// Converts a `String` that was output by `TargetTriple::serialize` back to a target triple.
    /// This target triple is assumed to orginiate from a build-metadata config.
    pub fn deserialize(target_triple: Option<String>) -> Option<TargetTriple> {
        Some(TargetTriple {
            triple: target_triple?,
            source: TargetTripleSource::Metadata,
        })
    }

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
        for discovered_config in cargo_configs.discovered_configs() {
            match discovered_config {
                DiscoveredConfig::CliOption { config, source }
                | DiscoveredConfig::File { config, source } => {
                    if let Some(triple) = &config.build.target {
                        return Ok(Some(TargetTriple {
                            triple: triple.to_owned(),
                            source: TargetTripleSource::CargoConfig {
                                source: source.clone(),
                            },
                        }));
                    }
                }
                DiscoveredConfig::Env => {
                    // Look at the CARGO_BUILD_TARGET env var.
                    if let Some(triple) = Self::from_env()? {
                        return Ok(Some(triple));
                    }
                }
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

    /// The platform runner was defined trough a metadata file provided using the --archive-file or
    /// the `--binaries-metadata` CLI option
    Metadata,
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
            Self::Metadata => {
                write!(f, "--archive-file or --binaries-metadata option")
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
    discovered: Vec<(CargoConfigSource, CargoConfig)>,
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

        Ok(Self {
            cli_configs,
            cwd,
            discovered,
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
    ) -> Result<Self, CargoConfigError> {
        let cli_configs = parse_cli_configs(cwd, cli_configs.into_iter())?;
        let discovered = discover_impl(&cwd, Some(terminate_search_at))?;

        Ok(Self {
            cli_configs,
            cwd: cwd.to_owned(),
            discovered,
        })
    }

    pub(crate) fn cwd(&self) -> &Utf8Path {
        &self.cwd
    }

    /// The environment variables to set when running Cargo commands.
    pub fn env(&self) -> EnvironmentMap {
        self.discovered_configs()
            .filter_map(|config| match config {
                DiscoveredConfig::CliOption { config, source }
                | DiscoveredConfig::File { config, source } => Some((config, source)),
                DiscoveredConfig::Env => None,
            })
            .flat_map(|(config, source)| {
                let source = match source {
                    CargoConfigSource::CliOption => None,
                    CargoConfigSource::File(path) => Some(path.clone()),
                };
                config
                    .env
                    .clone()
                    .into_iter()
                    .map(move |(name, value)| (source.clone(), name, value))
            })
            .map(|(source, name, value)| match value {
                CargoConfigEnv::Value(value) => CargoEnvironmentVariable {
                    source,
                    name,
                    value,
                    force: false,
                    relative: false,
                },
                CargoConfigEnv::Fields {
                    value,
                    force,
                    relative,
                } => CargoEnvironmentVariable {
                    source,
                    name,
                    value,
                    force,
                    relative,
                },
            })
            .collect()
    }

    pub(crate) fn discovered_configs(
        &self,
    ) -> impl Iterator<Item = DiscoveredConfig<'_>> + DoubleEndedIterator + '_ {
        // TODO/NOTE: https://github.com/rust-lang/cargo/issues/10992 means that currently
        // environment variables are privileged over files passed in over the CLI. Once this
        // behavior is fixed in upstream cargo, it should also be fixed here.
        let cli_option_iter = self.cli_configs.iter().filter_map(|(source, config)| {
            matches!(source, CargoConfigSource::CliOption)
                .then(|| DiscoveredConfig::CliOption { config, source })
        });

        let cli_file_iter = self.cli_configs.iter().filter_map(|(source, config)| {
            matches!(source, CargoConfigSource::File(_))
                .then(|| DiscoveredConfig::File { config, source })
        });

        let cargo_config_file_iter = self
            .discovered
            .iter()
            .map(|(source, config)| DiscoveredConfig::File { config, source });

        cli_option_iter
            .chain(std::iter::once(DiscoveredConfig::Env))
            .chain(cli_file_iter)
            .chain(cargo_config_file_iter)
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
    // parse the value as a toml_edit::Document, and check that the (single)
    // inner-most table is set via dotted keys.
    let doc: toml_edit::Document =
        config_str
            .parse()
            .map_err(|error| CargoConfigError::CliConfigParseError {
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
                        return Err(CargoConfigError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        })?;
                    }
                    table = nt;
                }
                Item::Value(v) if v.is_inline_table() => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToInlineTable,
                    })?;
                }
                Item::Value(v) => {
                    if non_empty_decor(v.decor()) {
                        return Err(CargoConfigError::InvalidCliConfig {
                            config_str: config_str.to_owned(),
                            reason: InvalidCargoCliConfigReason::IncludesNonWhitespaceDecoration,
                        })?;
                    }
                    got_to_value = true;
                    break;
                }
                Item::ArrayOfTables(_) => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::SetsValueToArrayOfTables,
                    })?;
                }
                Item::None => {
                    return Err(CargoConfigError::InvalidCliConfig {
                        config_str: config_str.to_owned(),
                        reason: InvalidCargoCliConfigReason::DoesntProvideValue,
                    })?;
                }
            }
        }
        got_to_value
    };
    if !ok {
        return Err(CargoConfigError::InvalidCliConfig {
            config_str: config_str.to_owned(),
            reason: InvalidCargoCliConfigReason::NotDottedKv,
        })?;
    }

    let cargo_config: CargoConfig = toml_edit::easy::from_document(doc).map_err(|error| {
        CargoConfigError::CliConfigDeError {
            config_str: config_str.to_owned(),
            error,
        }
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

    let config_contents =
        std::fs::read_to_string(&path).map_err(|error| CargoConfigError::ConfigReadError {
            path: path.clone(),
            error,
        })?;
    let config: CargoConfig = toml_edit::easy::from_str(&config_contents).map_err(|error| {
        CargoConfigError::ConfigParseError {
            path: path.clone(),
            error,
        }
    })?;
    Ok((CargoConfigSource::File(path), config))
}

/// Returns the directory against which relative paths are computed for the given config path.
pub(crate) fn relative_dir_for(config_path: &Utf8Path) -> Option<&Utf8Path> {
    // Need to call parent() twice here, since in Cargo land relative means relative to the *parent*
    // of the directory the config is in. First parent() gets the directory the config is in, and
    // the second one gets the parent of that.
    let relative_dir = config_path.parent()?.parent()?;

    // On Windows, remove the UNC prefix since Cargo does so as well.
    Some(strip_unc_prefix(relative_dir))
}

#[cfg(windows)]
#[inline]
fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
    dunce::simplified(path.as_std_path())
        .try_into()
        .expect("stripping verbatim components from a UTF-8 path should result in a UTF-8 path")
}

#[cfg(not(windows))]
#[inline]
fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
    path
}

#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum CargoConfigEnv {
    Value(String),
    Fields {
        value: String,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        relative: bool,
    },
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfig {
    #[serde(default)]
    pub(crate) build: CargoConfigBuild,
    pub(crate) target: Option<BTreeMap<String, CargoConfigRunner>>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, CargoConfigEnv>,
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

    #[test]
    fn test_find_target_triple() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        assert_eq!(
            find_target_triple(&[], None, &dir_foo_bar_path, &dir_path),
            Some(TargetTriple {
                triple: "x86_64-unknown-linux-gnu".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/bar/.cargo/config.toml")),
                },
            }),
        );

        assert_eq!(
            find_target_triple(&[], None, &dir_foo_path, &dir_path),
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
                None,
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
                None,
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

        // --config is preferred over the environment.
        assert_eq!(
            find_target_triple(
                &["build.target=\"aarch64-unknown-linux-gnu\"",],
                Some("aarch64-pc-windows-msvc"),
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

        // The environment is preferred over local paths.
        assert_eq!(
            find_target_triple(
                &[],
                Some("aarch64-pc-windows-msvc"),
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "aarch64-pc-windows-msvc".into(),
                source: TargetTripleSource::Env,
            })
        );

        // --config <path> should be parsed correctly. Config files currently come after
        // keys and values passed in via --config, and after the environment.
        assert_eq!(
            find_target_triple(&["extra-config.toml"], None, &dir_foo_path, &dir_path),
            Some(TargetTriple {
                triple: "aarch64-unknown-linux-gnu".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_foo_path.join("extra-config.toml")),
                },
            })
        );
        assert_eq!(
            find_target_triple(
                &["extra-config.toml"],
                Some("aarch64-pc-windows-msvc"),
                &dir_foo_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "aarch64-pc-windows-msvc".into(),
                source: TargetTripleSource::Env,
            })
        );
        assert_eq!(
            find_target_triple(
                &[
                    "../extra-config.toml",
                    "build.target=\"x86_64-unknown-linux-musl\"",
                ],
                None,
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "x86_64-unknown-linux-musl".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
            })
        );
        assert_eq!(
            find_target_triple(
                &[
                    "build.target=\"x86_64-unknown-linux-musl\"",
                    "extra-config.toml",
                ],
                None,
                &dir_foo_path,
                &dir_path
            ),
            Some(TargetTriple {
                triple: "x86_64-unknown-linux-musl".into(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
            })
        );

        assert_eq!(find_target_triple(&[], None, &dir_path, &dir_path), None);
    }

    #[test]
    fn test_env_var_precedence() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        let configs =
            CargoConfigs::new_with_isolation(&[] as &[&str], &dir_foo_bar_path, &dir_path).unwrap();
        let env = configs.env();
        let env_values: Vec<&str> = env.iter().map(|elem| elem.value.as_str()).collect();
        assert_eq!(env_values, vec!["foo-bar-config", "foo-config"]);

        let configs = CargoConfigs::new_with_isolation(
            &["env.SOME_VAR=\"cli-config\""],
            &dir_foo_bar_path,
            &dir_path,
        )
        .unwrap();
        let env = configs.env();
        let env_values: Vec<&str> = env.iter().map(|elem| elem.value.as_str()).collect();
        assert_eq!(
            env_values,
            vec!["cli-config", "foo-bar-config", "foo-config"]
        );
    }

    #[test]
    fn test_cli_env_var_relative() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        CargoConfigs::new_with_isolation(
            &["env.SOME_VAR={value = \"path\", relative = true }"],
            &dir_foo_bar_path,
            &dir_path,
        )
        .expect_err("CLI configs can't be relative");

        CargoConfigs::new_with_isolation(
            &["env.SOME_VAR.value=\"path\"", "env.SOME_VAR.relative=true"],
            &dir_foo_bar_path,
            &dir_path,
        )
        .expect_err("CLI configs can't be relative");
    }

    #[test]
    #[cfg(unix)]
    fn test_relative_dir_for_unix() {
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config.toml".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/config".as_ref()),
            Some("/foo".as_ref())
        );
        assert_eq!(relative_dir_for("/foo/config".as_ref()), Some("/".as_ref()));
        assert_eq!(relative_dir_for("/config.toml".as_ref()), None);
    }

    #[test]
    #[cfg(windows)]
    fn test_relative_dir_for_windows() {
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config.toml".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\config".as_ref()),
            Some("C:\\foo".as_ref())
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\config".as_ref()),
            Some("C:\\".as_ref())
        );
        assert_eq!(relative_dir_for("C:\\config.toml".as_ref()), None);
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
        std::fs::write(
            dir.path().join("foo/extra-config.toml"),
            FOO_EXTRA_CONFIG_CONTENTS,
        )
        .wrap_err("error writing foo/extra-config.toml")?;

        Ok(dir)
    }

    fn find_target_triple(
        cli_configs: &[&str],
        env: Option<&str>,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Option<TargetTriple> {
        let configs =
            CargoConfigs::new_with_isolation(cli_configs, start_search_at, terminate_search_at)
                .unwrap();
        if let Some(env) = env {
            std::env::set_var("CARGO_BUILD_TARGET", env);
        }
        let ret = TargetTriple::from_cargo_configs(&configs).unwrap();
        std::env::remove_var("CARGO_BUILD_TARGET");
        ret
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
}
