// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::ExtractedCustomPlatform;
use crate::{
    cargo_config::{CargoConfigSource, CargoConfigs, DiscoveredConfig},
    errors::TargetTripleError,
};
use camino::{Utf8Path, Utf8PathBuf};
use std::fmt;
use target_spec::{Platform, TargetFeatures, summaries::PlatformSummary};

/// Represents a target triple that's being cross-compiled against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetTriple {
    /// The target platform being built.
    pub platform: Platform,

    /// The source the triple came from.
    pub source: TargetTripleSource,

    /// The place where the target definition was obtained from.
    pub location: TargetDefinitionLocation,
}

impl TargetTriple {
    /// Converts a `PlatformSummary` that was output by `TargetTriple::serialize` back to a target triple.
    /// This target triple is assumed to originate from a build-metadata config.
    pub fn deserialize(
        platform: Option<PlatformSummary>,
    ) -> Result<Option<TargetTriple>, target_spec::Error> {
        platform
            .map(|summary| {
                let platform = summary.to_platform()?;
                let location = if platform.is_custom() {
                    TargetDefinitionLocation::MetadataCustom(
                        summary
                            .custom_json
                            .expect("custom platform <=> custom JSON"),
                    )
                } else {
                    TargetDefinitionLocation::Builtin
                };
                Ok(TargetTriple {
                    platform,
                    source: TargetTripleSource::Metadata,
                    location,
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
                    location: TargetDefinitionLocation::Builtin,
                })
            })
            .transpose()
    }

    /// Returns the target triple being built as a string to pass into downstream Cargo arguments,
    /// such as `cargo metadata --filter-platform`.
    ///
    /// For custom target triples, this will be a path to a file ending with `.json`. Nextest may
    /// temporarily extract the target triple, in which case a `Utf8TempFile` is returned.
    pub fn to_cargo_target_arg(&self) -> Result<CargoTargetArg, TargetTripleError> {
        match &self.location {
            // The determination for heuristic targets may not be quite right.
            TargetDefinitionLocation::Builtin | TargetDefinitionLocation::Heuristic => Ok(
                CargoTargetArg::Builtin(self.platform.triple_str().to_string()),
            ),
            TargetDefinitionLocation::DirectPath(path)
            | TargetDefinitionLocation::RustTargetPath(path) => {
                Ok(CargoTargetArg::Path(path.clone()))
            }
            TargetDefinitionLocation::MetadataCustom(json) => CargoTargetArg::from_custom_json(
                self.platform.triple_str(),
                json,
                self.source.clone(),
            ),
        }
    }

    /// Find the target triple being built.
    ///
    /// This does so by looking at, in order:
    ///
    /// 1. the passed in --target CLI option
    /// 2. the CARGO_BUILD_TARGET env var
    /// 3. build.target in Cargo config files
    ///
    /// The `host_platform` is used to resolve the special "host-tuple" target, which resolves to
    /// the host platform.
    pub fn find(
        cargo_configs: &CargoConfigs,
        target_cli_option: Option<&str>,
        host_platform: &Platform,
    ) -> Result<Option<Self>, TargetTripleError> {
        // First, look at the CLI option passed in.
        if let Some(triple_str_or_path) = target_cli_option {
            let ret = Self::resolve_triple(
                triple_str_or_path,
                TargetTripleSource::CliOption,
                cargo_configs.cwd(),
                cargo_configs.target_paths(),
                host_platform,
            )?;
            return Ok(Some(ret));
        }

        // Finally, look at the cargo configs.
        Self::from_cargo_configs(cargo_configs, host_platform)
    }

    /// The environment variable used for target searches
    pub const CARGO_BUILD_TARGET_ENV: &'static str = "CARGO_BUILD_TARGET";

    fn from_env(
        cwd: &Utf8Path,
        target_paths: &[Utf8PathBuf],
        host_platform: &Platform,
    ) -> Result<Option<Self>, TargetTripleError> {
        if let Some(triple_val) = std::env::var_os(Self::CARGO_BUILD_TARGET_ENV) {
            let triple = triple_val
                .into_string()
                .map_err(|_osstr| TargetTripleError::InvalidEnvironmentVar)?;
            let ret = Self::resolve_triple(
                &triple,
                TargetTripleSource::Env,
                cwd,
                target_paths,
                host_platform,
            )?;
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }

    fn from_cargo_configs(
        cargo_configs: &CargoConfigs,
        host_platform: &Platform,
    ) -> Result<Option<Self>, TargetTripleError> {
        for discovered_config in cargo_configs.discovered_configs() {
            match discovered_config {
                DiscoveredConfig::CliOption { config, source }
                | DiscoveredConfig::File { config, source } => {
                    if let Some(triple) = &config.build.target {
                        let resolve_dir = source.resolve_dir(cargo_configs.cwd());
                        let source = TargetTripleSource::CargoConfig {
                            source: source.clone(),
                        };
                        let ret = Self::resolve_triple(
                            triple,
                            source,
                            resolve_dir,
                            cargo_configs.target_paths(),
                            host_platform,
                        )?;
                        return Ok(Some(ret));
                    }
                }
                DiscoveredConfig::Env => {
                    // Look at the CARGO_BUILD_TARGET env var.
                    if let Some(triple) = Self::from_env(
                        cargo_configs.cwd(),
                        cargo_configs.target_paths(),
                        host_platform,
                    )? {
                        return Ok(Some(triple));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Resolves triples passed in over the command line using the algorithm described here:
    /// https://github.com/rust-lang/rust/blob/2d0aa57684e10f7b3d3fe740ee18d431181583ad/compiler/rustc_target/src/spec/mod.rs#L11C11-L20
    /// https://github.com/rust-lang/rust/blob/f217411bacbe943ead9dfca93a91dff0753c2a96/compiler/rustc_session/src/config.rs#L2065-L2079
    fn resolve_triple(
        triple_str_or_path: &str,
        source: TargetTripleSource,
        // This is typically the cwd but in case of a triple specified in a config file is resolved
        // with respect to that.
        resolve_dir: &Utf8Path,
        target_paths: &[Utf8PathBuf],
        host_platform: &Platform,
    ) -> Result<Self, TargetTripleError> {
        // Handle "host-tuple" special case: resolve to the host platform.
        if triple_str_or_path == "host-tuple" {
            return Ok(Self {
                platform: host_platform.clone(),
                source,
                location: TargetDefinitionLocation::Builtin,
            });
        }

        if triple_str_or_path.ends_with(".json") {
            return Self::custom_from_path(triple_str_or_path.as_ref(), source, resolve_dir);
        }

        // Is this a builtin (non-heuristic)?
        if let Ok(platform) =
            Platform::new_strict(triple_str_or_path.to_owned(), TargetFeatures::Unknown)
        {
            return Ok(Self {
                platform,
                source,
                location: TargetDefinitionLocation::Builtin,
            });
        }

        // Now look for this triple through all the paths in RUST_TARGET_PATH.
        let triple_filename = {
            let mut triple_str = triple_str_or_path.to_owned();
            triple_str.push_str(".json");
            Utf8PathBuf::from(triple_str)
        };

        for dir in target_paths {
            let path = dir.join(&triple_filename);
            if path.is_file() {
                let path = path.canonicalize_utf8().map_err(|error| {
                    TargetTripleError::TargetPathReadError {
                        source: source.clone(),
                        path,
                        error,
                    }
                })?;
                return Self::load_file(
                    triple_str_or_path,
                    &path,
                    source,
                    TargetDefinitionLocation::RustTargetPath(path.clone()),
                );
            }
        }

        // TODO: search in rustlib. This isn't documented and we need to implement searching for
        // rustlib:
        // https://github.com/rust-lang/rust/blob/2d0aa57684e10f7b3d3fe740ee18d431181583ad/compiler/rustc_target/src/spec/mod.rs#L2789-L2799.

        // As a last-ditch effort, use a heuristic approach.
        let platform = Platform::new(triple_str_or_path.to_owned(), TargetFeatures::Unknown)
            .map_err(|error| TargetTripleError::TargetSpecError {
                source: source.clone(),
                error,
            })?;
        Ok(Self {
            platform,
            source,
            location: TargetDefinitionLocation::Heuristic,
        })
    }

    /// Converts a path ending with `.json` to a custom target triple.
    pub(super) fn custom_from_path(
        path: &Utf8Path,
        source: TargetTripleSource,
        resolve_dir: &Utf8Path,
    ) -> Result<Self, TargetTripleError> {
        assert_eq!(
            path.extension(),
            Some("json"),
            "path {path} must end with .json",
        );
        let path = resolve_dir.join(path);
        let canonicalized_path =
            path.canonicalize_utf8()
                .map_err(|error| TargetTripleError::TargetPathReadError {
                    source: source.clone(),
                    path,
                    error,
                })?;
        // Strip the ".json" at the end.
        let triple_str = canonicalized_path
            .file_stem()
            .expect("target path must not be empty")
            .to_owned();
        Self::load_file(
            &triple_str,
            &canonicalized_path,
            source,
            TargetDefinitionLocation::DirectPath(canonicalized_path.clone()),
        )
    }

    fn load_file(
        triple_str: &str,
        path: &Utf8Path,
        source: TargetTripleSource,
        location: TargetDefinitionLocation,
    ) -> Result<Self, TargetTripleError> {
        let contents = std::fs::read_to_string(path).map_err(|error| {
            TargetTripleError::TargetPathReadError {
                source: source.clone(),
                path: path.to_owned(),
                error,
            }
        })?;
        let platform =
            Platform::new_custom(triple_str.to_owned(), &contents, TargetFeatures::Unknown)
                .map_err(|error| TargetTripleError::TargetSpecError {
                    source: source.clone(),
                    error,
                })?;
        Ok(Self {
            platform,
            source,
            location,
        })
    }
}

/// Cargo argument for downstream commands.
///
/// If it is necessary to run a Cargo command with a target triple, this enum provides the right
/// invocation. Create it with [`TargetTriple::to_cargo_target_arg`].
///
/// The `Display` impl of this type produces the argument to provide after `--target`, or `cargo
/// metadata --filter-platform`.
#[derive(Debug)]
pub enum CargoTargetArg {
    /// The target triple is a builtin.
    Builtin(String),

    /// The target triple is a JSON file at this path.
    Path(Utf8PathBuf),

    /// The target triple was extracted from metadata and stored in a temporary directory.
    Extracted(ExtractedCustomPlatform),
}

impl CargoTargetArg {
    fn from_custom_json(
        triple_str: &str,
        json: &str,
        source: TargetTripleSource,
    ) -> Result<Self, TargetTripleError> {
        let extracted = ExtractedCustomPlatform::new(triple_str, json, source)?;
        Ok(Self::Extracted(extracted))
    }
}

impl fmt::Display for CargoTargetArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(triple) => {
                write!(f, "{triple}")
            }
            Self::Path(path) => {
                write!(f, "{path}")
            }
            Self::Extracted(extracted) => {
                write!(f, "{}", extracted.path())
            }
        }
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

    /// The target triple was defined through a `.cargo/config.toml` or `.cargo/config` file, or a
    /// `--config` CLI option.
    CargoConfig {
        /// The source of the configuration.
        source: CargoConfigSource,
    },

    /// The target triple was defined through a metadata file provided using the --archive-file or
    /// the `--binaries-metadata` CLI option.
    Metadata,
}

impl fmt::Display for TargetTripleSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliOption => {
                write!(f, "--target <option>")
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

/// The location a target triple's definition was obtained from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetDefinitionLocation {
    /// The target triple was a builtin.
    Builtin,

    /// The definition was obtained from a file on disk -- the triple string ended with .json.
    DirectPath(Utf8PathBuf),

    /// The definition was obtained from a file in `RUST_TARGET_PATH`.
    RustTargetPath(Utf8PathBuf),

    /// The definition was obtained heuristically.
    Heuristic,

    /// A custom definition was stored in metadata. The string is the JSON of the custom target.
    MetadataCustom(String),
}

impl fmt::Display for TargetDefinitionLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => {
                write!(f, "target was builtin")
            }
            Self::DirectPath(path) => {
                write!(f, "definition obtained from file at path `{path}`")
            }
            Self::RustTargetPath(path) => {
                write!(f, "definition obtained from RUST_TARGET_PATH: `{path}`")
            }
            Self::Heuristic => {
                write!(f, "definition obtained heuristically")
            }
            Self::MetadataCustom(_) => {
                write!(f, "custom definition stored in metadata")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_config::test_helpers::{custom_platform, setup_temp_dir};

    #[test]
    fn test_find_target_triple() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");
        let dir_foo_bar_custom1_path = dir_foo_bar_path.join("custom1");
        let dir_foo_bar_custom2_path = dir_foo_bar_path.join("custom2");
        let custom_target_dir = dir.path().join("custom-target");
        let custom_target_path = dir
            .path()
            .join("custom-target/my-target.json")
            .canonicalize_utf8()
            .expect("path exists");

        // Test reading from config files
        assert_eq!(
            find_target_triple(&[], None, &dir_foo_bar_path, &dir_path),
            Some(TargetTriple {
                platform: platform("x86_64-unknown-linux-gnu"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/bar/.cargo/config.toml")),
                },
                location: TargetDefinitionLocation::Builtin,
            }),
        );

        assert_eq!(
            find_target_triple(&[], None, &dir_foo_path, &dir_path),
            Some(TargetTriple {
                platform: platform("x86_64-pc-windows-msvc"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join("foo/.cargo/config")),
                },
                location: TargetDefinitionLocation::Builtin,
            }),
        );

        assert_eq!(
            find_target_triple(&[], None, &dir_foo_bar_custom2_path, &dir_path),
            Some(TargetTriple {
                platform: custom_platform(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(
                        dir_path.join("foo/bar/custom2/.cargo/config.toml")
                    ),
                },
                location: TargetDefinitionLocation::DirectPath(custom_target_path.clone()),
            })
        );

        assert_eq!(
            find_target_triple_with_paths(
                &[],
                None,
                &dir_foo_bar_custom1_path,
                &dir_path,
                vec![custom_target_dir]
            ),
            Some(TargetTriple {
                platform: custom_platform(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(
                        dir_path.join("foo/bar/custom1/.cargo/config.toml")
                    ),
                },
                location: TargetDefinitionLocation::RustTargetPath(custom_target_path.clone()),
            })
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
            })
        );

        // --config arguments are resolved wrt the current dir.
        assert_eq!(
            find_target_triple(
                &["build.target=\"../../custom-target/my-target.json\"",],
                None,
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                platform: custom_platform(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
                location: TargetDefinitionLocation::DirectPath(custom_target_path.clone()),
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
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
                location: TargetDefinitionLocation::Builtin,
            })
        );
        // Config paths passed over the command line are resolved according to the directory they're
        // in. (To test this, run the test from dir/foo/bar -- extra-custom-config should be
        // resolved according to dir/foo).
        assert_eq!(
            find_target_triple(
                &["../extra-custom-config.toml"],
                None,
                &dir_foo_bar_path,
                &dir_path
            ),
            Some(TargetTriple {
                platform: custom_platform(),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_foo_path.join("extra-custom-config.toml")),
                },
                location: TargetDefinitionLocation::DirectPath(custom_target_path),
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
        find_target_triple_with_paths(
            cli_configs,
            env,
            start_search_at,
            terminate_search_at,
            Vec::new(),
        )
    }

    fn find_target_triple_with_paths(
        cli_configs: &[&str],
        env: Option<&str>,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
        target_paths: Vec<Utf8PathBuf>,
    ) -> Option<TargetTriple> {
        find_target_triple_impl(
            cli_configs,
            None,
            env,
            start_search_at,
            terminate_search_at,
            target_paths,
            &dummy_host_platform(),
        )
    }

    fn find_target_triple_with_host(
        cli_configs: &[&str],
        target_cli_option: Option<&str>,
        env: Option<&str>,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
        host_platform: &Platform,
    ) -> Option<TargetTriple> {
        find_target_triple_impl(
            cli_configs,
            target_cli_option,
            env,
            start_search_at,
            terminate_search_at,
            Vec::new(),
            host_platform,
        )
    }

    fn find_target_triple_impl(
        cli_configs: &[&str],
        target_cli_option: Option<&str>,
        env: Option<&str>,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
        target_paths: Vec<Utf8PathBuf>,
        host_platform: &Platform,
    ) -> Option<TargetTriple> {
        let configs = CargoConfigs::new_with_isolation(
            cli_configs,
            start_search_at,
            terminate_search_at,
            target_paths,
        )
        .unwrap();
        if let Some(env) = env {
            // SAFETY:
            // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
            unsafe { std::env::set_var("CARGO_BUILD_TARGET", env) };
        }
        let ret = TargetTriple::find(&configs, target_cli_option, host_platform).unwrap();
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe { std::env::remove_var("CARGO_BUILD_TARGET") };
        ret
    }

    #[test]
    fn test_host_tuple() {
        // Create a temp dir with a .cargo/config.toml that has build.target = "host-tuple".
        let dir = camino_tempfile::Builder::new()
            .tempdir()
            .expect("error creating tempdir");
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();

        std::fs::create_dir_all(dir.path().join(".cargo")).expect("error creating .cargo subdir");
        std::fs::write(
            dir.path().join(".cargo/config.toml"),
            r#"
                [build]
                target = "host-tuple"
            "#,
        )
        .expect("error writing .cargo/config.toml");

        let host_platform = platform("aarch64-apple-darwin");

        // Test --target host-tuple (CLI option).
        assert_eq!(
            find_target_triple_with_host(
                &[],
                Some("host-tuple"),
                None,
                &dir_path,
                &dir_path,
                &host_platform,
            ),
            Some(TargetTriple {
                platform: platform("aarch64-apple-darwin"),
                source: TargetTripleSource::CliOption,
                location: TargetDefinitionLocation::Builtin,
            })
        );

        // Test --config build.target="host-tuple".
        assert_eq!(
            find_target_triple_with_host(
                &["build.target=\"host-tuple\""],
                None,
                None,
                &dir_path,
                &dir_path,
                &host_platform,
            ),
            Some(TargetTriple {
                platform: platform("aarch64-apple-darwin"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::CliOption,
                },
                location: TargetDefinitionLocation::Builtin,
            })
        );

        // Test CARGO_BUILD_TARGET=host-tuple (env var).
        assert_eq!(
            find_target_triple_with_host(
                &[],
                None,
                Some("host-tuple"),
                &dir_path,
                &dir_path,
                &host_platform,
            ),
            Some(TargetTriple {
                platform: platform("aarch64-apple-darwin"),
                source: TargetTripleSource::Env,
                location: TargetDefinitionLocation::Builtin,
            })
        );

        // Test .cargo/config.toml with build.target = "host-tuple".
        assert_eq!(
            find_target_triple_with_host(&[], None, None, &dir_path, &dir_path, &host_platform),
            Some(TargetTriple {
                platform: platform("aarch64-apple-darwin"),
                source: TargetTripleSource::CargoConfig {
                    source: CargoConfigSource::File(dir_path.join(".cargo/config.toml")),
                },
                location: TargetDefinitionLocation::Builtin,
            })
        );
    }

    fn platform(triple_str: &str) -> Platform {
        Platform::new(triple_str.to_owned(), TargetFeatures::Unknown).expect("triple str is valid")
    }

    fn dummy_host_platform() -> Platform {
        Platform::new(
            "x86_64-unknown-linux-gnu".to_owned(),
            TargetFeatures::Unknown,
        )
        .unwrap()
    }
}
