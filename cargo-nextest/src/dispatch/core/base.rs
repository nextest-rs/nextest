// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Base application infrastructure shared by core commands.

use crate::{
    ExpectedError, Result, ReuseBuildKind,
    cargo_cli::CargoOptions,
    dispatch::{
        EarlyArgs,
        common::ConfigOpts,
        helpers::{acquire_graph_data, detect_build_platforms, runner_for_target},
    },
    output::{OutputContext, OutputWriter},
};
use camino::Utf8PathBuf;
use guppy::graph::PackageGraph;
use nextest_filtering::ParseContext;
use nextest_runner::{
    cargo_config::CargoConfigs,
    config::core::{
        ConfigExperimental, EarlyProfile, ExperimentalConfig, NextestConfig, NextestVersionConfig,
        NextestVersionEval, VersionOnlyConfig,
    },
    double_spawn::DoubleSpawnInfo,
    list::BinaryList,
    platform::BuildPlatforms,
    reuse_build::ReuseBuildInfo,
    target_runner::TargetRunner,
};
use owo_colors::OwoColorize;
use semver::Version;
use std::{
    collections::BTreeSet,
    env::VarError,
    sync::{Arc, OnceLock},
};
use tracing::{Level, info, warn};

/// Base application state shared by core commands (run, list, bench, archive).
pub(crate) struct BaseApp {
    pub(crate) output: OutputContext,
    pub(crate) early_args: EarlyArgs,
    // TODO: support multiple --target options.
    pub(crate) build_platforms: BuildPlatforms,
    pub(crate) cargo_metadata_json: Arc<String>,
    package_graph: Arc<PackageGraph>,
    // Potentially remapped workspace root (might not be the same as the graph).
    pub(crate) workspace_root: Utf8PathBuf,
    manifest_path: Option<Utf8PathBuf>,
    pub(crate) reuse_build: ReuseBuildInfo,
    pub(crate) cargo_opts: CargoOptions,
    pub(crate) config_opts: ConfigOpts,
    pub(crate) current_version: Version,

    pub(crate) cargo_configs: CargoConfigs,
    double_spawn: OnceLock<DoubleSpawnInfo>,
    target_runner: OnceLock<TargetRunner>,
}

impl BaseApp {
    pub(crate) fn new(
        output: OutputContext,
        early_args: EarlyArgs,
        reuse_build: crate::reuse_build::ReuseBuildOpts,
        cargo_opts: crate::cargo_cli::CargoOptions,
        config_opts: ConfigOpts,
        manifest_path: Option<Utf8PathBuf>,
        writer: &mut OutputWriter,
    ) -> Result<Self> {
        reuse_build.check_experimental(output);

        let reuse_build = reuse_build.process(output, writer)?;

        let cargo_configs = CargoConfigs::new(&cargo_opts.config).map_err(Box::new)?;

        let build_platforms = match reuse_build.binaries_metadata() {
            Some(kind) => kind.binary_list.rust_build_meta.build_platforms.clone(),
            None => detect_build_platforms(&cargo_configs, cargo_opts.target.as_deref())?,
        };

        let (cargo_metadata_json, package_graph) = match reuse_build.cargo_metadata() {
            Some(m) => (m.json.clone(), m.graph.clone()),
            None => {
                let json = acquire_graph_data(
                    manifest_path.as_deref(),
                    cargo_opts.target_dir.as_deref(),
                    &cargo_opts,
                    &build_platforms,
                    output,
                )?;
                let graph = PackageGraph::from_json(&json)
                    .map_err(|err| ExpectedError::cargo_metadata_parse_error(None, err))?;
                (Arc::new(json), Arc::new(graph))
            }
        };

        let manifest_path = if reuse_build.cargo_metadata.is_some() {
            Some(package_graph.workspace().root().join("Cargo.toml"))
        } else {
            manifest_path
        };

        let workspace_root = match reuse_build.workspace_remap() {
            Some(path) => path.to_owned(),
            _ => package_graph.workspace().root().to_owned(),
        };

        let root_manifest_path = workspace_root.join("Cargo.toml");
        if !root_manifest_path.exists() {
            // This doesn't happen in normal use, but is a common situation if the build is being
            // reused.
            let reuse_build_kind = if reuse_build.workspace_remap().is_some() {
                ReuseBuildKind::ReuseWithWorkspaceRemap { workspace_root }
            } else if reuse_build.is_active() {
                ReuseBuildKind::Reuse
            } else {
                ReuseBuildKind::Normal
            };

            return Err(ExpectedError::RootManifestNotFound {
                path: root_manifest_path,
                reuse_build_kind,
            });
        }

        let current_version = current_version();

        Ok(Self {
            output,
            early_args,
            build_platforms,
            cargo_metadata_json,
            package_graph,
            workspace_root,
            reuse_build,
            manifest_path,
            cargo_opts,
            config_opts,
            cargo_configs,
            current_version,

            double_spawn: OnceLock::new(),
            target_runner: OnceLock::new(),
        })
    }

    pub(crate) fn load_config(
        &self,
        pcx: &ParseContext<'_>,
        required_experimental: &BTreeSet<ConfigExperimental>,
    ) -> Result<(VersionOnlyConfig, NextestConfig)> {
        // Load the version-only config first to avoid incompatibilities with parsing the rest of
        // the config.
        let version_only_config = self
            .config_opts
            .make_version_only_config(&self.workspace_root)?;
        self.check_version_config_initial(version_only_config.nextest_version())?;

        // Check for unknown experimental features after the version check. This ensures that if
        // the required nextest version is higher than the current version, the version error takes
        // precedence (a future version may have new experimental features).
        self.check_experimental_config_initial(version_only_config.experimental())?;

        let mut experimental = ConfigExperimental::from_env();
        experimental.extend(version_only_config.experimental().known());

        // Check that all required experimental features are enabled.
        let missing = required_experimental
            .difference(&experimental)
            .copied()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            let config_file = self
                .config_opts
                .config_file
                .clone()
                .unwrap_or_else(|| Utf8PathBuf::from(".config/nextest.toml"));
            return Err(ExpectedError::ConfigExperimentalFeaturesNotEnabled {
                config_file,
                missing,
            });
        }

        if !experimental.is_empty() {
            info!(
                "experimental features enabled: {}",
                experimental
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let config = self.config_opts.make_config(
            &self.workspace_root,
            pcx,
            version_only_config.experimental().known(),
        )?;

        Ok((version_only_config, config))
    }

    fn check_version_config_initial(&self, version_cfg: &NextestVersionConfig) -> Result<()> {
        let styles = self.output.stderr_styles();

        match version_cfg.eval(
            &self.current_version,
            self.config_opts.override_version_check,
        ) {
            NextestVersionEval::Satisfied => Ok(()),
            NextestVersionEval::Error {
                required,
                current,
                tool,
            } => Err(ExpectedError::RequiredVersionNotMet {
                required,
                current,
                tool,
            }),
            NextestVersionEval::Warn {
                recommended: required,
                current,
                tool,
            } => {
                warn!(
                    "this repository recommends nextest version {}, but the current version is {}",
                    required.style(styles.bold),
                    current.style(styles.bold),
                );
                if let Some(tool) = tool {
                    info!(
                        target: "cargo_nextest::no_heading",
                        "(recommended version specified by tool `{}`)",
                        tool,
                    );
                }

                Ok(())
            }
            NextestVersionEval::ErrorOverride {
                required,
                current,
                tool,
            } => {
                info!(
                    "overriding version check (required: {}, current: {})",
                    required, current
                );
                if let Some(tool) = tool {
                    info!(
                        target: "cargo_nextest::no_heading",
                        "(required version specified by tool `{}`)",
                        tool,
                    );
                }

                Ok(())
            }
            NextestVersionEval::WarnOverride {
                recommended,
                current,
                tool,
            } => {
                info!(
                    "overriding version check (recommended: {}, current: {})",
                    recommended, current,
                );
                if let Some(tool) = tool {
                    info!(
                        target: "cargo_nextest::no_heading",
                        "(recommended version specified by tool `{}`)",
                        tool,
                    );
                }

                Ok(())
            }
        }
    }

    fn check_experimental_config_initial(
        &self,
        experimental_cfg: &ExperimentalConfig,
    ) -> Result<()> {
        let config_file = self
            .config_opts
            .config_file
            .clone()
            .unwrap_or_else(|| self.workspace_root.join(NextestConfig::CONFIG_PATH));
        if let Some(err) = experimental_cfg.eval().into_error(config_file) {
            Err(err.into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_version_config_final(
        &self,
        version_cfg: &NextestVersionConfig,
    ) -> Result<()> {
        let styles = self.output.stderr_styles();

        match version_cfg.eval(
            &self.current_version,
            self.config_opts.override_version_check,
        ) {
            NextestVersionEval::Satisfied => Ok(()),
            NextestVersionEval::Error {
                required,
                current,
                tool,
            } => Err(ExpectedError::RequiredVersionNotMet {
                required,
                current,
                tool,
            }),
            NextestVersionEval::Warn {
                recommended: required,
                current,
                tool,
            } => {
                warn!(
                    "this repository recommends nextest version {}, but the current version is {}",
                    required.style(styles.bold),
                    current.style(styles.bold),
                );
                if let Some(tool) = tool {
                    info!(
                        target: "cargo_nextest::no_heading",
                        "(recommended version specified by tool `{}`)",
                        tool,
                    );
                }

                // Don't need to print extra text here -- this is a warning, not an error.
                crate::helpers::log_needs_update(
                    Level::INFO,
                    crate::helpers::BYPASS_VERSION_TEXT,
                    &styles,
                );

                Ok(())
            }
            NextestVersionEval::ErrorOverride { .. } | NextestVersionEval::WarnOverride { .. } => {
                // Don't print overrides at the end since users have already opted into overrides --
                // just be ok with the one at the beginning.
                Ok(())
            }
        }
    }

    pub(crate) fn load_double_spawn(&self) -> &DoubleSpawnInfo {
        self.double_spawn.get_or_init(|| {
            if std::env::var("NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN").is_ok() {
                warn!(
                    "double-spawn is no longer experimental: \
                     NEXTEST_EXPERIMENTAL_DOUBLE_SPAWN does not need to be set"
                );
            }
            if std::env::var("NEXTEST_DOUBLE_SPAWN") == Ok("0".to_owned()) {
                info!("NEXTEST_DOUBLE_SPAWN=0 set, disabling double-spawn for test processes");
                DoubleSpawnInfo::disabled()
            } else {
                DoubleSpawnInfo::try_enable()
            }
        })
    }

    pub(crate) fn load_runner(&self, build_platforms: &BuildPlatforms) -> &TargetRunner {
        self.target_runner.get_or_init(|| {
            runner_for_target(
                &self.cargo_configs,
                build_platforms,
                &self.output.stderr_styles(),
            )
        })
    }

    pub(crate) fn build_scope_args(&self) -> Vec<String> {
        self.cargo_opts
            .build_scope
            .to_cli_args()
            .into_iter()
            .map(|s| s.to_owned())
            .collect()
    }

    pub(crate) fn build_binary_list(&self, cargo_command: &str) -> Result<Arc<BinaryList>> {
        let binary_list = match self.reuse_build.binaries_metadata() {
            Some(m) => m.binary_list.clone(),
            None => Arc::new(self.cargo_opts.compute_binary_list(
                cargo_command,
                self.graph(),
                self.manifest_path.as_deref(),
                self.output,
                self.build_platforms.clone(),
            )?),
        };
        Ok(binary_list)
    }

    /// Builds the binary list, potentially inheriting build scope from a rerun.
    ///
    /// If `rerun_build_scope` is provided and the CLI has no build scope args,
    /// inherits the build scope from the original run in the rerun chain.
    pub(crate) fn build_binary_list_with_rerun(
        &self,
        cargo_command: &str,
        rerun_build_scope: Option<&[String]>,
    ) -> Result<Arc<BinaryList>> {
        // If reusing a build, just return that.
        if let Some(m) = self.reuse_build.binaries_metadata() {
            return Ok(m.binary_list.clone());
        }

        let cli_has_scope = self.cargo_opts.build_scope.has_any();

        let inherited_scope = match (rerun_build_scope, cli_has_scope) {
            (Some(scope), false) => {
                // Inherit from the original run.
                if scope.is_empty() {
                    info!("rerun: inheriting build scope from original run: (default scope)");
                } else {
                    info!(
                        "rerun: inheriting build scope from original run: {}",
                        scope.join(" ")
                    );
                }
                Some(scope)
            }
            (Some(_), true) => {
                // User provided build scope args, not inheriting.
                info!("rerun: using provided build scope, not inheriting from original run");
                None
            }
            (None, _) => None,
        };

        let binary_list = if let Some(scope) = inherited_scope {
            self.cargo_opts.compute_binary_list_with_inherited(
                cargo_command,
                scope,
                self.graph(),
                self.manifest_path.as_deref(),
                self.output,
                self.build_platforms.clone(),
            )?
        } else {
            self.cargo_opts.compute_binary_list(
                cargo_command,
                self.graph(),
                self.manifest_path.as_deref(),
                self.output,
                self.build_platforms.clone(),
            )?
        };

        Ok(Arc::new(binary_list))
    }

    #[inline]
    pub(crate) fn graph(&self) -> &PackageGraph {
        &self.package_graph
    }

    pub(crate) fn load_profile<'cfg>(
        &self,
        config: &'cfg NextestConfig,
    ) -> Result<EarlyProfile<'cfg>> {
        let profile_name = self.config_opts.profile.as_deref().unwrap_or_else(|| {
            // The "official" way to detect a miri environment is with MIRI_SYSROOT.
            // https://github.com/rust-lang/miri/pull/2398#issuecomment-1190747685
            if std::env::var_os("MIRI_SYSROOT").is_some() {
                NextestConfig::DEFAULT_MIRI_PROFILE
            } else {
                NextestConfig::DEFAULT_PROFILE
            }
        });
        let profile = config
            .profile(profile_name)
            .map_err(ExpectedError::profile_not_found)?;
        let store_dir = profile.store_dir();
        std::fs::create_dir_all(store_dir).map_err(|err| ExpectedError::StoreDirCreateError {
            store_dir: store_dir.to_owned(),
            err,
        })?;
        Ok(profile)
    }
}

/// Returns the current nextest version.
pub(crate) fn current_version() -> Version {
    // This is a test-only, not part of the public API.
    match std::env::var("__NEXTEST_TEST_VERSION") {
        Ok(version) => version
            .parse()
            .expect("__NEXTEST_TEST_VERSION should be a valid semver version"),
        Err(VarError::NotPresent) => env!("CARGO_PKG_VERSION")
            .parse()
            .expect("CARGO_PKG_VERSION should be a valid semver version"),
        Err(error) => {
            panic!("error reading __NEXTEST_TEST_VERSION: {error}");
        }
    }
}

// Helper for CargoOptions.
mod helpers {
    use crate::{
        ExpectedError, Result,
        cargo_cli::{CargoCli, CargoOptions},
        dispatch::core::value_enums::CargoMessageFormatOpt,
        output::OutputContext,
    };
    use buf_list::{BufList, Cursor};
    use bytes::Bytes;
    use camino::Utf8Path;
    use guppy::graph::PackageGraph;
    use nextest_runner::{list::BinaryList, platform::BuildPlatforms};
    use std::io::{BufRead, BufReader};

    impl CargoOptions {
        pub(crate) fn compute_binary_list(
            &self,
            cargo_command: &str,
            graph: &PackageGraph,
            manifest_path: Option<&Utf8Path>,
            output: OutputContext,
            build_platforms: BuildPlatforms,
        ) -> Result<BinaryList> {
            // Don't use the manifest path from the graph to ensure that if the user cd's into a
            // particular crate and runs cargo nextest, then it behaves identically to cargo test.
            let mut cargo_cli = CargoCli::new(cargo_command, manifest_path, output);

            let message_format = CargoMessageFormatOpt::combine(&self.cargo_message_format)?;

            // Only build tests in the cargo test invocation, do not run them.
            cargo_cli.add_args(["--no-run", "--message-format", message_format.cargo_arg()]);
            cargo_cli.add_options(self);

            Self::run_cargo_build(
                cargo_cli,
                graph,
                build_platforms,
                message_format.forward_json(),
            )
        }

        /// Computes the binary list using inherited build scope from a rerun.
        ///
        /// This uses the inherited build scope args instead of the current CLI's
        /// build scope, but uses all other options from the current CLI.
        pub(crate) fn compute_binary_list_with_inherited(
            &self,
            cargo_command: &str,
            inherited_build_scope: &[String],
            graph: &PackageGraph,
            manifest_path: Option<&Utf8Path>,
            output: OutputContext,
            build_platforms: BuildPlatforms,
        ) -> Result<BinaryList> {
            let mut cargo_cli = CargoCli::new(cargo_command, manifest_path, output);

            let message_format = CargoMessageFormatOpt::combine(&self.cargo_message_format)?;

            cargo_cli.add_args(["--no-run", "--message-format", message_format.cargo_arg()]);

            // Add inherited build scope instead of self.build_scope.
            for arg in inherited_build_scope {
                cargo_cli.add_owned_arg(arg.clone());
            }
            // Add non-build-scope options from the current CLI.
            cargo_cli.add_non_build_scope_options(self);

            Self::run_cargo_build(
                cargo_cli,
                graph,
                build_platforms,
                message_format.forward_json(),
            )
        }

        fn run_cargo_build(
            cargo_cli: CargoCli<'_>,
            graph: &PackageGraph,
            build_platforms: BuildPlatforms,
            forward_json: bool,
        ) -> Result<BinaryList> {
            let expression = cargo_cli.to_expression();
            let reader_handle = expression
                .stdout_capture()
                .unchecked()
                .reader()
                .map_err(|err| ExpectedError::build_exec_failed(cargo_cli.all_args(), err))?;

            // Read lines as they arrive, forwarding JSON to stdout if requested.
            // XXX should lines be Vec<u8>?
            let mut stdout_buf = BufList::new();
            for line in BufReader::new(&reader_handle).lines() {
                let line = line
                    .map_err(|err| ExpectedError::build_exec_failed(cargo_cli.all_args(), err))?;
                if forward_json {
                    println!("{}", line);
                }
                let mut line = Vec::from(line);
                line.push(b'\n');
                stdout_buf.push_chunk(Bytes::from(line));
            }

            // After reading completes (EOF), the handle is internally waited on.
            // try_wait() returns the output.
            let output = reader_handle
                .try_wait()
                .map_err(|err| ExpectedError::build_exec_failed(cargo_cli.all_args(), err))?
                .expect("child process should have exited after EOF");
            if !output.status.success() {
                return Err(ExpectedError::build_failed(
                    cargo_cli.all_args(),
                    output.status.code(),
                ));
            }

            let test_binaries =
                BinaryList::from_messages(Cursor::new(&stdout_buf), graph, build_platforms)?;
            Ok(test_binaries)
        }
    }
}
