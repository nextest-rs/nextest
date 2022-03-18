// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for [target runners](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)

use crate::errors::TargetRunnerError;
use camino::{Utf8Path, Utf8PathBuf};
use nextest_metadata::BuildPlatform;
use std::borrow::Cow;
use target_spec::Platform;

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
enum Runner {
    Simple(String),
    List(Vec<String>),
}

/// A [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
/// used to execute a test binary rather than the default of executing natively.
#[derive(Debug, Eq, PartialEq)]
pub struct TargetRunner {
    host: Option<PlatformRunner>,
    target: Option<PlatformRunner>,
}

impl TargetRunner {
    /// Acquires the [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
    /// which can be set in a [.cargo/config.toml](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
    /// or via a `CARGO_TARGET_{TRIPLE}_RUNNER` environment variable
    pub fn new(target_triple: Option<&str>) -> Result<Self, TargetRunnerError> {
        let host = PlatformRunner::by_precedence(None, None, None)?;
        let target = if target_triple.is_some() {
            PlatformRunner::by_precedence(target_triple, None, None)?
        } else {
            host.clone()
        };

        Ok(Self { host, target })
    }

    /// Configures the start and terminate search the search for cargo configs.
    ///
    /// The default is normally the current working directory. Not part of the public API, for
    /// testing only.
    #[doc(hidden)]
    pub fn with_isolation(
        target_triple: Option<&str>,
        start_search_at: &Utf8Path,
        terminate_search_at: &Utf8Path,
    ) -> Result<Self, TargetRunnerError> {
        let host =
            PlatformRunner::by_precedence(None, Some(start_search_at), Some(terminate_search_at))?;
        let target = if target_triple.is_some() {
            PlatformRunner::by_precedence(
                target_triple,
                Some(start_search_at),
                Some(terminate_search_at),
            )?
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
}

impl PlatformRunner {
    fn by_precedence(
        target_triple: Option<&str>,
        root: Option<&Utf8Path>,
        terminate_search_at: Option<&Utf8Path>,
    ) -> Result<Option<Self>, TargetRunnerError> {
        let target = match target_triple {
            Some(target) => Platform::from_triple(
                target_spec::Triple::new(target.to_owned()).map_err(|error| {
                    TargetRunnerError::FailedToParseTargetTriple {
                        triple: target.to_owned(),
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

        let start_search_at = match root {
            Some(rp) => Cow::Borrowed(rp),
            None => {
                // This is a bit non-intuitive, but the .cargo/config.toml hierarchy is actually
                // based on the current working directory, _not_ the manifest path, this bug
                // has existed for a while https://github.com/rust-lang/cargo/issues/2930
                let dir = std::env::current_dir()
                    .map_err(TargetRunnerError::UnableToReadDir)
                    .and_then(|cwd| {
                        Utf8PathBuf::from_path_buf(cwd).map_err(TargetRunnerError::NonUtf8Path)
                    })?;
                Cow::Owned(dir)
            }
        };

        Self::find_config(target, &start_search_at, terminate_search_at)
    }

    fn from_env(env_key: String) -> Result<Option<Self>, TargetRunnerError> {
        if let Some(runner_var) = std::env::var_os(&env_key) {
            let runner = runner_var
                .into_string()
                .map_err(|_osstr| TargetRunnerError::InvalidEnvironmentVar(env_key.clone()))?;
            Self::parse_runner(&env_key, Runner::Simple(runner)).map(Some)
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
        target: target_spec::Platform,
        start_search_at: &Utf8Path,
        terminate_search_at: Option<&Utf8Path>,
    ) -> Result<Option<Self>, TargetRunnerError> {
        let mut configs = Vec::new();

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

        let mut dir = start_search_at
            .canonicalize()
            .map_err(|error| TargetRunnerError::FailedPathCanonicalization {
                path: start_search_at.to_owned(),
                error,
            })
            .and_then(|canon| {
                Utf8PathBuf::from_path_buf(canon).map_err(TargetRunnerError::NonUtf8Path)
            })?;

        for _ in 0..dir.ancestors().count() {
            dir.push(".cargo");

            if !dir.exists() {
                dir.pop();
                dir.pop();
                continue;
            }

            if let Some(config) = read_config_dir(&mut dir) {
                configs.push(config);
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
                .map_err(TargetRunnerError::UnableToReadDir)
                .and_then(|home| {
                    Utf8PathBuf::from_path_buf(home).map_err(TargetRunnerError::NonUtf8Path)
                })?;

            if let Some(home_config) = read_config_dir(&mut cargo_home_path) {
                // Ensure we don't add a duplicate if the current directory is underneath
                // the same root as $CARGO_HOME
                if !configs.iter().any(|path| path == &home_config) {
                    configs.push(home_config);
                }
            }
        }

        #[derive(serde::Deserialize, Debug)]
        struct CargoConfigRunner {
            #[serde(default)]
            runner: Option<Runner>,
        }

        #[derive(serde::Deserialize, Debug)]
        struct CargoConfig {
            target: Option<std::collections::BTreeMap<String, CargoConfigRunner>>,
        }

        let mut target_runner = None;

        let triple_runner_key = format!("target.{}.runner", target.triple_str());

        // Now that we've found all of the config files that could declare
        // a runner that matches our target triple, we need to actually find
        // all the matches, but in reverse order as the closer the config is
        // to our current working directory, the higher precedence it has
        'config: for config_path in configs.into_iter().rev() {
            let config_contents = std::fs::read_to_string(&config_path).map_err(|error| {
                TargetRunnerError::FailedToReadConfig {
                    path: config_path.clone(),
                    error,
                }
            })?;
            let config: CargoConfig = toml::from_str(&config_contents).map_err(|error| {
                TargetRunnerError::FailedToParseConfig {
                    path: config_path.clone(),
                    error,
                }
            })?;

            if let Some(mut targets) = config.target {
                // First lookup by the exact triple, as that one always takes precedence
                if let Some(target) = targets.remove(target.triple_str()) {
                    if let Some(runner) = target.runner {
                        target_runner = Some(Self::parse_runner(&triple_runner_key, runner)?);
                        continue;
                    }
                }

                // Next check if there are target.'cfg(..)' expressions that match
                // the target. cargo states that it is not allowed for more than
                // 1 cfg runner to match the target, but we let cargo handle that
                // error itself, we just use the first one that matches
                for (cfg, runner) in targets.into_iter().filter_map(|(k, v)| match v.runner {
                    Some(runner) if k.starts_with("cfg(") => Some((k, runner)),
                    _ => None,
                }) {
                    // Treat these as non-fatal, but would be good to log maybe
                    let expr = match target_spec::TargetExpression::new(&cfg) {
                        Ok(expr) => expr,
                        Err(_err) => continue,
                    };

                    if expr.eval(&target) == Some(true) {
                        target_runner = Some(Self::parse_runner(&triple_runner_key, runner)?);
                        continue 'config;
                    }
                }
            }
        }

        Ok(target_runner)
    }

    fn parse_runner(key: &str, runner: Runner) -> Result<Self, TargetRunnerError> {
        let (runner_binary, args) = match runner {
            Runner::Simple(value) => {
                // We only split on whitespace, which doesn't take quoting into account,
                // but I believe that cargo doesn't do that either
                let mut runner_iter = value.split_whitespace();

                let runner_binary =
                    runner_iter
                        .next()
                        .ok_or_else(|| TargetRunnerError::BinaryNotSpecified {
                            key: key.to_owned(),
                            value: value.clone(),
                        })?;
                let args = runner_iter.map(String::from).collect();
                (runner_binary.into(), args)
            }
            Runner::List(mut values) => {
                if values.is_empty() {
                    return Err(TargetRunnerError::BinaryNotSpecified {
                        key: key.to_owned(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use color_eyre::eyre::{Context, Result};
    use target_spec::TargetFeatures;
    use tempfile::TempDir;

    #[test]
    fn test_find_config() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = <&Utf8Path>::try_from(dir.path()).unwrap();
        let dir_foo_path = Utf8PathBuf::try_from(dir.path().join("foo")).unwrap();
        let dir_foo_bar_path = Utf8PathBuf::try_from(dir.path().join("foo/bar")).unwrap();

        // ---
        // Searches through the full directory tree
        // ---
        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                Some(dir_path),
            )
            .unwrap(),
            Some(PlatformRunner {
                runner_binary: "wine".into(),
                args: vec!["--test-arg".into()],
            }),
        );

        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                Some(dir_path),
            )
            .unwrap(),
            Some(PlatformRunner {
                runner_binary: "wine2".into(),
                args: vec![],
            }),
        );

        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_bar_path,
                Some(dir_path),
            )
            .unwrap(),
            Some(PlatformRunner {
                runner_binary: "unix-runner".into(),
                args: vec![],
            }),
        );

        // ---
        // Searches starting from the "foo" directory which has no .cargo/config in it
        // ---
        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                &dir_foo_path,
                Some(dir_path),
            )
            .unwrap(),
            Some(PlatformRunner {
                runner_binary: "parent-wine".into(),
                args: vec![],
            }),
        );

        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                &dir_foo_path,
                Some(dir_path),
            )
            .unwrap(),
            None,
        );

        // ---
        // Searches starting and ending at the root directory.
        // ---
        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-msvc", TargetFeatures::Unknown).unwrap(),
                dir_path,
                Some(dir_path),
            )
            .unwrap(),
            Some(PlatformRunner {
                runner_binary: "parent-wine".into(),
                args: vec![],
            }),
        );

        assert_eq!(
            PlatformRunner::find_config(
                Platform::new("x86_64-pc-windows-gnu", TargetFeatures::Unknown).unwrap(),
                dir_path,
                Some(dir_path),
            )
            .unwrap(),
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
