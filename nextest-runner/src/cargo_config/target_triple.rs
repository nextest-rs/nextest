// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    cargo_config::{CargoConfigSource, CargoConfigs, DiscoveredConfig},
    errors::TargetTripleError,
};
use std::fmt;
use target_spec::{summaries::PlatformSummary, Platform, TargetFeatures};

/// Represents a target triple that's being cross-compiled against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetTriple {
    /// The target platform being built.
    pub platform: Platform,

    /// The source the triple came from.
    pub source: TargetTripleSource,
}

impl TargetTriple {
    /// Converts a target triple to a `String` that can be stored in the build-metadata.
    /// cargo-nextest represents the host triple with `None` during runtime.
    /// However the build-metadata might be used on a system with a different host triple.
    /// Therefore the host triple is detected if `target_triple` is `None`
    pub fn serialize(target_triple: Option<&TargetTriple>) -> Option<PlatformSummary> {
        if let Some(target) = &target_triple {
            Some(target.platform.to_summary())
        } else {
            match Platform::current() {
                Ok(host) => Some(host.to_summary()),
                Err(err) => {
                    log::warn!(
                        "failed to detect host target: {err}!\n cargo nextest may use the wrong test runner for this archive."
                    );
                    None
                }
            }
        }
    }

    /// Converts a `PlatformSummary` that was output by `TargetTriple::serialize` back to a target triple.
    /// This target triple is assumed to originate from a build-metadata config.
    pub fn deserialize(
        platform: Option<PlatformSummary>,
    ) -> Result<Option<TargetTriple>, target_spec::Error> {
        platform
            .map(|platform| {
                Ok(TargetTriple {
                    platform: platform.to_platform()?,
                    source: TargetTripleSource::Metadata,
                })
            })
            .transpose()
    }

    /// Converts a string that was output by older versions of nextest back to a target triple.
    pub fn deserialize_str(
        triple_str: Option<String>,
    ) -> Result<Option<TargetTriple>, target_spec::Error> {
        triple_str
            .map(|triple_str| {
                Ok(TargetTriple {
                    platform: Platform::new(triple_str, TargetFeatures::Unknown)?,
                    source: TargetTripleSource::Metadata,
                })
            })
            .transpose()
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
            let platform =
                Platform::new(triple.to_owned(), TargetFeatures::Unknown).map_err(|error| {
                    TargetTripleError::TargetSpecError {
                        source: TargetTripleSource::CliOption,
                        error,
                    }
                })?;
            return Ok(Some(TargetTriple {
                // TODO: need to get the minimum set of target features from here
                platform,
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
            let platform = Platform::new(triple, TargetFeatures::Unknown).map_err(|error| {
                TargetTripleError::TargetSpecError {
                    source: TargetTripleSource::Env,
                    error,
                }
            })?;
            Ok(Some(Self {
                platform,
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
                    let source = TargetTripleSource::CargoConfig {
                        source: source.clone(),
                    };
                    if let Some(triple) = &config.build.target {
                        match Platform::new(triple.clone(), TargetFeatures::Unknown) {
                            Ok(platform) => return Ok(Some(TargetTriple { platform, source })),
                            Err(error) => {
                                return Err(TargetTripleError::TargetSpecError { source, error })
                            }
                        }
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

    /// The platform runner was defined through a metadata file provided using the --archive-file or
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
                platform: platform("x86_64-unknown-linux-gnu"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/bar/.cargo/config.toml")),
                },
            }),
        );

        assert_eq!(
            find_target_triple(&[], None, &dir_foo_path, &dir_path),
            Some(TargetTriple {
                platform: platform("x86_64-pc-windows-msvc"),
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
                platform: platform("aarch64-unknown-linux-gnu"),
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
                platform: platform("aarch64-unknown-linux-gnu"),
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
                platform: platform("aarch64-unknown-linux-gnu"),
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
                platform: platform("aarch64-pc-windows-msvc"),
                source: TargetTripleSource::Env,
            })
        );

        // --config <path> should be parsed correctly. Config files passed in via --config currently
        // come after keys and values passed in via --config, and before the environment (this
        // didn't used to be the case in older versions of Rust, but is now the case as of Rust 1.68
        // with https://github.com/rust-lang/cargo/pull/11077).
        assert_eq!(
            find_target_triple(&["extra-config.toml"], None, &dir_foo_path, &dir_path),
            Some(TargetTriple {
                platform: platform("aarch64-unknown-linux-gnu"),
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
                platform: platform("aarch64-unknown-linux-gnu"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_foo_path.join("extra-config.toml")),
                },
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
                platform: platform("x86_64-unknown-linux-musl"),
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
                platform: platform("x86_64-unknown-linux-musl"),
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

    fn platform(triple_str: &str) -> Platform {
        Platform::new(triple_str.to_owned(), TargetFeatures::Unknown).expect("triple str is valid")
    }
}
