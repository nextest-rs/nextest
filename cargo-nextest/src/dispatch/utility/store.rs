// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Store command implementation for managing the record store.

use crate::{
    ExpectedError, Result,
    dispatch::{EarlyArgs, helpers::locate_workspace_root},
    output::OutputContext,
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use clap::{Args, Subcommand, ValueEnum};
use nextest_runner::{
    helpers::ThemeCharacters,
    pager::PagedOutput,
    record::{
        ChromeTraceGroupBy, ChromeTraceMessageFormat, DisplayRunList, PortableRecording,
        PortableRecordingWriter, PruneKind, RecordReader, RecordRetentionPolicy, RecordedRunStatus,
        RunIdIndex, RunIdOrRecordingSelector, RunIdSelector, RunStore, STORE_FORMAT_VERSION,
        SnapshotWithReplayability, Styles as RecordStyles, convert_to_chrome_trace,
        has_zip_extension, records_state_dir,
    },
    redact::Redactor,
    user_config::{UserConfig, elements::RecordConfig},
    write_str::WriteStr,
};
use owo_colors::OwoColorize;
use std::io::Write;
use tracing::{info, warn};

/// Subcommands for managing the record store.
#[derive(Debug, Subcommand)]
pub(crate) enum StoreCommand {
    /// List all recorded runs.
    List {},
    /// Show detailed information about a recorded run.
    Info(InfoOpts),
    /// Prune old recorded runs according to retention policy.
    Prune(PruneOpts),
    /// Export a recorded run as a portable recording.
    Export(ExportOpts),
    /// Export a recorded run as a Chrome Trace Event Format JSON file.
    ///
    /// The output can be loaded into Chrome's chrome://tracing or Perfetto UI
    /// (ui.perfetto.dev) for a timeline view of test parallelism and execution.
    ExportChromeTrace(ExportChromeTraceOpts),
}

/// Common arguments for selecting a run by ID, `latest`, or recording path.
///
/// Used by subcommands that can operate on both on-disk runs and portable
/// recordings. Use `#[command(flatten)]` to embed in a subcommand.
#[derive(Debug, Args)]
pub(crate) struct RunIdOrRecordingArgs {
    /// Run ID, `latest`, or recording path [aliases: -R].
    ///
    /// Accepts "latest" for the most recent completed run, a full UUID or
    /// unambiguous prefix, or a file path (ending in `.zip`, or, on Unix,
    /// `<(curl url)`).
    #[arg(
        value_name = "RUN_ID_OR_RECORDING",
        required_unless_present = "run_id_opt"
    )]
    run_id: Option<RunIdOrRecordingSelector>,

    /// Run ID, `latest`, or recording path (alternative to positional
    /// argument).
    #[arg(
        short = 'R',
        long = "run-id",
        hide = true,
        value_name = "RUN_ID_OR_RECORDING",
        conflicts_with = "run_id"
    )]
    run_id_opt: Option<RunIdOrRecordingSelector>,
}

impl RunIdOrRecordingArgs {
    fn resolved_selector(&self) -> &RunIdOrRecordingSelector {
        self.run_id
            .as_ref()
            .or(self.run_id_opt.as_ref())
            .expect("run_id or run_id_opt is present due to clap validation")
    }
}

/// Options for the `cargo nextest store info` command.
#[derive(Debug, Args)]
pub(crate) struct InfoOpts {
    #[command(flatten)]
    selector: RunIdOrRecordingArgs,
}

impl InfoOpts {
    fn exec_from_store(
        &self,
        run_id_selector: &RunIdSelector,
        state_dir: &Utf8Path,
        styles: &RecordStyles,
        theme_characters: &ThemeCharacters,
        paged_output: &mut PagedOutput,
        redactor: &Redactor,
    ) -> Result<i32> {
        let store =
            RunStore::new(state_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;

        let snapshot = store
            .lock_shared()
            .map_err(|err| ExpectedError::RecordSetupError { err })?
            .into_snapshot();

        let resolved = snapshot
            .resolve_run_id(run_id_selector)
            .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
        let run_id = resolved.run_id;

        // This should never fail since we just resolved the run ID.
        let run = snapshot
            .get_run(run_id)
            .expect("run ID was just resolved, so the run should exist");

        let replayability = run.check_replayability(&snapshot.runs_dir().run_files(run_id));
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

    fn exec_from_archive(
        &self,
        archive_path: &Utf8Path,
        styles: &RecordStyles,
        theme_characters: &ThemeCharacters,
        paged_output: &mut PagedOutput,
        redactor: &Redactor,
    ) -> Result<i32> {
        let archive = PortableRecording::open(archive_path)
            .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

        let run_info = archive.run_info();

        // Create a single-entry index for display purposes.
        let run_id_index = RunIdIndex::new(std::slice::from_ref(&run_info));

        // Check replayability using the archive for file existence checks.
        let replayability = run_info.check_replayability(&archive);

        let display = run_info.display_detailed(
            &run_id_index,
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

/// Options for the `cargo nextest store export` command.
#[derive(Debug, Args)]
pub(crate) struct ExportOpts {
    /// Run ID to export, or `latest` [aliases: -R].
    ///
    /// Accepts "latest" for the most recent completed run, or a full UUID or
    /// unambiguous prefix.
    #[arg(value_name = "RUN_ID", required_unless_present = "run_id_opt")]
    run_id: Option<RunIdSelector>,

    /// Run ID to export (alternative to positional argument).
    #[arg(
        short = 'R',
        long = "run-id",
        hide = true,
        value_name = "RUN_ID",
        conflicts_with = "run_id"
    )]
    run_id_opt: Option<RunIdSelector>,

    /// Destination for the archive file.
    ///
    /// Defaults to `nextest-run-<run-id>.zip` in the current directory.
    #[arg(long, value_name = "PATH", value_parser = zip_extension_path)]
    archive_file: Option<Utf8PathBuf>,
}

/// How tests are grouped in the Chrome trace output.
///
/// CLI counterpart of `ChromeTraceGroupBy` from nextest-runner.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub(crate) enum ChromeTraceGroupByOpt {
    /// Group tests by binary: each binary gets its own process in the trace
    /// viewer, and event names show only the test name.
    #[default]
    Binary,
    /// Group tests by slot: all tests share one process, so each row
    /// represents a slot. Event names include the binary name.
    Slot,
}

impl From<ChromeTraceGroupByOpt> for ChromeTraceGroupBy {
    fn from(opt: ChromeTraceGroupByOpt) -> Self {
        match opt {
            ChromeTraceGroupByOpt::Binary => ChromeTraceGroupBy::Binary,
            ChromeTraceGroupByOpt::Slot => ChromeTraceGroupBy::Slot,
        }
    }
}

/// JSON serialization format for the Chrome trace output.
///
/// CLI counterpart of `ChromeTraceMessageFormat` from nextest-runner.
#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub(crate) enum MessageFormatOpt {
    /// JSON with no whitespace.
    #[default]
    Json,
    /// JSON, prettified.
    JsonPretty,
}

impl From<MessageFormatOpt> for ChromeTraceMessageFormat {
    fn from(opt: MessageFormatOpt) -> Self {
        match opt {
            MessageFormatOpt::Json => ChromeTraceMessageFormat::Json,
            MessageFormatOpt::JsonPretty => ChromeTraceMessageFormat::JsonPretty,
        }
    }
}

/// Options for the `cargo nextest store export-chrome-trace` command.
#[derive(Debug, Args)]
pub(crate) struct ExportChromeTraceOpts {
    #[command(flatten)]
    selector: RunIdOrRecordingArgs,

    /// How to group tests in the trace output.
    #[arg(long, value_enum, default_value_t, value_name = "MODE")]
    group_by: ChromeTraceGroupByOpt,

    /// JSON serialization format for the output.
    #[arg(long, value_enum, default_value_t, value_name = "FORMAT")]
    message_format: MessageFormatOpt,

    /// Output file path. Defaults to stdout.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    output: Option<Utf8PathBuf>,
}

impl ExportChromeTraceOpts {
    fn exec_from_store(
        &self,
        run_id_selector: &RunIdSelector,
        state_dir: &Utf8Path,
        styles: &RecordStyles,
    ) -> Result<i32> {
        let store =
            RunStore::new(state_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;

        let snapshot = store
            .lock_shared()
            .map_err(|err| ExpectedError::RecordSetupError { err })?
            .into_snapshot();

        let resolved = snapshot
            .resolve_run_id(run_id_selector)
            .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
        let run_id = resolved.run_id;

        let run = snapshot
            .get_run(run_id)
            .expect("run ID was just resolved, so the run should exist");

        // Check the store format version before opening the archive. Otherwise,
        // an incompatible run would fail partway through event deserialization
        // with a confusing error.
        if let Err(incompatibility) = run
            .store_format_version
            .check_readable_by(STORE_FORMAT_VERSION)
        {
            return Err(ExpectedError::StoreVersionIncompatible {
                run_id,
                incompatibility,
            });
        }

        if matches!(
            run.status,
            RecordedRunStatus::Incomplete | RecordedRunStatus::Unknown
        ) {
            warn!(
                "run {} is {}: the exported Chrome trace may be incomplete",
                run_id.style(styles.label),
                run.status.short_status_str(),
            );
        }

        let reader = RecordReader::open(&snapshot.runs_dir().run_dir(run_id))
            .map_err(|err| ExpectedError::RecordReadError { err })?;

        let events = reader
            .events()
            .map_err(|err| ExpectedError::RecordReadError { err })?;

        let json_bytes = convert_to_chrome_trace(
            &run.nextest_version,
            events,
            self.group_by.into(),
            self.message_format.into(),
        )
        .map_err(|err| ExpectedError::ChromeTraceExportError { err })?;

        let formatted_run_id = styles.format_run_id(run_id, Some(snapshot.run_id_index()));
        self.write_output(&json_bytes, &formatted_run_id)?;
        Ok(0)
    }

    fn exec_from_archive(&self, archive_path: &Utf8Path, styles: &RecordStyles) -> Result<i32> {
        let mut archive = PortableRecording::open(archive_path)
            .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

        let run_id = archive.run_info().run_id;
        let nextest_version = archive.run_info().nextest_version.clone();

        let run_log = archive
            .read_run_log()
            .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

        let events = run_log
            .events()
            .map_err(|err| ExpectedError::RecordReadError { err })?;

        let json_bytes = convert_to_chrome_trace(
            &nextest_version,
            events,
            self.group_by.into(),
            self.message_format.into(),
        )
        .map_err(|err| ExpectedError::ChromeTraceExportError { err })?;

        let formatted_run_id = styles.format_run_id(run_id, None);
        self.write_output(&json_bytes, &formatted_run_id)?;
        Ok(0)
    }

    fn write_output(&self, json_bytes: &[u8], formatted_run_id: &str) -> Result<()> {
        match &self.output {
            Some(path) => {
                std::fs::write(path, json_bytes)
                    .map_err(|err| ExpectedError::WriteError { err })?;
                info!("wrote Chrome trace for run {formatted_run_id} to {path}");
            }
            None => {
                std::io::stdout()
                    .write_all(json_bytes)
                    .map_err(|err| ExpectedError::WriteError { err })?;
            }
        }
        Ok(())
    }
}

fn zip_extension_path(input: &str) -> Result<Utf8PathBuf, &'static str> {
    let path = Utf8PathBuf::from(input);
    if has_zip_extension(&path) {
        Ok(path)
    } else {
        Err("must end in .zip")
    }
}

impl ExportOpts {
    fn resolved_run_id(&self) -> &RunIdSelector {
        self.run_id
            .as_ref()
            .or(self.run_id_opt.as_ref())
            .expect("run_id or run_id_opt is present due to clap validation")
    }

    fn exec(&self, state_dir: &Utf8Path, styles: &RecordStyles) -> Result<i32> {
        let store =
            RunStore::new(state_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;

        let snapshot = store
            .lock_shared()
            .map_err(|err| ExpectedError::RecordSetupError { err })?
            .into_snapshot();

        let resolved = snapshot
            .resolve_run_id(self.resolved_run_id())
            .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
        let run_id = resolved.run_id;

        let run = snapshot
            .get_run(run_id)
            .expect("run ID was just resolved, so the run should exist");

        // Check the store format version. If the current nextest can't read
        // the run, don't let it produce a portable recording from it.
        if let Err(incompatibility) = run
            .store_format_version
            .check_readable_by(STORE_FORMAT_VERSION)
        {
            return Err(ExpectedError::StoreVersionIncompatible {
                run_id,
                incompatibility,
            });
        }

        if matches!(
            run.status,
            RecordedRunStatus::Incomplete | RecordedRunStatus::Unknown
        ) {
            warn!(
                "run {} is {}: the exported archive may be incomplete or corrupted",
                run_id.style(styles.label),
                run.status.short_status_str(),
            );
        }

        let writer = PortableRecordingWriter::new(run, snapshot.runs_dir())
            .map_err(|err| ExpectedError::PortableRecordingError { err })?;

        let output_path = self
            .archive_file
            .clone()
            .unwrap_or_else(|| Utf8PathBuf::from(writer.default_filename()));

        let result = writer
            .write_to_path(&output_path)
            .map_err(|err| ExpectedError::PortableRecordingError { err })?;

        info!(
            "exported run {} to {} ({} bytes)",
            run_id.style(styles.label),
            result.path.style(styles.label),
            result.size,
        );

        Ok(0)
    }
}

impl PruneOpts {
    fn exec(
        &self,
        state_dir: &Utf8Path,
        record_config: &RecordConfig,
        styles: &RecordStyles,
        paged_output: &mut PagedOutput,
        redactor: &Redactor,
    ) -> Result<i32> {
        let store =
            RunStore::new(state_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;
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
            // Actual prune: output via tracing.
            let mut locked = store
                .lock_exclusive()
                .map_err(|err| ExpectedError::RecordSetupError { err })?;
            let result = locked
                .prune(&policy, PruneKind::Explicit)
                .map_err(|err| ExpectedError::RecordSetupError { err })?;

            info!("{}", result.display(styles));
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
    ) -> Result<i32> {
        let mut styles = RecordStyles::default();
        let mut theme_characters = ThemeCharacters::default();
        if output.color.should_colorize(supports_color::Stream::Stdout) {
            styles.colorize();
        }
        if supports_unicode::on(supports_unicode::Stream::Stdout) {
            theme_characters.use_unicode();
        }

        let (pager_setting, paginate) = early_args.resolve_pager(&user_config.ui);
        let mut paged_output =
            PagedOutput::request_pager(&pager_setting, paginate, &user_config.ui.streampager);

        // Create redactor for snapshot testing if __NEXTEST_REDACT=1.
        let redactor = if crate::output::should_redact() {
            Redactor::for_snapshot_testing()
        } else {
            Redactor::noop()
        };

        // Resolve the workspace state directory lazily, since archive-based
        // commands don't need a workspace.
        let resolve_state_dir = || -> Result<Utf8PathBuf> {
            let workspace_root = locate_workspace_root(manifest_path.as_deref(), output)?;
            records_state_dir(&workspace_root)
                .map_err(|err| ExpectedError::RecordStateDirNotFound { err })
        };

        match self {
            Self::List {} => {
                let state_dir = resolve_state_dir()?;
                let store = RunStore::new(&state_dir)
                    .map_err(|err| ExpectedError::RecordSetupError { err })?;

                let snapshot = store
                    .lock_shared()
                    .map_err(|err| ExpectedError::RecordSetupError { err })?
                    .into_snapshot();

                let store_path = if output.verbose {
                    Some(state_dir.as_path())
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
            Self::Info(opts) => match opts.selector.resolved_selector() {
                RunIdOrRecordingSelector::RecordingPath(path) => opts.exec_from_archive(
                    path,
                    &styles,
                    &theme_characters,
                    &mut paged_output,
                    &redactor,
                ),
                RunIdOrRecordingSelector::RunId(run_id_selector) => {
                    let state_dir = resolve_state_dir()?;
                    opts.exec_from_store(
                        run_id_selector,
                        &state_dir,
                        &styles,
                        &theme_characters,
                        &mut paged_output,
                        &redactor,
                    )
                }
            },
            Self::Prune(opts) => {
                let state_dir = resolve_state_dir()?;
                opts.exec(
                    &state_dir,
                    &user_config.record,
                    &styles,
                    &mut paged_output,
                    &redactor,
                )
            }
            Self::Export(opts) => {
                let state_dir = resolve_state_dir()?;
                opts.exec(&state_dir, &styles)
            }
            Self::ExportChromeTrace(opts) => match opts.selector.resolved_selector() {
                RunIdOrRecordingSelector::RecordingPath(path) => {
                    opts.exec_from_archive(path, &styles)
                }
                RunIdOrRecordingSelector::RunId(run_id_selector) => {
                    let state_dir = resolve_state_dir()?;
                    opts.exec_from_store(run_id_selector, &state_dir, &styles)
                }
            },
        }
    }
}
