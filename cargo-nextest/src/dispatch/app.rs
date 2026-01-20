// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Top-level application and command routing.

use super::{
    EarlyArgs,
    common::CommonOpts,
    core::{
        App, ArchiveApp, ArchiveOpts, BaseApp, BenchOpts, ListOpts, ReplayOpts, RunOpts,
        exec_replay,
    },
    utility::{DebugCommand, SelfCommand, ShowConfigCommand, StoreCommand},
};
use crate::{ExpectedError, Result, output::OutputContext, reuse_build::ReuseBuildOpts};
use clap::{Args, Subcommand};
use guppy::platform::Platform;
use nextest_runner::user_config::UserConfig;

/// A next-generation test runner for Rust.
///
/// This binary should typically be invoked as `cargo nextest` (in which case
/// this message will not be seen), not `cargo-nextest`.
#[derive(Debug, clap::Parser)]
#[command(
    version = crate::version::short(),
    long_version = crate::version::long(),
    bin_name = "cargo",
    styles = crate::output::clap_styles::style(),
    max_term_width = 100,
)]
pub struct CargoNextestApp {
    /// Early args (color, no_pager) flattened at root for early extraction.
    #[clap(flatten)]
    early_args: EarlyArgs,

    #[clap(subcommand)]
    subcommand: NextestSubcommand,
}

impl CargoNextestApp {
    /// Initializes the output context.
    pub fn init_output(&self) -> OutputContext {
        match &self.subcommand {
            NextestSubcommand::Nextest(args) => args.common.output.init(&self.early_args),
            NextestSubcommand::Ntr(args) => args.common.output.init(&self.early_args),
            #[cfg(unix)]
            // Double-spawned processes should never use coloring.
            NextestSubcommand::DoubleSpawn(_) => OutputContext::color_never_init(),
        }
    }

    /// Executes the app.
    pub fn exec(
        self,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        if let Err(err) = nextest_runner::usdt::register_probes() {
            tracing::warn!("failed to register USDT probes: {}", err);
        }

        match self.subcommand {
            NextestSubcommand::Nextest(app) => {
                app.exec(self.early_args, cli_args, output, output_writer)
            }
            NextestSubcommand::Ntr(opts) => {
                opts.exec(self.early_args, cli_args, output, output_writer)
            }
            #[cfg(unix)]
            NextestSubcommand::DoubleSpawn(opts) => opts.exec(output),
        }
    }
}

#[derive(Debug, Subcommand)]
enum NextestSubcommand {
    /// A next-generation test runner for Rust. <https://nexte.st>.
    Nextest(Box<AppOpts>),
    /// Build and run tests: a shortcut for `cargo nextest run`.
    Ntr(Box<NtrOpts>),
    /// Private command, used to double-spawn test processes.
    #[cfg(unix)]
    #[command(name = nextest_runner::double_spawn::DoubleSpawnInfo::SUBCOMMAND_NAME, hide = true)]
    DoubleSpawn(crate::double_spawn::DoubleSpawnOpts),
}

/// Main app options (under `cargo nextest`).
#[derive(Debug, Args)]
#[clap(
    version = crate::version::short(),
    long_version = crate::version::long(),
    display_name = "cargo-nextest",
)]
pub(crate) struct AppOpts {
    #[clap(flatten)]
    common: CommonOpts,

    #[clap(subcommand)]
    command: Command,
}

impl AppOpts {
    /// Execute the command.
    ///
    /// Returns the exit code.
    fn exec(
        self,
        early_args: EarlyArgs,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        match self.command {
            Command::List(list_opts) => {
                let base = BaseApp::new(
                    output,
                    early_args,
                    list_opts.reuse_build,
                    list_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = App::new(base, list_opts.build_filter)?;
                app.exec_list(list_opts.message_format, list_opts.list_type)?;
                Ok(0)
            }
            Command::Run(run_opts) => {
                let base = BaseApp::new(
                    output,
                    early_args,
                    run_opts.reuse_build,
                    run_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = App::new(base, run_opts.build_filter)?;
                app.exec_run(
                    run_opts.no_capture,
                    run_opts.rerun.as_ref(),
                    &run_opts.runner_opts,
                    &run_opts.reporter_opts,
                    cli_args,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::Bench(bench_opts) => {
                let base = BaseApp::new(
                    output,
                    early_args,
                    ReuseBuildOpts::default(),
                    bench_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;
                let app = App::new(base, bench_opts.build_filter)?;
                app.exec_bench(
                    &bench_opts.runner_opts,
                    &bench_opts.reporter_opts,
                    cli_args,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::Archive(archive_opts) => {
                let app = BaseApp::new(
                    output,
                    early_args,
                    ReuseBuildOpts::default(),
                    archive_opts.cargo_options,
                    self.common.config_opts,
                    self.common.manifest_path,
                    output_writer,
                )?;

                let app = ArchiveApp::new(app, archive_opts.archive_build_filter)?;
                app.exec_archive(
                    &archive_opts.archive_file,
                    archive_opts.archive_format,
                    archive_opts.zstd_level,
                    output_writer,
                )?;
                Ok(0)
            }
            Command::ShowConfig { command } => command.exec(
                early_args,
                self.common.manifest_path,
                self.common.config_opts,
                output,
                output_writer,
            ),
            Command::Self_ { command } => command.exec(output),
            Command::Debug { command } => command.exec(output),
            Command::Replay(replay_opts) => {
                exec_replay(&early_args, *replay_opts, self.common.manifest_path, output)
            }
            Command::Store { command } => {
                let host_platform =
                    Platform::build_target().expect("nextest is built for a supported platform");
                let user_config = UserConfig::for_host_platform(
                    &host_platform,
                    early_args.user_config_location(),
                )
                .map_err(|e| ExpectedError::UserConfigError { err: Box::new(e) })?;
                command.exec(
                    &early_args,
                    self.common.manifest_path,
                    &user_config,
                    output,
                    output_writer,
                )
            }
        }
    }
}

/// All commands supported by nextest.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List tests in workspace.
    ///
    /// This command builds test binaries and queries them for the tests they contain.
    ///
    /// Use --verbose to get more information about tests, including test binary paths and skipped
    /// tests.
    ///
    /// Use --message-format json to get machine-readable output.
    ///
    /// For more information, see <https://nexte.st/docs/listing>.
    List(Box<ListOpts>),
    /// Build and run tests.
    ///
    /// This command builds test binaries and queries them for the tests they contain,
    /// then runs each test in parallel.
    ///
    /// For more information, see <https://nexte.st/docs/running>.
    #[command(visible_alias = "r")]
    Run(Box<RunOpts>),
    /// Build and run benchmarks (experimental).
    ///
    /// This command builds benchmark binaries and queries them for the benchmarks they contain,
    /// then runs each benchmark **serially**.
    ///
    /// This is an experimental feature. To enable it, set the environment variable
    /// `NEXTEST_EXPERIMENTAL_BENCHMARKS=1`.
    #[command(visible_alias = "b")]
    Bench(Box<BenchOpts>),
    /// Build and archive tests.
    ///
    /// This command builds test binaries and archives them to a file. The archive can then be
    /// transferred to another machine, and tests within it can be run with `cargo nextest run
    /// --archive-file`.
    ///
    /// The archive is a tarball compressed with Zstandard (.tar.zst).
    Archive(Box<ArchiveOpts>),
    /// Show information about nextest's configuration in this workspace.
    ///
    /// This command shows configuration information about nextest, including overrides applied to
    /// individual tests.
    ///
    /// In the future, this will show more information about configurations and overrides.
    ShowConfig {
        #[clap(subcommand)]
        command: ShowConfigCommand,
    },
    /// Manage the nextest installation.
    #[clap(name = "self")]
    Self_ {
        #[clap(subcommand)]
        command: SelfCommand,
    },
    /// Debug commands.
    ///
    /// The commands in this section are for nextest's own developers and those integrating with it
    /// to debug issues. They are not part of the public API and may change at any time.
    #[clap(hide = true)]
    Debug {
        #[clap(subcommand)]
        command: DebugCommand,
    },
    /// Replay a recorded test run (experimental).
    ///
    /// This command replays a recorded test run, displaying events as if the run were happening
    /// live.
    ///
    /// This is an experimental feature. To enable it, set the environment variable
    /// `NEXTEST_EXPERIMENTAL_RECORD=1`.
    #[clap(hide = true)]
    Replay(Box<ReplayOpts>),
    /// Manage the record store (experimental).
    ///
    /// This command provides operations for managing the record store, such as pruning old runs
    /// and showing storage information.
    ///
    /// This is an experimental feature. To enable it, set the environment variable
    /// `NEXTEST_EXPERIMENTAL_RECORD=1`.
    #[clap(hide = true)]
    Store {
        #[clap(subcommand)]
        command: StoreCommand,
    },
}

/// Options for `cargo ntr` (shortcut for `cargo nextest run`).
#[derive(Debug, Args)]
struct NtrOpts {
    #[clap(flatten)]
    common: CommonOpts,

    #[clap(flatten)]
    run_opts: RunOpts,
}

impl NtrOpts {
    fn exec(
        self,
        early_args: EarlyArgs,
        cli_args: Vec<String>,
        output: OutputContext,
        output_writer: &mut crate::output::OutputWriter,
    ) -> Result<i32> {
        let base = BaseApp::new(
            output,
            early_args,
            self.run_opts.reuse_build,
            self.run_opts.cargo_options,
            self.common.config_opts,
            self.common.manifest_path,
            output_writer,
        )?;
        let app = App::new(base, self.run_opts.build_filter)?;
        app.exec_run(
            self.run_opts.no_capture,
            self.run_opts.rerun.as_ref(),
            &self.run_opts.runner_opts,
            &self.run_opts.reporter_opts,
            cli_args,
            output_writer,
        )?;
        Ok(0)
    }
}
