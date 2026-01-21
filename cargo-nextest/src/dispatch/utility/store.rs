// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Store command implementation for managing the record store.

use crate::{
    ExpectedError, Result,
    cargo_cli::CargoCli,
    dispatch::EarlyArgs,
    output::{OutputContext, OutputWriter},
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use clap::{Args, Subcommand};
use nextest_runner::{
    helpers::ThemeCharacters,
    pager::PagedOutput,
    record::{
        DisplayRunList, PruneKind, RecordRetentionPolicy, RunIdSelector, RunStore,
        SnapshotWithReplayability, Styles as RecordStyles, records_cache_dir,
    },
    redact::Redactor,
    user_config::{UserConfig, elements::RecordConfig},
    write_str::WriteStr,
};
use tracing::info;

/// Subcommands for managing the record store.
#[derive(Debug, Subcommand)]
pub(crate) enum StoreCommand {
    /// List all recorded runs.
    List {},
    /// Show detailed information about a recorded run.
    Info(InfoOpts),
    /// Prune old recorded runs according to retention policy.
    Prune(PruneOpts),
}

/// Options for the `cargo nextest store info` command.
#[derive(Debug, Args)]
pub(crate) struct InfoOpts {
    /// Run ID to show info for, or `latest` [aliases: -R].
    ///
    /// Accepts "latest" for the most recent completed run, or a full UUID or
    /// unambiguous prefix.
    #[arg(value_name = "RUN_ID", required_unless_present = "run_id_opt")]
    run_id: Option<RunIdSelector>,

    /// Run ID to show info for (alternative to positional argument).
    #[arg(
        short = 'R',
        long = "run-id",
        hide = true,
        value_name = "RUN_ID",
        conflicts_with = "run_id"
    )]
    run_id_opt: Option<RunIdSelector>,
}

impl InfoOpts {
    fn resolved_run_id(&self) -> &RunIdSelector {
        // One of these must be Some due to clap's required_unless_present.
        self.run_id
            .as_ref()
            .or(self.run_id_opt.as_ref())
            .expect("run_id or run_id_opt is present due to clap validation")
    }

    fn exec(
        &self,
        cache_dir: &Utf8Path,
        styles: &RecordStyles,
        theme_characters: &ThemeCharacters,
        paged_output: &mut PagedOutput,
        redactor: &Redactor,
    ) -> Result<i32> {
        let store =
            RunStore::new(cache_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;

        let snapshot = store
            .lock_shared()
            .map_err(|err| ExpectedError::RecordSetupError { err })?
            .into_snapshot();

        let resolved = snapshot
            .resolve_run_id(self.resolved_run_id())
            .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
        let run_id = resolved.run_id;

        // This should never fail since we just resolved the run ID.
        let run = snapshot
            .get_run(run_id)
            .expect("run ID was just resolved, so the run should exist");

        let replayability = run.check_replayability(snapshot.runs_dir());
        let display = run.display_detailed(
            snapshot.run_id_index(),
            &replayability,
            Utc::now(),
            styles,
            theme_characters,
            redactor,
        );

        write!(paged_output, "{}", display).map_err(|err| ExpectedError::WriteError { err })?;

        Ok(0)
    }
}

/// Options for the `cargo nextest store prune` command.
#[derive(Debug, Args)]
pub(crate) struct PruneOpts {
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
        redactor: &Redactor,
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
                plan.display(snapshot.run_id_index(), styles, redactor)
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

impl StoreCommand {
    pub(crate) fn exec(
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

        // Create redactor for snapshot testing if __NEXTEST_REDACT=1.
        let redactor = if crate::output::should_redact() {
            Redactor::for_snapshot_testing()
        } else {
            Redactor::noop()
        };

        match self {
            Self::List {} => {
                let store = RunStore::new(&cache_dir)
                    .map_err(|err| ExpectedError::RecordSetupError { err })?;

                let snapshot = store
                    .lock_shared()
                    .map_err(|err| ExpectedError::RecordSetupError { err })?
                    .into_snapshot();

                let store_path = if output.verbose {
                    Some(cache_dir.as_path())
                } else {
                    None
                };
                let snapshot_with_replayability = SnapshotWithReplayability::new(&snapshot);
                let display = DisplayRunList::new(
                    &snapshot_with_replayability,
                    store_path,
                    &styles,
                    &theme_characters,
                    &redactor,
                );
                write!(paged_output, "{}", display)
                    .map_err(|err| ExpectedError::WriteError { err })?;

                if snapshot.run_count() == 0 {
                    info!("no recorded runs");
                }

                Ok(0)
            }
            Self::Info(opts) => opts.exec(
                &cache_dir,
                &styles,
                &theme_characters,
                &mut paged_output,
                &redactor,
            ),
            Self::Prune(opts) => opts.exec(
                &cache_dir,
                &user_config.record,
                &styles,
                &mut paged_output,
                output_writer,
                &redactor,
            ),
        }
    }
}
