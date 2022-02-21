//! Adds support for [target runners](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)

use crate::errors::TargetRunnerError;
use camino::Utf8PathBuf;

/// A [target runner](https://doc.rust-lang.org/cargo/reference/config.html#targettriplerunner)
/// used to execute a test binary rather than the default of executing natively
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
        use std::borrow::Cow;

        let target_triple: Cow<'_, str> = match target_triple {
            Some(target) => Cow::Borrowed(target),
            None => Cow::Owned(cfg_expr::target_lexicon::HOST.to_string()),
        };

        // First check if have a CARGO_TARGET_{TRIPLE}_RUNNER environment variable
        // set, and if so use that, as it takes precedence over the static config.toml
        {
            let env_key = format!(
                "CARGO_TARGET_{}_RUNNER",
                target_triple.to_ascii_uppercase().replace('-', "_")
            );

            if let Some(runner_var) = std::env::var_os(&env_key) {
                let runner = runner_var
                    .into_string()
                    .map_err(|_osstr| TargetRunnerError::InvalidEnvironmentVar(env_key.clone()))?;
                return Self::parse_runner(&env_key, runner).map(Some);
            }
        }

        Self::find_config(&target_triple)
    }

    fn find_config(target_triple: &str) -> Result<Option<Self>, TargetRunnerError> {
        // This is a bit non-intuitive, but the .cargo/config.toml hierarchy is actually
        // based on the current working directory, _not_ the manifest path, this bug
        // has existed for a while https://github.com/rust-lang/cargo/issues/2930
        let root = std::env::current_dir()
            .map_err(TargetRunnerError::UnableToReadDir)
            .and_then(|cwd| {
                Utf8PathBuf::from_path_buf(cwd).map_err(TargetRunnerError::NonUtf8Path)
            })?;

        // Attempt lookup the $CARGO_HOME directory from the cwd, as that can
        // contain a default config.toml
        let mut cargo_home_path = home::cargo_home_with_cwd(root.as_std_path())
            .map_err(TargetRunnerError::UnableToReadDir)
            .and_then(|home| {
                Utf8PathBuf::from_path_buf(home).map_err(TargetRunnerError::NonUtf8Path)
            })?;

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
            .map_err(|error| TargetRunnerError::FailedPathCanonicalization { path: root, error })
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

        if let Some(home_config) = read_config_dir(&mut cargo_home_path) {
            // Ensure we don't add a duplicate if the current directory is underneath
            // the same root as $CARGO_HOME
            if !configs.iter().any(|path| path == &home_config) {
                configs.push(home_config);
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

        let triple_runner_key = format!("target.{}.runner", target_triple);
        // Attempt to get the target info from a triple, this can fail if the
        // target is actually a .json target spec, or is otherwise of a form
        // that target_lexicon is unable to parse, which can happen with newer
        // niche targets
        let target_info: Option<cfg_expr::target_lexicon::Triple> = target_triple.parse().ok();

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
                if let Some(target) = targets.remove(target_triple) {
                    if let Some(runner) = target.runner {
                        target_runner = Some(Self::parse_runner(&triple_runner_key, runner)?);
                        continue;
                    }
                }

                if let Some(target_info) = &target_info {
                    // Next check if there are target.'cfg(..)' expressions that match
                    // the target. cargo states that it is not allowed for more than
                    // 1 cfg runner to match the target, but we let cargo handle that
                    // error itself, we just use the first one that matches
                    for (cfg, runner) in targets.into_iter().filter_map(|(k, v)| match v.runner {
                        Some(runner) if k.starts_with("cfg(") => Some((k, runner)),
                        _ => None,
                    }) {
                        // Treat these as non-fatal, but would be good to log maybe
                        let expr = match cfg_expr::Expression::parse(&cfg) {
                            Ok(expr) => expr,
                            Err(_err) => continue,
                        };

                        if expr.eval(|pred| match pred {
                            cfg_expr::Predicate::Target(tp) => tp.matches(target_info),
                            _ => false,
                        }) {
                            target_runner = Some(Self::parse_runner(&triple_runner_key, runner)?);
                            continue 'config;
                        }
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
        &self.runner_binary.as_str()
    }

    /// Gets the (optional) runner binary arguments
    #[inline]
    pub fn args(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(AsRef::as_ref)
    }
}
