//! Adds support for [target runners](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)

use crate::errors::TargetRunnerError;
use camino::Utf8PathBuf;

/// A [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
/// used to execute a test binary rather than the default of executing natively
#[derive(Debug)]
pub struct TargetRunner {
    /// This is required
    runner_binary: Utf8PathBuf,
    /// These are optional
    args: Vec<String>,
}

impl TargetRunner {
    /// Acquires the [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
    /// which can be set in a [.cargo/config.toml](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
    /// or via a `CARGO_TARGET_{TRIPLE}_RUNNER` environment variable
    pub fn for_target(target_triple: Option<&str>) -> Result<Option<Self>, TargetRunnerError> {
        Self::get_runner_by_precedence(target_triple, true, None)
    }

    /// Configures the root directory that starts the search for cargo configs.
    ///
    /// The default is normally the current working directory, but this method
    /// is made available for testing purposes.
    pub fn with_root(
        target_triple: Option<&str>,
        use_cargo_home: bool,
        root: Utf8PathBuf,
    ) -> Result<Option<Self>, TargetRunnerError> {
        Self::get_runner_by_precedence(target_triple, use_cargo_home, Some(root))
    }

    fn get_runner_by_precedence(
        target_triple: Option<&str>,
        use_cargo_home: bool,
        root: Option<Utf8PathBuf>,
    ) -> Result<Option<Self>, TargetRunnerError> {
        let target = match target_triple {
            Some(target) => target_spec::Platform::from_triple(
                target_spec::Triple::new(target.to_owned()).map_err(|error| {
                    TargetRunnerError::FailedToParseTargetTriple {
                        triple: target.to_owned(),
                        error,
                    }
                })?,
                target_spec::TargetFeatures::Unknown,
            ),
            None => {
                target_spec::Platform::current().map_err(TargetRunnerError::UnknownHostPlatform)?
            }
        };

        let triple_str = target.triple_str().to_ascii_uppercase().replace('-', "_");

        // Check for a nextest specific runner, this is highest precedence
        if let Some(tr) = Self::from_env(format!("NEXTEST_{}_RUNNER", triple_str))? {
            return Ok(Some(tr));
        }

        // Next check for a config in the nextest.toml config

        // Next check if have a CARGO_TARGET_{TRIPLE}_RUNNER environment variable
        // set, and if so use that, as it takes precedence over the static config(:?.toml)?
        if let Some(tr) = Self::from_env(format!("CARGO_TARGET_{}_RUNNER", triple_str))? {
            return Ok(Some(tr));
        }

        let root = match root {
            Some(rp) => rp,
            None => {
                // This is a bit non-intuitive, but the .cargo/config.toml hierarchy is actually
                // based on the current working directory, _not_ the manifest path, this bug
                // has existed for a while https://github.com/rust-lang/cargo/issues/2930
                std::env::current_dir()
                    .map_err(TargetRunnerError::UnableToReadDir)
                    .and_then(|cwd| {
                        Utf8PathBuf::from_path_buf(cwd).map_err(TargetRunnerError::NonUtf8Path)
                    })?
            }
        };

        Self::find_config(target, use_cargo_home, root)
    }

    fn from_env(env_key: String) -> Result<Option<Self>, TargetRunnerError> {
        if let Some(runner_var) = std::env::var_os(&env_key) {
            let runner = runner_var
                .into_string()
                .map_err(|_osstr| TargetRunnerError::InvalidEnvironmentVar(env_key.clone()))?;
            Self::parse_runner(&env_key, runner).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempts to find a target runner for the specified target from a
    /// [cargo config](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure)
    pub fn find_config(
        target: target_spec::Platform,
        use_cargo_home: bool,
        root: Utf8PathBuf,
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

        let mut dir = root
            .canonicalize()
            .map_err(|error| TargetRunnerError::FailedPathCanonicalization {
                path: root.clone(),
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
            dir.pop();
        }

        if use_cargo_home {
            // Attempt lookup the $CARGO_HOME directory from the cwd, as that can
            // contain a default config.toml
            let mut cargo_home_path = home::cargo_home_with_cwd(root.as_std_path())
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
            runner: Option<String>,
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

    fn parse_runner(key: &str, value: String) -> Result<Self, TargetRunnerError> {
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

        Ok(Self {
            runner_binary: runner_binary.into(),
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
