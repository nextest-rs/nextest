// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for emulating Cargo's configuration file discovery.
//!
//! Since `cargo config get` is not stable as of Rust 1.61, nextest must do its own config file
//! search.

use crate::errors::CargoConfigSearchError;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use std::collections::BTreeMap;

/// A store for Cargo config files discovered from disk.
///
/// This is required by [`TargetRunner`](crate::target_runner::TargetRunner) and for target triple
/// discovery.
#[derive(Debug)]
pub struct CargoConfigs {
    discovered: Vec<(Utf8PathBuf, CargoConfig)>,
}

impl CargoConfigs {
    /// Discover Cargo config files using the same algorithm that Cargo uses.
    pub fn discover() -> Result<Self, CargoConfigSearchError> {
        let current = std::env::current_dir()
            .map_err(CargoConfigSearchError::GetCurrentDir)
            .and_then(|cwd| {
                Utf8PathBuf::try_from(cwd).map_err(CargoConfigSearchError::NonUtf8Path)
            })?;
        let discovered = discover_impl(&current, None)?;
        Ok(Self { discovered })
    }

    /// Discover Cargo config files with isolation.
    ///
    /// Not part of the public API, for testing only.
    #[doc(hidden)]
    pub fn discover_with_isolation(
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Result<Self, CargoConfigSearchError> {
        let discovered = discover_impl(start_search_at, Some(terminate_search_at))?;
        Ok(Self { discovered })
    }

    pub(crate) fn discovered_configs(
        &self,
    ) -> impl Iterator<Item = (&Utf8Path, &CargoConfig)> + DoubleEndedIterator + '_ {
        self.discovered
            .iter()
            .map(|(path, config)| (path.as_path(), config))
    }
}

fn discover_impl(
    start_search_at: &Utf8Path,
    terminate_search_at: Option<&Utf8Path>,
) -> Result<Vec<(Utf8PathBuf, CargoConfig)>, CargoConfigSearchError> {
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
            let config: CargoConfig = toml::from_str(&config_contents).map_err(|error| {
                CargoConfigSearchError::ConfigParseError {
                    path: path.clone(),
                    error,
                }
            })?;
            Ok((path, config))
        })
        .collect::<Result<Vec<_>, CargoConfigSearchError>>()?;

    Ok(configs)
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfig {
    // pub(crate) build: CargoConfigBuild,
    pub(crate) target: Option<BTreeMap<String, CargoConfigRunner>>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfigBuild {
    // pub(crate) target: Option<String>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CargoConfigRunner {
    #[serde(default)]
    pub(crate) runner: Option<Runner>,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum Runner {
    Simple(String),
    List(Vec<String>),
}
