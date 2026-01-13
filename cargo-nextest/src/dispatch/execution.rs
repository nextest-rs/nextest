// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Application execution logic.

use super::{
    cli::{
        ArchiveBuildFilter, BenchReporterOpts, BenchRunnerOpts, ListType, MessageFormat,
        MessageFormatOpts, NoTestsBehavior, PagerOpts, ReporterOpts, TestBuildFilter,
        TestRunnerOpts,
    },
    helpers::{
        acquire_graph_data, build_filtersets, detect_build_platforms, resolve_user_config,
        runner_for_target,
    },
};
use crate::{
    ExpectedError, Result, ReuseBuildKind,
    output::{OutputContext, OutputWriter},
};
use camino::Utf8PathBuf;
use guppy::graph::PackageGraph;
use nextest_filtering::{FiltersetKind, ParseContext};
use nextest_runner::{
    cargo_config::{CargoConfigs, EnvironmentMap},
    config::core::{
        ConfigExperimental, EarlyProfile, EvaluatableProfile, NextestConfig, NextestVersionConfig,
        NextestVersionEval,
    },
    double_spawn::DoubleSpawnInfo,
    errors::WriteTestListError,
    helpers::plural,
    input::InputHandlerKind,
    list::{BinaryList, TestExecuteContext, TestList},
    pager::PagedOutput,
    platform::BuildPlatforms,
    redact::Redactor,
    reporter::{
        ShowTerminalProgress,
        events::{FinalRunStats, RunStats, RunStatsFailureKind},
        structured,
    },
    reuse_build::{
        ArchiveReporter, PathMapper, ReuseBuildInfo, apply_archive_filters, archive_to_file,
    },
    run_mode::NextestRunMode,
    runner::configure_handle_inheritance,
    show_config::{ShowTestGroupSettings, ShowTestGroups, ShowTestGroupsMode},
    signal::SignalHandlerKind,
    target_runner::TargetRunner,
    test_filter::{BinaryFilter, TestFilterBuilder},
    test_output::CaptureStrategy,
    user_config::elements::PaginateSetting,
    write_str::WriteStr,
};
use owo_colors::OwoColorize;
use semver::Version;
use std::{
    collections::BTreeSet,
    env::VarError,
    io::IsTerminal,
    sync::{Arc, OnceLock},
};
use tracing::{Level, info, warn};

pub(super) struct BaseApp {
    output: OutputContext,
    // TODO: support multiple --target options
    build_platforms: BuildPlatforms,
    cargo_metadata_json: Arc<String>,
    package_graph: Arc<PackageGraph>,
    // Potentially remapped workspace root (might not be the same as the graph).
    workspace_root: Utf8PathBuf,
    manifest_path: Option<Utf8PathBuf>,
    reuse_build: ReuseBuildInfo,
    cargo_opts: crate::cargo_cli::CargoOptions,
    config_opts: super::cli::ConfigOpts,
    current_version: Version,

    cargo_configs: CargoConfigs,
    double_spawn: OnceLock<DoubleSpawnInfo>,
    target_runner: OnceLock<TargetRunner>,
}

impl BaseApp {
    pub(super) fn new(
        output: OutputContext,
        reuse_build: crate::reuse_build::ReuseBuildOpts,
        cargo_opts: crate::cargo_cli::CargoOptions,
        config_opts: super::cli::ConfigOpts,
        manifest_path: Option<Utf8PathBuf>,
        writer: &mut OutputWriter,
    ) -> Result<Self> {
        reuse_build.check_experimental(output);

        let reuse_build = reuse_build.process(output, writer)?;

        // First obtain the Cargo configs.
        let cargo_configs = CargoConfigs::new(&cargo_opts.config).map_err(Box::new)?;

        // Next, read the build platforms.
        let build_platforms = match reuse_build.binaries_metadata() {
            Some(kind) => kind.binary_list.rust_build_meta.build_platforms.clone(),
            None => detect_build_platforms(&cargo_configs, cargo_opts.target.as_deref())?,
        };

        // Read the Cargo metadata.
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

    pub(super) fn load_config(
        &self,
        pcx: &ParseContext<'_>,
        required_experimental: &BTreeSet<ConfigExperimental>,
    ) -> Result<(
        nextest_runner::config::core::VersionOnlyConfig,
        NextestConfig,
    )> {
        // Load the version-only config first to avoid incompatibilities with parsing the rest of
        // the config.
        let version_only_config = self
            .config_opts
            .make_version_only_config(&self.workspace_root)?;
        self.check_version_config_initial(version_only_config.nextest_version())?;

        let mut experimental = ConfigExperimental::from_env();
        experimental.extend(version_only_config.experimental());

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
            version_only_config.experimental(),
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

    pub(super) fn check_version_config_final(
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

    fn load_double_spawn(&self) -> &DoubleSpawnInfo {
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

    fn load_runner(&self, build_platforms: &BuildPlatforms) -> &TargetRunner {
        self.target_runner.get_or_init(|| {
            runner_for_target(
                &self.cargo_configs,
                build_platforms,
                &self.output.stderr_styles(),
            )
        })
    }

    fn build_binary_list(&self, cargo_command: &str) -> Result<Arc<BinaryList>> {
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

    #[inline]
    pub(super) fn graph(&self) -> &PackageGraph {
        &self.package_graph
    }

    pub(super) fn load_profile<'cfg>(
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

pub(super) fn current_version() -> Version {
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

pub(super) struct App {
    base: BaseApp,
    build_filter: TestBuildFilter,
}

// (_output is not used, but must be passed in to ensure that the output is properly initialized
// before calling this method)
fn check_experimental_filtering(_output: OutputContext) {
    const EXPERIMENTAL_ENV: &str = "NEXTEST_EXPERIMENTAL_FILTER_EXPR";
    if std::env::var(EXPERIMENTAL_ENV).is_ok() {
        warn!(
            "filtersets are no longer experimental: NEXTEST_EXPERIMENTAL_FILTER_EXPR does not need to be set"
        );
    }
}

impl App {
    pub(super) fn new(base: BaseApp, build_filter: TestBuildFilter) -> Result<Self> {
        check_experimental_filtering(base.output);

        Ok(Self { base, build_filter })
    }

    fn build_test_list(
        &self,
        ctx: &TestExecuteContext<'_>,
        binary_list: Arc<BinaryList>,
        test_filter_builder: TestFilterBuilder,
        profile: &EvaluatableProfile<'_>,
    ) -> Result<TestList<'_>> {
        let env = EnvironmentMap::new(&self.base.cargo_configs);
        self.build_filter.compute_test_list(
            ctx,
            self.base.graph(),
            self.base.workspace_root.clone(),
            binary_list,
            test_filter_builder,
            env,
            profile,
            &self.base.reuse_build,
        )
    }

    pub(super) fn exec_list(
        &self,
        message_format: MessageFormatOpts,
        list_type: ListType,
        pager_opts: &PagerOpts,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());

        let (version_only_config, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;
        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;

        let binary_list = self.base.build_binary_list("test")?;

        // Resolve user config to get pager settings.
        let resolved_user_config = resolve_user_config(&self.base.build_platforms.host.platform)?;
        let (pager_setting, paginate) = pager_opts.resolve(&resolved_user_config.ui);

        // Determine if we should page output.
        // Don't page if:
        //
        // - paginate is Never
        // - message_format is not human-readable
        let should_page =
            !matches!(paginate, PaginateSetting::Never) && message_format.is_human_readable();

        // Create paged output if conditions are met.
        let mut paged_output = if should_page {
            PagedOutput::request_pager(
                &pager_setting,
                paginate,
                &resolved_user_config.ui.streampager,
            )
        } else {
            PagedOutput::terminal()
        };

        // (Grab the value of is_interactive before a mutable ref is passed in.)
        let is_interactive = paged_output.is_interactive();
        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stdout);

        match list_type {
            ListType::BinariesOnly => {
                binary_list.write(
                    message_format.to_output_format(self.base.output.verbose, is_interactive),
                    &mut paged_output,
                    should_colorize,
                )?;
            }
            ListType::Full => {
                let double_spawn = self.base.load_double_spawn();
                let target_runner = self
                    .base
                    .load_runner(&binary_list.rust_build_meta.build_platforms);
                let profile =
                    profile.apply_build_platforms(&binary_list.rust_build_meta.build_platforms);
                let ctx = TestExecuteContext {
                    profile_name: profile.name(),
                    double_spawn,
                    target_runner,
                };

                let test_list =
                    self.build_test_list(&ctx, binary_list, test_filter_builder, &profile)?;

                test_list.write(
                    message_format.to_output_format(self.base.output.verbose, is_interactive),
                    &mut paged_output,
                    should_colorize,
                )?;
            }
        }

        // Finalize the pager: close stdin/the pipe, wait for exit.
        paged_output
            .write_str_flush()
            .map_err(WriteTestListError::Io)?;
        paged_output.finalize();

        self.base
            .check_version_config_final(version_only_config.nextest_version())?;
        Ok(())
    }

    pub(super) fn exec_show_test_groups(
        &self,
        show_default: bool,
        groups: Vec<nextest_runner::config::elements::TestGroup>,
        pager_opts: &PagerOpts,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());
        let (_, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;

        // Validate test groups before doing any other work.
        let mode = if groups.is_empty() {
            ShowTestGroupsMode::All
        } else {
            let groups = ShowTestGroups::validate_groups(&profile, groups)?;
            ShowTestGroupsMode::Only(groups)
        };
        let settings = ShowTestGroupSettings { mode, show_default };

        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;

        let binary_list = self.base.build_binary_list("test")?;
        let build_platforms = binary_list.rust_build_meta.build_platforms.clone();

        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(&build_platforms);
        let profile = profile.apply_build_platforms(&build_platforms);
        let ctx = TestExecuteContext {
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, test_filter_builder, &profile)?;

        // Resolve user config to get pager settings.
        let resolved_user_config = resolve_user_config(&self.base.build_platforms.host.platform)?;
        let (pager_setting, paginate) = pager_opts.resolve(&resolved_user_config.ui);

        // Create paged output.
        let mut paged_output = PagedOutput::request_pager(
            &pager_setting,
            paginate,
            &resolved_user_config.ui.streampager,
        );

        let show_test_groups = ShowTestGroups::new(&profile, &test_list, &settings);
        show_test_groups
            .write_human(
                &mut paged_output,
                self.base
                    .output
                    .color
                    .should_colorize(supports_color::Stream::Stdout),
            )
            .map_err(WriteTestListError::Io)?;

        // Finalize the pager: close stdin/the pipe, wait for exit.
        paged_output
            .write_str_flush()
            .map_err(WriteTestListError::Io)?;
        paged_output.finalize();

        Ok(())
    }

    pub(super) fn exec_run(
        &self,
        no_capture: bool,
        runner_opts: &TestRunnerOpts,
        reporter_opts: &ReporterOpts,
        cli_args: Vec<String>,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());
        let (version_only_config, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;

        // Construct this here so that errors are reported before the build step.
        let mut structured_reporter = structured::StructuredReporter::new();
        let message_format = reporter_opts.message_format.unwrap_or_default();

        match message_format {
            MessageFormat::Human => {}
            MessageFormat::LibtestJson | MessageFormat::LibtestJsonPlus => {
                // This is currently an experimental feature, and is gated on this environment
                // variable.
                const EXPERIMENTAL_ENV: &str = "NEXTEST_EXPERIMENTAL_LIBTEST_JSON";
                if std::env::var(EXPERIMENTAL_ENV).as_deref() != Ok("1") {
                    return Err(ExpectedError::ExperimentalFeatureNotEnabled {
                        name: "libtest JSON output",
                        var_name: EXPERIMENTAL_ENV,
                    });
                }

                let libtest = structured::LibtestReporter::new(
                    reporter_opts.message_format_version.as_deref(),
                    if matches!(message_format, MessageFormat::LibtestJsonPlus) {
                        structured::EmitNextestObject::Yes
                    } else {
                        structured::EmitNextestObject::No
                    },
                )?;
                structured_reporter.set_libtest(libtest);
            }
        };

        let cap_strat = if no_capture || runner_opts.interceptor.is_active() {
            CaptureStrategy::None
        } else if matches!(message_format, MessageFormat::Human) {
            CaptureStrategy::Split
        } else {
            CaptureStrategy::Combined
        };

        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr);

        // Load and resolve user config with platform-specific overrides.
        let resolved_user_config = resolve_user_config(&self.base.build_platforms.host.platform)?;

        // Make the runner and reporter builders. Do them now so warnings are
        // emitted before we start doing the build.
        let runner_builder = runner_opts.to_builder(cap_strat);
        let mut reporter_builder = reporter_opts.to_builder(
            runner_opts.no_run,
            no_capture || runner_opts.interceptor.is_active(),
            should_colorize,
            &resolved_user_config.ui,
        );
        reporter_builder.set_verbose(self.base.output.verbose);

        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;

        let binary_list = self.base.build_binary_list("test")?;
        let build_platforms = &binary_list.rust_build_meta.build_platforms.clone();
        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(build_platforms);

        let profile = profile.apply_build_platforms(build_platforms);
        let ctx = TestExecuteContext {
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, test_filter_builder, &profile)?;

        // Validate interceptor mode requirements.
        if runner_opts.interceptor.is_active() {
            let test_count = test_list.run_count();

            if test_count == 0 {
                if let Some(debugger) = &runner_opts.interceptor.debugger {
                    return Err(ExpectedError::DebuggerNoTests {
                        debugger: debugger.clone(),
                        mode: NextestRunMode::Test,
                    });
                } else if let Some(tracer) = &runner_opts.interceptor.tracer {
                    return Err(ExpectedError::TracerNoTests {
                        tracer: tracer.clone(),
                        mode: NextestRunMode::Test,
                    });
                } else {
                    unreachable!("interceptor is active but neither debugger nor tracer is set");
                }
            } else if test_count > 1 {
                // Collect the first 8 matching test instances for the error
                // message.
                let test_instances: Vec<_> = test_list
                    .iter_tests()
                    .filter(|test| test.test_info.filter_match.is_match())
                    .take(8)
                    .map(|test| test.id().to_owned())
                    .collect();

                if let Some(debugger) = &runner_opts.interceptor.debugger {
                    return Err(ExpectedError::DebuggerTooManyTests {
                        debugger: debugger.clone(),
                        mode: NextestRunMode::Test,
                        test_count,
                        test_instances,
                    });
                } else if let Some(tracer) = &runner_opts.interceptor.tracer {
                    return Err(ExpectedError::TracerTooManyTests {
                        tracer: tracer.clone(),
                        mode: NextestRunMode::Test,
                        test_count,
                        test_instances,
                    });
                } else {
                    unreachable!("interceptor is active but neither debugger nor tracer is set");
                }
            }
        }

        let output = output_writer.reporter_output();

        let signal_handler = if runner_opts.interceptor.debugger.is_some() {
            // Only debuggers use special signal handling. Tracers use standard
            // handling.
            SignalHandlerKind::DebuggerMode
        } else {
            SignalHandlerKind::Standard
        };

        let input_handler =
            if reporter_opts.no_input_handler || runner_opts.interceptor.debugger.is_some() {
                // Only debuggers disable the input handler -- tracers do not.
                InputHandlerKind::Noop
            } else if resolved_user_config.ui.input_handler {
                // This means that the input handler determines whether it
                // should be enabled.
                InputHandlerKind::Standard
            } else {
                InputHandlerKind::Noop
            };

        // Make the runner.
        let Some(runner_builder) = runner_builder else {
            // This means --no-run was passed in. Exit.
            return Ok(());
        };
        let runner = runner_builder.build(
            &test_list,
            &profile,
            cli_args,
            signal_handler,
            input_handler,
            double_spawn.clone(),
            target_runner.clone(),
        )?;

        // Make the reporter.
        let show_term_progress = ShowTerminalProgress::from_cargo_configs(
            &self.base.cargo_configs,
            std::io::stderr().is_terminal(),
        );
        let mut reporter = reporter_builder.build(
            &test_list,
            &profile,
            show_term_progress,
            output,
            structured_reporter,
        );

        configure_handle_inheritance(no_capture)?;
        let run_stats = runner.try_execute(|event| {
            // Write and flush the event.
            reporter.report_event(event)
        })?;
        reporter.finish();
        self.base
            .check_version_config_final(version_only_config.nextest_version())?;

        final_result(NextestRunMode::Test, run_stats, runner_opts.no_tests)
    }

    pub(super) fn exec_bench(
        &self,
        runner_opts: &BenchRunnerOpts,
        reporter_opts: &BenchReporterOpts,
        cli_args: Vec<String>,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());
        let (version_only_config, config) = self.base.load_config(
            &pcx,
            &[ConfigExperimental::Benchmarks].into_iter().collect(),
        )?;
        let profile = self.base.load_profile(&config)?;

        // Construct this here so that errors are reported before the build step.
        let structured_reporter = structured::StructuredReporter::new();
        // TODO: support message format for benchmarks.
        // TODO: maybe support capture strategy for benchmarks?
        let cap_strat = CaptureStrategy::None;

        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr);

        // Load and resolve user config with platform-specific overrides.
        let resolved_user_config = resolve_user_config(&self.base.build_platforms.host.platform)?;

        // Make the runner and reporter builders. Do them now so warnings are
        // emitted before we start doing the build.
        let runner_builder = runner_opts.to_builder(cap_strat);
        let mut reporter_builder =
            reporter_opts.to_builder(should_colorize, &resolved_user_config.ui);
        reporter_builder.set_verbose(self.base.output.verbose);

        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Benchmark, filter_exprs)?;

        let binary_list = self.base.build_binary_list("bench")?;
        let build_platforms = &binary_list.rust_build_meta.build_platforms.clone();
        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(build_platforms);

        let profile = profile.apply_build_platforms(build_platforms);
        let ctx = TestExecuteContext {
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, test_filter_builder, &profile)?;

        // Validate interceptor mode requirements.
        if runner_opts.interceptor.is_active() {
            let test_count = test_list.run_count();

            if test_count == 0 {
                if let Some(debugger) = &runner_opts.interceptor.debugger {
                    return Err(ExpectedError::DebuggerNoTests {
                        debugger: debugger.clone(),
                        mode: NextestRunMode::Benchmark,
                    });
                } else if let Some(tracer) = &runner_opts.interceptor.tracer {
                    return Err(ExpectedError::TracerNoTests {
                        tracer: tracer.clone(),
                        mode: NextestRunMode::Benchmark,
                    });
                } else {
                    unreachable!("interceptor is active but neither debugger nor tracer is set");
                }
            } else if test_count > 1 {
                // Collect the first 8 matching test instances for the error
                // message.
                let test_instances: Vec<_> = test_list
                    .iter_tests()
                    .filter(|test| test.test_info.filter_match.is_match())
                    .take(8)
                    .map(|test| test.id().to_owned())
                    .collect();

                if let Some(debugger) = &runner_opts.interceptor.debugger {
                    return Err(ExpectedError::DebuggerTooManyTests {
                        debugger: debugger.clone(),
                        mode: NextestRunMode::Benchmark,
                        test_count,
                        test_instances,
                    });
                } else if let Some(tracer) = &runner_opts.interceptor.tracer {
                    return Err(ExpectedError::TracerTooManyTests {
                        tracer: tracer.clone(),
                        mode: NextestRunMode::Benchmark,
                        test_count,
                        test_instances,
                    });
                } else {
                    unreachable!("interceptor is active but neither debugger nor tracer is set");
                }
            }
        }

        let output = output_writer.reporter_output();

        let signal_handler = if runner_opts.interceptor.debugger.is_some() {
            // Only debuggers use special signal handling. Tracers use standard
            // handling.
            SignalHandlerKind::DebuggerMode
        } else {
            SignalHandlerKind::Standard
        };

        let input_handler =
            if reporter_opts.no_input_handler || runner_opts.interceptor.debugger.is_some() {
                // Only debuggers disable the input handler -- tracers do not.
                InputHandlerKind::Noop
            } else if resolved_user_config.ui.input_handler {
                InputHandlerKind::Standard
            } else {
                InputHandlerKind::Noop
            };

        // Make the runner.
        let Some(runner_builder) = runner_builder else {
            // This means --no-run was passed in. Exit.
            return Ok(());
        };
        let runner = runner_builder.build(
            &test_list,
            &profile,
            cli_args,
            signal_handler,
            input_handler,
            double_spawn.clone(),
            target_runner.clone(),
        )?;

        // Make the reporter.
        let show_term_progress = ShowTerminalProgress::from_cargo_configs(
            &self.base.cargo_configs,
            std::io::stderr().is_terminal(),
        );
        let mut reporter = reporter_builder.build(
            &test_list,
            &profile,
            show_term_progress,
            output,
            structured_reporter,
        );

        // TODO: no_capture is always true for benchmarks for now.
        configure_handle_inheritance(true)?;
        let run_stats = runner.try_execute(|event| {
            // Write and flush the event.
            reporter.report_event(event)
        })?;
        reporter.finish();
        self.base
            .check_version_config_final(version_only_config.nextest_version())?;

        final_result(NextestRunMode::Benchmark, run_stats, runner_opts.no_tests)
    }
}

pub(super) struct ArchiveApp {
    base: BaseApp,
    archive_filter: ArchiveBuildFilter,
}

impl ArchiveApp {
    pub(super) fn new(base: BaseApp, archive_filter: ArchiveBuildFilter) -> Result<Self> {
        Ok(Self {
            base,
            archive_filter,
        })
    }

    pub(super) fn exec_archive(
        &self,
        output_file: &camino::Utf8Path,
        format: crate::reuse_build::ArchiveFormatOpt,
        zstd_level: i32,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        // Do format detection first so we fail immediately.
        let format = format.to_archive_format(output_file)?;
        let binary_list = self.base.build_binary_list("test")?;
        let path_mapper = PathMapper::noop();
        let build_platforms = &binary_list.rust_build_meta.build_platforms;
        let pcx = ParseContext::new(self.base.graph());
        let (_, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self
            .base
            .load_profile(&config)?
            .apply_build_platforms(build_platforms);
        let redactor = if crate::output::should_redact() {
            Redactor::build_active(&binary_list.rust_build_meta)
                .with_path(output_file.to_path_buf(), "<archive-file>".to_owned())
                .build()
        } else {
            Redactor::noop()
        };
        let mut reporter = ArchiveReporter::new(self.base.output.verbose, redactor.clone());

        if self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr)
        {
            reporter.colorize();
        }

        let filtersets = build_filtersets(
            &pcx,
            &self.archive_filter.filterset,
            FiltersetKind::TestArchive,
        )?;
        let binary_filter = BinaryFilter::new(filtersets);
        let ecx = profile.filterset_ecx();

        let (binary_list_to_archive, filter_counts) = apply_archive_filters(
            self.base.graph(),
            binary_list.clone(),
            &binary_filter,
            &ecx,
            &path_mapper,
        )?;

        let mut writer = output_writer.stderr_writer();
        archive_to_file(
            profile,
            &binary_list_to_archive,
            filter_counts,
            &self.base.cargo_metadata_json,
            &self.base.package_graph,
            // Note that path_mapper is currently a no-op -- we don't support reusing builds for
            // archive creation because it's too confusing.
            &path_mapper,
            format,
            zstd_level,
            output_file,
            |event| {
                reporter.report_event(event, &mut writer)?;
                writer.write_str_flush()
            },
            redactor.clone(),
        )
        .map_err(|err| ExpectedError::ArchiveCreateError {
            archive_file: output_file.to_owned(),
            err,
            redactor,
        })?;

        Ok(())
    }
}

fn final_result(
    mode: NextestRunMode,
    run_stats: RunStats,
    no_tests: Option<NoTestsBehavior>,
) -> Result<(), ExpectedError> {
    match run_stats.summarize_final() {
        FinalRunStats::Success => Ok(()),
        FinalRunStats::NoTestsRun => match no_tests {
            Some(NoTestsBehavior::Pass) => Ok(()),
            Some(NoTestsBehavior::Warn) => {
                warn!("no {} to run", plural::tests_plural(mode));
                Ok(())
            }
            Some(NoTestsBehavior::Fail) => Err(ExpectedError::NoTestsRun {
                mode,
                is_default: false,
            }),
            None => Err(ExpectedError::NoTestsRun {
                mode,
                is_default: true,
            }),
        },
        FinalRunStats::Cancelled {
            reason: _,
            kind: RunStatsFailureKind::SetupScript,
        }
        | FinalRunStats::Failed {
            kind: RunStatsFailureKind::SetupScript,
        } => Err(ExpectedError::setup_script_failed()),
        FinalRunStats::Cancelled {
            reason: _,
            kind: RunStatsFailureKind::Test { .. },
        }
        | FinalRunStats::Failed {
            kind: RunStatsFailureKind::Test { .. },
        } => Err(ExpectedError::test_run_failed()),
    }
}
