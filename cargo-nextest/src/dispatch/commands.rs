// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Subcommand implementations for show-config, self, and debug commands.

use super::{
    EarlyArgs,
    cli::{ConfigOpts, TestBuildFilter},
    execution::{App, BaseApp},
    helpers::{detect_build_platforms, display_output_slice, extract_slice_from_output},
};
use crate::{
    ExpectedError, Result,
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, OutputWriter},
    reuse_build::ReuseBuildOpts,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Subcommand, ValueEnum};
use nextest_runner::{
    cargo_config::CargoConfigs,
    config::core::NextestVersionEval,
    errors::WriteTestListError,
    helpers::ThemeCharacters,
    pager::PagedOutput,
    record::{
        DisplayRunList, PruneKind, RecordRetentionPolicy, RunStore, Styles as RecordStyles,
        records_cache_dir,
    },
    user_config::{UserConfig, elements::RecordConfig},
    write_str::WriteStr,
};
use std::fmt;
use tracing::{Level, info};

#[derive(Debug, Subcommand)]
pub(super) enum ShowConfigCommand {
    /// Show version-related configuration.
    Version {},
    /// Show defined test groups and their associated tests.
    TestGroups {
        /// Show default test groups.
        #[arg(long)]
        show_default: bool,

        /// Show only the named groups.
        #[arg(long)]
        groups: Vec<nextest_runner::config::elements::TestGroup>,

        #[clap(flatten)]
        cargo_options: Box<CargoOptions>,

        #[clap(flatten)]
        build_filter: TestBuildFilter,

        #[clap(flatten)]
        reuse_build: Box<ReuseBuildOpts>,
    },
}

impl ShowConfigCommand {
    pub(super) fn exec(
        self,
        early_args: EarlyArgs,
        manifest_path: Option<Utf8PathBuf>,
        config_opts: ConfigOpts,
        output: OutputContext,
        output_writer: &mut OutputWriter,
    ) -> Result<i32> {
        match self {
            Self::Version {} => {
                let mut cargo_cli =
                    CargoCli::new("locate-project", manifest_path.as_deref(), output);
                cargo_cli.add_args(["--workspace", "--message-format=plain"]);
                let locate_project_output = cargo_cli
                    .to_expression()
                    .stdout_capture()
                    .unchecked()
                    .run()
                    .map_err(|error| {
                        ExpectedError::cargo_locate_project_exec_failed(cargo_cli.all_args(), error)
                    })?;
                if !locate_project_output.status.success() {
                    return Err(ExpectedError::cargo_locate_project_failed(
                        cargo_cli.all_args(),
                        locate_project_output.status,
                    ));
                }
                let workspace_root = String::from_utf8(locate_project_output.stdout)
                    .map_err(|err| ExpectedError::WorkspaceRootInvalidUtf8 { err })?;
                let workspace_root = Utf8Path::new(workspace_root.trim_end());
                let workspace_root =
                    workspace_root
                        .parent()
                        .ok_or_else(|| ExpectedError::WorkspaceRootInvalid {
                            workspace_root: workspace_root.to_owned(),
                        })?;

                let config = config_opts.make_version_only_config(workspace_root)?;
                let current_version = super::execution::current_version();

                let show = nextest_runner::show_config::ShowNextestVersion::new(
                    config.nextest_version(),
                    &current_version,
                    config_opts.override_version_check,
                );
                show.write_human(
                    &mut output_writer.stdout_writer(),
                    output.color.should_colorize(supports_color::Stream::Stdout),
                )
                .map_err(WriteTestListError::Io)?;

                match config
                    .nextest_version()
                    .eval(&current_version, config_opts.override_version_check)
                {
                    NextestVersionEval::Satisfied => Ok(0),
                    NextestVersionEval::Error { .. } => {
                        crate::helpers::log_needs_update(
                            Level::ERROR,
                            crate::helpers::BYPASS_VERSION_TEXT,
                            &output.stderr_styles(),
                        );
                        Ok(nextest_metadata::NextestExitCode::REQUIRED_VERSION_NOT_MET)
                    }
                    NextestVersionEval::Warn { .. } => {
                        crate::helpers::log_needs_update(
                            Level::WARN,
                            crate::helpers::BYPASS_VERSION_TEXT,
                            &output.stderr_styles(),
                        );
                        Ok(nextest_metadata::NextestExitCode::RECOMMENDED_VERSION_NOT_MET)
                    }
                    NextestVersionEval::ErrorOverride { .. }
                    | NextestVersionEval::WarnOverride { .. } => Ok(0),
                }
            }
            Self::TestGroups {
                show_default,
                groups,
                cargo_options,
                build_filter,
                reuse_build,
            } => {
                let base = BaseApp::new(
                    output,
                    early_args,
                    *reuse_build,
                    *cargo_options,
                    config_opts,
                    manifest_path,
                    output_writer,
                )?;
                let app = App::new(base, build_filter)?;

                app.exec_show_test_groups(show_default, groups)?;

                Ok(0)
            }
        }
    }
}

/// Arguments for specifying which version to update to.
#[derive(Debug, Args)]
#[group(id = "version_spec", multiple = false)]
pub(super) struct UpdateVersionOpt {
    /// Version or version range to download.
    #[arg(long, help_heading = "Version selection")]
    version: Option<String>,

    /// Update to the latest version, including prereleases.
    ///
    /// This installs the latest beta, RC, or stable version.
    #[arg(long, help_heading = "Version selection")]
    beta: bool,

    /// Update to the latest RC or stable version.
    ///
    /// This installs the latest RC or stable version. Beta versions are
    /// excluded.
    #[arg(long, help_heading = "Version selection")]
    rc: bool,
}

impl UpdateVersionOpt {
    /// Converts to `UpdateVersionReq`.
    #[cfg(feature = "self-update")]
    pub(super) fn to_update_version_req(
        &self,
    ) -> Result<nextest_runner::update::UpdateVersionReq, ExpectedError> {
        use nextest_runner::update::{PrereleaseKind, UpdateVersionReq};

        if self.beta {
            Ok(UpdateVersionReq::LatestPrerelease(PrereleaseKind::Beta))
        } else if self.rc {
            Ok(UpdateVersionReq::LatestPrerelease(PrereleaseKind::Rc))
        } else if let Some(version) = &self.version {
            version
                .parse()
                .map(UpdateVersionReq::Version)
                .map_err(|err| ExpectedError::UpdateVersionParseError { err })
        } else {
            Ok(UpdateVersionReq::Latest)
        }
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum SelfCommand {
    #[clap(hide = true)]
    /// Perform setup actions (currently a no-op).
    Setup {
        /// The entity running the setup command.
        #[arg(long, value_enum, default_value_t = SetupSource::User)]
        source: SetupSource,
    },
    #[cfg_attr(
        not(feature = "self-update"),
        doc = "This version of nextest does not have self-update enabled.\n\
        \n\
        Always exits with code 93 (SELF_UPDATE_UNAVAILABLE).
        "
    )]
    #[cfg_attr(
        feature = "self-update",
        doc = "Download and install updates to nextest.\n\
        \n\
        This command checks the internet for updates to nextest, then downloads and
        installs them if an update is available."
    )]
    Update {
        #[command(flatten)]
        version: UpdateVersionOpt,

        /// Check for updates rather than downloading them.
        ///
        /// If no update is available, exits with code 0. If an update is available, exits with code
        /// 80 (UPDATE_AVAILABLE).
        #[arg(short = 'n', long)]
        check: bool,

        /// Do not prompt for confirmation.
        #[arg(short = 'y', long, conflicts_with = "check")]
        yes: bool,

        /// Force downgrades and reinstalls.
        #[arg(short, long)]
        force: bool,

        /// URL or path to fetch releases.json from.
        #[arg(long)]
        releases_url: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum SetupSource {
    User,
    SelfUpdate,
    PackageManager,
}

impl SelfCommand {
    #[cfg_attr(not(feature = "self-update"), expect(unused_variables))]
    pub(super) fn exec(self, output: OutputContext) -> Result<i32> {
        match self {
            Self::Setup { source: _source } => Ok(0),
            Self::Update {
                version,
                check,
                yes,
                force,
                releases_url,
            } => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "self-update")] {
                        let version_req = version.to_update_version_req()?;
                        crate::update::perform_update(
                            version_req,
                            check,
                            yes,
                            force,
                            releases_url,
                            output,
                        )
                    } else {
                        let _ = version;
                        tracing::info!(
                            "this version of cargo-nextest cannot perform self-updates\n\
                            (hint: this usually means nextest was installed by a package manager)"
                        );
                        Ok(nextest_metadata::NextestExitCode::SELF_UPDATE_UNAVAILABLE)
                    }
                }
            }
        }
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum DebugCommand {
    /// Show the data that nextest would extract from standard output or standard error.
    ///
    /// Text extraction is a heuristic process driven by a bunch of regexes and other similar logic.
    /// This command shows what nextest would extract from a given input.
    Extract {
        /// The path to the standard output produced by the test process.
        #[arg(long, required_unless_present_any = ["stderr", "combined"])]
        stdout: Option<Utf8PathBuf>,

        /// The path to the standard error produced by the test process.
        #[arg(long, required_unless_present_any = ["stdout", "combined"])]
        stderr: Option<Utf8PathBuf>,

        /// The combined output produced by the test process.
        #[arg(long, conflicts_with_all = ["stdout", "stderr"])]
        combined: Option<Utf8PathBuf>,

        /// The kind of output to produce.
        #[arg(value_enum)]
        output_format: ExtractOutputFormat,
    },

    /// Print the current executable path.
    CurrentExe,

    /// Show the build platforms that nextest would use.
    BuildPlatforms {
        /// The target triple to use.
        #[arg(long)]
        target: Option<String>,

        /// Override a Cargo Configuration value.
        #[arg(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Output format.
        #[arg(long, value_enum, default_value_t)]
        output_format: BuildPlatformsOutputFormat,
    },
}

impl DebugCommand {
    pub(super) fn exec(self, output: OutputContext) -> Result<i32> {
        let _ = output;

        match self {
            DebugCommand::Extract {
                stdout,
                stderr,
                combined,
                output_format,
            } => {
                if let Some(combined) = combined {
                    let combined = std::fs::read(&combined).map_err(|err| {
                        ExpectedError::DebugExtractReadError {
                            kind: "combined",
                            path: combined,
                            err,
                        }
                    })?;

                    let description_kind = extract_slice_from_output(&combined, &combined);
                    display_output_slice(description_kind, output_format)?;
                } else {
                    let stdout = stdout
                        .map(|path| {
                            std::fs::read(&path).map_err(|err| {
                                ExpectedError::DebugExtractReadError {
                                    kind: "stdout",
                                    path,
                                    err,
                                }
                            })
                        })
                        .transpose()?
                        .unwrap_or_default();
                    let stderr = stderr
                        .map(|path| {
                            std::fs::read(&path).map_err(|err| {
                                ExpectedError::DebugExtractReadError {
                                    kind: "stderr",
                                    path,
                                    err,
                                }
                            })
                        })
                        .transpose()?
                        .unwrap_or_default();

                    let output_slice = extract_slice_from_output(&stdout, &stderr);
                    display_output_slice(output_slice, output_format)?;
                }
            }
            DebugCommand::CurrentExe => {
                let exe = std::env::current_exe()
                    .map_err(|err| ExpectedError::GetCurrentExeFailed { err })?;
                println!("{}", exe.display());
            }
            DebugCommand::BuildPlatforms {
                target,
                config,
                output_format,
            } => {
                let cargo_configs = CargoConfigs::new(&config).map_err(Box::new)?;
                let build_platforms = detect_build_platforms(&cargo_configs, target.as_deref())?;
                match output_format {
                    BuildPlatformsOutputFormat::Debug => {
                        println!("{build_platforms:#?}");
                    }
                    BuildPlatformsOutputFormat::Triple => {
                        println!(
                            "host triple: {}",
                            build_platforms.host.platform.triple().as_str()
                        );
                        if let Some(target) = &build_platforms.target {
                            println!(
                                "target triple: {}",
                                target.triple.platform.triple().as_str()
                            );
                        } else {
                            println!("target triple: (none)");
                        }
                    }
                }
            }
        }

        Ok(0)
    }
}

/// Output format for `nextest debug extract`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExtractOutputFormat {
    /// Show the raw text extracted.
    Raw,

    /// Show what would be put in the description field of JUnit reports.
    ///
    /// This is similar to `Raw`, but is valid Unicode, and strips out ANSI escape codes and other
    /// invalid XML characters.
    JunitDescription,

    /// Show what would be highlighted in nextest's output.
    Highlight,
}

impl fmt::Display for ExtractOutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::JunitDescription => write!(f, "junit-description"),
            Self::Highlight => write!(f, "highlight"),
        }
    }
}

/// Output format for `nextest debug build-platforms`.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum BuildPlatformsOutputFormat {
    /// Show Debug output.
    #[default]
    Debug,

    /// Show just the triple.
    Triple,
}

/// Subcommands for managing the record store.
#[derive(Debug, Subcommand)]
pub(super) enum StoreCommand {
    /// List all recorded runs.
    List {
        /// Show verbose output.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Prune old recorded runs according to retention policy.
    Prune(PruneOpts),
}

impl StoreCommand {
    pub(super) fn exec(
        self,
        early_args: &EarlyArgs,
        manifest_path: Option<Utf8PathBuf>,
        user_config: &UserConfig,
        output: OutputContext,
        output_writer: &mut OutputWriter,
    ) -> Result<i32> {
        let mut cargo_cli = CargoCli::new("locate-project", manifest_path.as_deref(), output);
        cargo_cli.add_args(["--workspace", "--message-format=plain"]);
        let locate_project_output = cargo_cli
            .to_expression()
            .stdout_capture()
            .unchecked()
            .run()
            .map_err(|error| {
                ExpectedError::cargo_locate_project_exec_failed(cargo_cli.all_args(), error)
            })?;
        if !locate_project_output.status.success() {
            return Err(ExpectedError::cargo_locate_project_failed(
                cargo_cli.all_args(),
                locate_project_output.status,
            ));
        }
        let workspace_root = String::from_utf8(locate_project_output.stdout)
            .map_err(|err| ExpectedError::WorkspaceRootInvalidUtf8 { err })?;
        let workspace_root = Utf8Path::new(workspace_root.trim_end());
        let workspace_root =
            workspace_root
                .parent()
                .ok_or_else(|| ExpectedError::WorkspaceRootInvalid {
                    workspace_root: workspace_root.to_owned(),
                })?;

        let cache_dir = records_cache_dir(workspace_root)
            .map_err(|err| ExpectedError::RecordCacheDirNotFound { err })?;

        let (pager_setting, paginate) = early_args.resolve_pager(&user_config.ui);
        let mut paged_output =
            PagedOutput::request_pager(&pager_setting, paginate, &user_config.ui.streampager);

        let mut styles = RecordStyles::default();
        let mut theme_characters = ThemeCharacters::default();
        if output.color.should_colorize(supports_color::Stream::Stdout) {
            styles.colorize();
        }
        if supports_unicode::on(supports_unicode::Stream::Stdout) {
            theme_characters.use_unicode();
        }

        match self {
            Self::List { verbose } => {
                let store = RunStore::new(&cache_dir)
                    .map_err(|err| ExpectedError::RecordSetupError { err })?;

                let mut snapshot = store
                    .lock_shared()
                    .map_err(|err| ExpectedError::RecordSetupError { err })?
                    .into_snapshot();

                // Sort runs by start time, most recent first.
                snapshot
                    .runs_mut()
                    .sort_by(|a, b| b.started_at.cmp(&a.started_at));

                let store_path = if verbose {
                    Some(cache_dir.as_path())
                } else {
                    None
                };
                let display =
                    DisplayRunList::new(&snapshot, store_path, &styles, &theme_characters);
                write!(paged_output, "{}", display)
                    .map_err(|err| ExpectedError::WriteError { err })?;

                if snapshot.run_count() == 0 {
                    info!("no recorded runs");
                }

                Ok(0)
            }
            Self::Prune(opts) => opts.exec(
                &cache_dir,
                &user_config.record,
                &styles,
                &mut paged_output,
                output_writer,
            ),
        }
    }
}

/// Options for the `cargo nextest store prune` command.
#[derive(Debug, Args)]
pub(super) struct PruneOpts {
    /// Show what would be deleted without actually deleting.
    #[arg(long)]
    dry_run: bool,
}

impl PruneOpts {
    fn exec(
        &self,
        cache_dir: &Utf8Path,
        record_config: &RecordConfig,
        styles: &RecordStyles,
        paged_output: &mut PagedOutput,
        output_writer: &mut OutputWriter,
    ) -> Result<i32> {
        let store =
            RunStore::new(cache_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;
        let policy = RecordRetentionPolicy::from(record_config);

        if self.dry_run {
            // Dry run: show what would be deleted via paged output.
            let snapshot = store
                .lock_shared()
                .map_err(|err| ExpectedError::RecordSetupError { err })?
                .into_snapshot();

            let plan = snapshot.compute_prune_plan(&policy);

            write!(
                paged_output,
                "{}",
                plan.display(snapshot.run_id_index(), styles)
            )
            .map_err(|err| ExpectedError::WriteError { err })?;
            Ok(0)
        } else {
            // Actual prune: output to stderr.
            let mut locked = store
                .lock_exclusive()
                .map_err(|err| ExpectedError::RecordSetupError { err })?;
            let result = locked
                .prune(&policy, PruneKind::Explicit)
                .map_err(|err| ExpectedError::RecordSetupError { err })?;

            write!(output_writer.stderr_writer(), "{}", result.display(styles))
                .map_err(|err| ExpectedError::WriteError { err })?;
            Ok(0)
        }
    }
}
