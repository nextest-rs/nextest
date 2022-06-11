// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for [target runners](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)

use crate::{
    cargo_config::{CargoConfigs, Runner, TargetTriple},
    errors::TargetRunnerError,
};
use camino::Utf8PathBuf;
use nextest_metadata::BuildPlatform;
use std::fmt;
use target_spec::Platform;

/// A [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
/// used to execute a test binary rather than the default of executing natively.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetRunner {
    host: Option<PlatformRunner>,
    target: Option<PlatformRunner>,
}

impl TargetRunner {
    /// Acquires the [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
    /// which can be set in a [.cargo/config.toml](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
    /// or via a `CARGO_TARGET_{TRIPLE}_RUNNER` environment variable
    pub fn new(
        configs: &CargoConfigs,
        target_triple: Option<&TargetTriple>,
    ) -> Result<Self, TargetRunnerError> {
        let host = PlatformRunner::by_precedence(configs, None)?;
        let target = if target_triple.is_some() {
            PlatformRunner::by_precedence(configs, target_triple)?
        } else {
            host.clone()
        };

        Ok(Self { host, target })
    }

    /// Creates an empty target runner that does not delegate to any runner binaries.
    pub fn empty() -> Self {
        Self {
            host: None,
            target: None,
        }
    }

    /// Returns the target [`PlatformRunner`].
    #[inline]
    pub fn target(&self) -> Option<&PlatformRunner> {
        self.target.as_ref()
    }

    /// Returns the host [`PlatformRunner`].
    #[inline]
    pub fn host(&self) -> Option<&PlatformRunner> {
        self.host.as_ref()
    }

    /// Returns the [`PlatformRunner`] for the given build platform (host or target).
    #[inline]
    pub fn for_build_platform(&self, build_platform: BuildPlatform) -> Option<&PlatformRunner> {
        match build_platform {
            BuildPlatform::Target => self.target(),
            BuildPlatform::Host => self.host(),
        }
    }

    /// Returns the platform runners for all build platforms.
    #[inline]
    pub fn all_build_platforms(&self) -> [(BuildPlatform, Option<&PlatformRunner>); 2] {
        [
            (BuildPlatform::Target, self.target()),
            (BuildPlatform::Host, self.host()),
        ]
    }
}

/// A target runner scoped to a specific platform (host or target).
///
/// This forms part of [`TargetRunner`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformRunner {
    runner_binary: Utf8PathBuf,
    args: Vec<String>,
    source: PlatformRunnerSource,
}

impl PlatformRunner {
    fn by_precedence(
        configs: &CargoConfigs,
        triple: Option<&TargetTriple>,
    ) -> Result<Option<Self>, TargetRunnerError> {
        let target = match triple {
            Some(target) => Platform::from_triple(
                target_spec::Triple::new(target.triple.to_owned()).map_err(|error| {
                    TargetRunnerError::FailedToParseTargetTriple {
                        triple: target.triple.to_owned(),
                        error,
                    }
                })?,
                target_spec::TargetFeatures::Unknown,
            ),
            None => Platform::current().map_err(TargetRunnerError::UnknownHostPlatform)?,
        };

        // Check if we have a CARGO_TARGET_{TRIPLE}_RUNNER environment variable
        // set, and if so use that, as it takes precedence over the static config(:?.toml)?
        if let Some(tr) = Self::from_env(Self::runner_env_var(&target))? {
            return Ok(Some(tr));
        }

        Self::find_config(configs, target)
    }

    fn from_env(env_key: String) -> Result<Option<Self>, TargetRunnerError> {
        if let Some(runner_var) = std::env::var_os(&env_key) {
            let runner = runner_var
                .into_string()
                .map_err(|_osstr| TargetRunnerError::InvalidEnvironmentVar(env_key.clone()))?;
            Self::parse_runner(PlatformRunnerSource::Env(env_key), Runner::Simple(runner)).map(Some)
        } else {
            Ok(None)
        }
    }

    // Not part of the public API. Exposed for testing only.
    #[doc(hidden)]
    pub fn runner_env_var(target: &Platform) -> String {
        let triple_str = target.triple_str().to_ascii_uppercase().replace('-', "_");
        format!("CARGO_TARGET_{}_RUNNER", triple_str)
    }

    /// Attempts to find a target runner for the specified target from a
    /// [cargo config](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
    ///
    /// Not part of the public API. For testing only.
    #[doc(hidden)]
    pub fn find_config(
        configs: &CargoConfigs,
        target: target_spec::Platform,
    ) -> Result<Option<Self>, TargetRunnerError> {
        let mut target_runner = None;

        // Now that we've found all of the config files that could declare
        // a runner that matches our target triple, we need to actually find
        // all the matches, but in reverse order as the closer the config is
        // to our current working directory, the higher precedence it has
        'config: for (path, config) in configs.discovered_configs()?.into_iter().rev() {
            if let Some(targets) = &config.target {
                // First lookup by the exact triple, as that one always takes precedence
                if let Some(parent) = targets.get(target.triple_str()) {
                    if let Some(runner) = &parent.runner {
                        target_runner = Some(Self::parse_runner(
                            PlatformRunnerSource::CargoConfig {
                                path: path.to_owned(),
                                target_table: target.triple_str().into(),
                            },
                            runner.clone(),
                        )?);
                        continue;
                    }
                }

                // Next check if there are target.'cfg(..)' expressions that match
                // the target. cargo states that it is not allowed for more than
                // 1 cfg runner to match the target, but we let cargo handle that
                // error itself, we just use the first one that matches
                for (cfg, runner) in targets.iter().filter_map(|(k, v)| match &v.runner {
                    Some(runner) if k.starts_with("cfg(") => Some((k, runner)),
                    _ => None,
                }) {
                    // Treat these as non-fatal, but would be good to log maybe
                    let expr = match target_spec::TargetExpression::new(cfg) {
                        Ok(expr) => expr,
                        Err(_err) => continue,
                    };

                    if expr.eval(&target) == Some(true) {
                        target_runner = Some(Self::parse_runner(
                            PlatformRunnerSource::CargoConfig {
                                path: path.to_owned(),
                                target_table: cfg.clone(),
                            },
                            runner.clone(),
                        )?);
                        continue 'config;
                    }
                }
            }
        }

        Ok(target_runner)
    }

    fn parse_runner(
        source: PlatformRunnerSource,
        runner: Runner,
    ) -> Result<Self, TargetRunnerError> {
        let (runner_binary, args) = match runner {
            Runner::Simple(value) => {
                // We only split on whitespace, which doesn't take quoting into account,
                // but I believe that cargo doesn't do that either
                let mut runner_iter = value.split_whitespace();

                let runner_binary =
                    runner_iter
                        .next()
                        .ok_or_else(|| TargetRunnerError::BinaryNotSpecified {
                            key: source.clone(),
                            value: value.clone(),
                        })?;
                let args = runner_iter.map(String::from).collect();
                (runner_binary.into(), args)
            }
            Runner::List(mut values) => {
                if values.is_empty() {
                    return Err(TargetRunnerError::BinaryNotSpecified {
                        key: source,
                        value: String::new(),
                    });
                } else {
                    let runner_binary = values.remove(0);
                    (runner_binary.into(), values)
                }
            }
        };

        Ok(Self {
            runner_binary,
            args,
            source,
        })
    }

    /// Gets the runner binary path.
    ///
    /// Note that this is returned as a `str` specifically to avoid duct's
    /// behavior of prepending `.` to paths it thinks are relative, the path
    /// specified for a runner can be a full path, but it is most commonly a
    /// binary found in `PATH`
    #[inline]
    pub fn binary(&self) -> &str {
        self.runner_binary.as_str()
    }

    /// Gets the (optional) runner binary arguments
    #[inline]
    pub fn args(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(AsRef::as_ref)
    }

    /// Returns the location where the platform runner is defined.
    #[inline]
    pub fn source(&self) -> &PlatformRunnerSource {
        &self.source
    }
}

/// The place where a platform runner's configuration was picked up from.
///
/// Returned by [`PlatformRunner::source`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformRunnerSource {
    /// The platform runner was defined by this environment variable.
    Env(String),

    /// The platform runner was defined through a `.cargo/config.toml` or `.cargo/config` file.
    CargoConfig {
        /// The path to the configuration file.
        path: Utf8PathBuf,

        /// The table name within `target` that was used.
        ///
        /// # Examples
        ///
        /// If `target.'cfg(target_os = "linux")'.runner` is used, this is `cfg(target_os = "linux")`.
        target_table: String,
    },
}

impl fmt::Display for PlatformRunnerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(var) => {
                write!(f, "environment variable `{var}`")
            }
            Self::CargoConfig { path, target_table } => {
                write!(f, "`target.{target_table}.runner` within `{path}`")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;
    use color_eyre::eyre::{Context, Result};
    use target_spec::TargetFeatures;
    use tempfile::TempDir;

    #[test]
    fn test_find_config() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        // ---
        // Searches through the full directory tree
        // ---
        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                &dir_path,
            ),
            Some(PlatformRunner {
                runner_binary: "wine".into(),
                args: vec!["--test-arg".into()],
                source: PlatformRunnerSource::CargoConfig {
                    path: dir_path.join("foo/bar/.cargo/config.toml"),
                    target_table: "x86_64-pc-windows-msvc".into()
                },
            }),
        );

        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                &dir_path,
            ),
            Some(PlatformRunner {
                runner_binary: "wine2".into(),
                args: vec![],
                source: PlatformRunnerSource::CargoConfig {
                    path: dir_path.join("foo/bar/.cargo/config.toml"),
                    target_table: "cfg(windows)".into()
                },
            }),
        );

        assert_eq!(
            find_config(
                Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                &dir_path,
            ),
            Some(PlatformRunner {
                runner_binary: "unix-runner".into(),
                args: vec![],
                source: PlatformRunnerSource::CargoConfig {
                    path: dir_path.join(".cargo/config"),
                    target_table: "cfg(unix)".into()
                },
            }),
        );

        // ---
        // Searches starting from the "foo" directory which has no .cargo/config in it
        // ---
        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                &dir_foo_path,
                &dir_path,
            ),
            Some(PlatformRunner {
                runner_binary: "parent-wine".into(),
                args: vec![],
                source: PlatformRunnerSource::CargoConfig {
                    path: dir_path.join(".cargo/config"),
                    target_table: "x86_64-pc-windows-msvc".into()
                },
            }),
        );

        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_path,
                &dir_path,
            ),
            None,
        );

        // ---
        // Searches starting and ending at the root directory.
        // ---
        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                &dir_path,
                &dir_path,
            ),
            Some(PlatformRunner {
                runner_binary: "parent-wine".into(),
                args: vec![],
                source: PlatformRunnerSource::CargoConfig {
                    path: dir_path.join(".cargo/config"),
                    target_table: "x86_64-pc-windows-msvc".into()
                },
            }),
        );

        assert_eq!(
            find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_path,
                &dir_path,
            ),
            None,
        );
    }

    fn setup_temp_dir() -> Result<TempDir> {
        let dir = tempfile::Builder::new()
            .tempdir()
            .wrap_err("error creating tempdir")?;

        std::fs::create_dir_all(dir.path().join(".cargo"))
            .wrap_err("error creating .cargo subdir")?;
        std::fs::create_dir_all(dir.path().join("foo/bar/.cargo"))
            .wrap_err("error creating foo/bar/.cargo subdir")?;

        std::fs::write(dir.path().join(".cargo/config"), CARGO_CONFIG_CONTENTS)
            .wrap_err("error writing .cargo/config")?;
        std::fs::write(
            dir.path().join("foo/bar/.cargo/config.toml"),
            FOO_BAR_CARGO_CONFIG_CONTENTS,
        )
        .wrap_err("error writing foo/bar/.cargo/config.toml")?;

        Ok(dir)
    }

    fn find_config(
        platform: Platform,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Option<PlatformRunner> {
        let configs =
            CargoConfigs::new_with_isolation(start_search_at, terminate_search_at).unwrap();
        PlatformRunner::find_config(&configs, platform).unwrap()
    }

    static CARGO_CONFIG_CONTENTS: &str = r#"
    [target.x86_64-pc-windows-msvc]
    runner = "parent-wine"

    [target.'cfg(unix)']
    runner = "unix-runner"
    "#;

    static FOO_BAR_CARGO_CONFIG_CONTENTS: &str = r#"
    [target.x86_64-pc-windows-msvc]
    runner = ["wine", "--test-arg"]

    [target.'cfg(windows)']
    runner = "wine2"
    "#;
}
