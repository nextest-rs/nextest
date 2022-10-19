// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{CargoConfigSource, CargoConfigs, DiscoveredConfig},
    errors::TargetTripleError,
};
use std::fmt;
use target_spec::Platform;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_config::{test_helpers::setup_temp_dir, CargoConfigs};
    use camino::{Utf8Path, Utf8PathBuf};

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
}
