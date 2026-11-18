// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::{CargoConfigError, CargoConfigParseError, InvalidCargoCliConfigReason};
use camino::{Utf8Path, Utf8PathBuf};
use itertools::Itertools;
use serde::{
    Deserialize,
    de::{self, MapAccess, Visitor, value::MapAccessDeserializer},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
};
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
        .map(|config_str| {
            // Each cargo config is expected to be a valid TOML file.
            let config_str = config_str.as_ref();

            let as_path = cwd.join(config_str);
            if as_path.exists() {
                // Read this config as a file, expanding any files it includes.
                load_file(as_path)
            } else {
                let config = parse_cli_config(config_str)?;
                Ok(vec![(CargoConfigSource::CliOption, config)])
            }
        })
        .flatten_ok()
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

    config_paths
        .into_iter()
        .map(load_file)
        .flatten_ok()
        .collect()
}

/// Loads a config file and, recursively, any files it includes via the `include` key.
///
/// The returned configs are in precedence order, highest first, matching the order expected by
/// [`CargoConfigs::discovered_configs`]: a config file's own values take precedence over the files
/// it includes, and among included files, those listed later take precedence over those listed
/// earlier. Includes are resolved depth-first, so a file's entire include subtree outranks the next
/// sibling include.
///
/// Errors if any file is reached more than once while resolving this file's includes, matching
/// Cargo. For example, if `a` includes `b` and `c`, and `b` also includes `c`, then `c` is reached
/// twice and is rejected. Each call tracks reached files independently, so the same file included
/// from two separate top-level config files (e.g. at different hierarchy levels) is fine.
fn load_file(
    path: impl Into<Utf8PathBuf>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigError> {
    let path = path.into();
    let path = path
        .canonicalize_utf8()
        .map_err(|error| CargoConfigError::FailedPathCanonicalization { path, error })?;

    // `seen` is traversal bookkeeping shared across the whole include tree, so it is threaded
    // through by mutable reference. The loaded configs, in contrast, are returned by each call and
    // concatenated by the caller.
    let mut seen = BTreeSet::new();
    load_file_impl(path, &mut seen)
}

/// Loads a single already-canonicalized config file and its includes, returning them in precedence
/// order (highest first).
///
/// `seen` tracks the paths reached so far across the entire include tree, so that a file reached
/// more than once (via a cycle or multiple include paths) is rejected.
fn load_file_impl(
    path: Utf8PathBuf,
    seen: &mut BTreeSet<Utf8PathBuf>,
) -> Result<Vec<(CargoConfigSource, CargoConfig)>, CargoConfigError> {
    if !seen.insert(path.clone()) {
        return Err(CargoConfigError::IncludeReachedTwice { path });
    }

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

    // The directory containing this config file. Include paths are resolved relative to it.
    let config_dir = path
        .parent()
        .expect("a canonicalized config file path has a parent directory")
        .to_owned();
    let includes = config.include.clone();

    // This config's own values take precedence over anything it includes, so it comes first.
    let mut configs = vec![(CargoConfigSource::File(path.clone()), config)];

    // Then each include in reverse listed order: a later include takes precedence over an earlier
    // one, and higher precedence comes first, so the last include (and its subtree) is appended
    // before earlier ones. Each include's own subtree is resolved fully before the next sibling.
    for include in includes.into_iter().rev() {
        if !include.path.ends_with(".toml") {
            return Err(CargoConfigError::IncludePathNotToml {
                path: include.path,
                included_from: path.clone(),
            });
        }

        let include_path = config_dir.join(&include.path);
        let canonicalized = match include_path.canonicalize_utf8() {
            Ok(canonicalized) => canonicalized,
            Err(error) if include.optional && error.kind() == io::ErrorKind::NotFound => {
                // Optional includes silently skip missing files, matching Cargo.
                continue;
            }
            Err(error) => {
                return Err(CargoConfigError::FailedPathCanonicalization {
                    path: include_path,
                    error,
                });
            }
        };

        // Wrap any failure with the include that led to it, building up a chain of `include` keys
        // that mirrors Cargo's nested diagnostics.
        let included = load_file_impl(canonicalized, seen).map_err(|error| {
            CargoConfigError::FailedToLoadInclude {
                path: include.path,
                included_from: path.clone(),
                error: Box::new(error),
            }
        })?;
        configs.extend(included);
    }

    Ok(configs)
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
    /// Additional config files to load, per Cargo's `include` key. See
    /// <https://doc.rust-lang.org/cargo/reference/config.html#config-in-external-files>.
    #[serde(default)]
    pub(crate) include: Vec<CargoConfigInclude>,
    #[serde(default)]
    pub(crate) build: CargoConfigBuild,
    pub(crate) target: Option<BTreeMap<String, CargoConfigRunner>>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, CargoConfigEnv>,
    #[serde(default)]
    pub(crate) term: CargoConfigTerm,
}

/// A single entry in a Cargo config `include` list.
///
/// Each entry is either a bare path string or a table with `path` and optional `optional` keys. See
/// <https://doc.rust-lang.org/cargo/reference/config.html#including-extra-configuration-files>.
#[derive(Clone, Debug)]
pub(crate) struct CargoConfigInclude {
    /// The path to the config file to include, relative to the directory containing the config file
    /// that specifies it.
    pub(crate) path: String,

    /// If `true`, a missing include file is silently skipped rather than being treated as an error.
    pub(crate) optional: bool,
}

impl<'de> Deserialize<'de> for CargoConfigInclude {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Cargo accepts either a bare string or a `{ path, optional }` table for each include entry.
        // A custom visitor is used rather than `#[serde(untagged)]` to produce clearer error
        // messages.
        struct IncludeVisitor;

        impl<'de> Visitor<'de> for IncludeVisitor {
            type Value = CargoConfigInclude;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a config include path (a string, or a table with a `path` key \
                     and an optional `optional` key)",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(CargoConfigInclude {
                    path: value.to_owned(),
                    optional: false,
                })
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                // The table form of an include entry.
                #[derive(Deserialize)]
                struct IncludeTable {
                    path: String,
                    #[serde(default)]
                    optional: bool,
                }
                let table = IncludeTable::deserialize(MapAccessDeserializer::new(map))?;
                Ok(CargoConfigInclude {
                    path: table.path,
                    optional: table.optional,
                })
            }
        }

        deserializer.deserialize_any(IncludeVisitor)
    }
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
    use crate::{cargo_config::test_helpers::write_config, errors::DisplayErrorChain};
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
    fn test_include_deserialize() {
        // Bare string form.
        let config: CargoConfig = toml::from_str(r#"include = ["a.toml", "b.toml"]"#).unwrap();
        assert_eq!(config.include.len(), 2);
        assert_eq!(config.include[0].path, "a.toml");
        assert!(!config.include[0].optional);
        assert_eq!(config.include[1].path, "b.toml");

        // Table form, with and without `optional`.
        let config: CargoConfig = toml::from_str(
            r#"include = [{ path = "a.toml" }, { path = "b.toml", optional = true }]"#,
        )
        .unwrap();
        assert_eq!(config.include[0].path, "a.toml");
        assert!(!config.include[0].optional);
        assert_eq!(config.include[1].path, "b.toml");
        assert!(config.include[1].optional);

        // Mixed string and table forms.
        let config: CargoConfig =
            toml::from_str(r#"include = ["a.toml", { path = "b.toml", optional = true }]"#)
                .unwrap();
        assert_eq!(config.include[0].path, "a.toml");
        assert!(config.include[1].optional);

        // Missing `path` in a table is an error.
        toml::from_str::<CargoConfig>(r#"include = [{ optional = true }]"#)
            .expect_err("table include without `path` should fail");

        // Unknown keys in a table is ignored.
        let config: CargoConfig =
            toml::from_str(r#"include = [{ path = "a.toml", nope = true }]"#).unwrap();
        assert_eq!(config.include[0].path, "a.toml");
    }

    /// Peels [`CargoConfigError::FailedToLoadInclude`] wrappers off an error, returning the
    /// innermost cause. Errors from inside an include are wrapped in one such layer per include hop.
    fn innermost_cause(error: &CargoConfigError) -> &CargoConfigError {
        let mut error = error;
        while let CargoConfigError::FailedToLoadInclude { error: inner, .. } = error {
            error = inner;
        }
        error
    }

    /// Loads the config at `<dir>/.cargo/config.toml` and returns the config sources in precedence
    /// order (highest first), relative to `dir` for readability.
    fn load_include_sources(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let configs = load_file(dir.join(".cargo/config.toml")).expect("configs load successfully");
        configs
            .into_iter()
            .map(|(source, _)| match source {
                CargoConfigSource::File(path) => path
                    .strip_prefix(dir)
                    .expect("path is under the temp dir")
                    .to_owned(),
                CargoConfigSource::CliOption => panic!("no CLI options in this test"),
            })
            .collect()
    }

    #[test]
    fn test_include_precedence_and_recursion() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        // config.toml includes a.toml then b.toml; a.toml recursively includes nested.toml. The
        // resulting precedence order (highest first) should be:
        //   config.toml, b.toml, a.toml, nested.toml.
        // That is: a file's own values beat its includes, and later includes beat earlier ones.
        write_config(
            &dir,
            ".cargo/config.toml",
            r#"include = ["a.toml", "b.toml"]"#,
        );
        write_config(&dir, ".cargo/a.toml", r#"include = ["nested.toml"]"#);
        write_config(&dir, ".cargo/b.toml", "");
        write_config(&dir, ".cargo/nested.toml", "");

        assert_eq!(
            load_include_sources(&dir),
            vec![
                Utf8PathBuf::from(".cargo/config.toml"),
                Utf8PathBuf::from(".cargo/b.toml"),
                Utf8PathBuf::from(".cargo/a.toml"),
                Utf8PathBuf::from(".cargo/nested.toml"),
            ]
        );
    }

    #[test]
    fn test_include_relative_to_config_file() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        // Include paths are resolved relative to the directory of the config file that specifies
        // them, not the workspace root.
        write_config(
            &dir,
            ".cargo/config.toml",
            r#"include = ["../shared/extra.toml"]"#,
        );
        write_config(&dir, "shared/extra.toml", "");

        assert_eq!(
            load_include_sources(&dir),
            vec![
                Utf8PathBuf::from(".cargo/config.toml"),
                Utf8PathBuf::from("shared/extra.toml"),
            ]
        );
    }

    #[test]
    fn test_include_optional_missing() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        // An optional include that is missing is silently skipped; a required one that is missing is
        // an error.
        write_config(
            &dir,
            ".cargo/config.toml",
            r#"include = [{ path = "missing.toml", optional = true }]"#,
        );
        assert_eq!(
            load_include_sources(&dir),
            vec![Utf8PathBuf::from(".cargo/config.toml")]
        );

        write_config(&dir, ".cargo/config.toml", r#"include = ["missing.toml"]"#);
        let err = load_file(dir.join(".cargo/config.toml"))
            .expect_err("required missing include should fail");
        assert!(
            matches!(err, CargoConfigError::FailedPathCanonicalization { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn test_include_non_toml_rejected() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        write_config(&dir, ".cargo/config.toml", r#"include = ["extra.conf"]"#);
        write_config(&dir, ".cargo/extra.conf", "");

        let err =
            load_file(dir.join(".cargo/config.toml")).expect_err("non-.toml include should fail");
        assert!(
            matches!(err, CargoConfigError::IncludePathNotToml { .. }),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn test_include_cycle_detected() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        write_config(&dir, ".cargo/config.toml", r#"include = ["a.toml"]"#);
        write_config(&dir, ".cargo/a.toml", r#"include = ["b.toml"]"#);
        write_config(&dir, ".cargo/b.toml", r#"include = ["a.toml"]"#);

        let err = load_file(dir.join(".cargo/config.toml")).expect_err("include cycle should fail");
        assert!(
            matches!(
                innermost_cause(&err),
                CargoConfigError::IncludeReachedTwice { .. }
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn test_include_reached_twice_rejected() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        // config.toml includes a.toml and b.toml, and a.toml also includes b.toml. b.toml is reached
        // twice, so it is rejected even though there is no cycle. This matches Cargo.
        write_config(
            &dir,
            ".cargo/config.toml",
            r#"include = ["a.toml", "b.toml"]"#,
        );
        write_config(&dir, ".cargo/a.toml", r#"include = ["b.toml"]"#);
        write_config(&dir, ".cargo/b.toml", "");

        let err = load_file(dir.join(".cargo/config.toml"))
            .expect_err("a file reached twice should fail");
        assert!(
            matches!(
                innermost_cause(&err),
                CargoConfigError::IncludeReachedTwice { .. }
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn test_include_error_chain() {
        let temp = camino_tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::try_from(temp.path().canonicalize().unwrap()).unwrap();

        // config.toml -> a.toml -> b.toml -> missing.toml. The failure at the leaf should surface as
        // a chain naming each include hop, mirroring Cargo's nested diagnostics.
        write_config(&dir, ".cargo/config.toml", r#"include = ["a.toml"]"#);
        write_config(&dir, ".cargo/a.toml", r#"include = ["b.toml"]"#);
        write_config(&dir, ".cargo/b.toml", r#"include = ["missing.toml"]"#);

        let err = load_file(dir.join(".cargo/config.toml"))
            .expect_err("missing leaf include should fail");

        // The top-level error names the first hop, and the innermost cause is the missing file.
        assert!(
            matches!(&err, CargoConfigError::FailedToLoadInclude { path, .. } if path == "a.toml"),
            "unexpected top-level error: {err:?}"
        );
        assert!(
            matches!(
                innermost_cause(&err),
                CargoConfigError::FailedPathCanonicalization { .. }
            ),
            "unexpected innermost cause: {err:?}"
        );

        // The rendered chain should mention every include hop.
        let rendered = format!("{}", DisplayErrorChain::new(&err));
        for hop in ["a.toml", "b.toml", "missing.toml"] {
            assert!(
                rendered.contains(hop),
                "error chain should mention `{hop}`, got:\n{rendered}"
            );
        }
    }
}
