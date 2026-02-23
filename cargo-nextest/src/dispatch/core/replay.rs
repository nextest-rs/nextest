// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Replay command options and execution.

use super::run::ReporterCommonOpts;
use crate::{
    ExpectedError, Result,
    cargo_cli::CargoCli,
    dispatch::{EarlyArgs, common::CommonOpts},
    output::OutputContext,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use guppy::{graph::PackageGraph, platform::Platform};
use nextest_metadata::NextestExitCode;
use nextest_runner::{
    errors::{DisplayErrorChain, RecordReadError},
    list::{OwnedTestInstanceId, TestList},
    output_spec::RecordingSpec,
    pager::PagedOutput,
    record::{
        LoadOutput, PortableRecording, RecordReader, RecordedRunInfo, ReplayContext, ReplayHeader,
        ReplayReporterBuilder, RunIdIndex, RunIdOrRecordingSelector, RunStore,
        STORE_FORMAT_VERSION, StoreReader, TestEventKindSummary, TestEventSummary,
        records_state_dir,
    },
    reporter::ReporterOutput,
    user_config::{UserConfig, UserConfigExperimental},
};
use quick_junit::ReportUuid;
use tracing::warn;

/// Options for the replay command.
#[derive(Debug, Args)]
pub(crate) struct ReplayOpts {
    /// Run ID, `latest`, or recording path to replay.
    ///
    /// Accepts "latest" (the default) for the most recent completed run,
    /// a full UUID or unambiguous prefix, or a file path (ending in `.zip`
    /// or containing path separators, e.g. `<(curl url)`).
    #[arg(long, short = 'R', value_name = "RUN_ID_OR_RECORDING", default_value_t)]
    pub(crate) run_id: RunIdOrRecordingSelector,

    /// Exit with the same code as the original run.
    ///
    /// By default, replay exits with code 0 if the replay itself succeeds.
    /// With this flag, replay exits with the code that the original test run
    /// would have returned (e.g., 100 for test failures, 105 for setup script
    /// failures).
    #[arg(long)]
    pub(crate) exit_code: bool,

    /// Simulate no-capture mode during replay.
    ///
    /// This is a convenience flag that sets:
    /// - `--success-output immediate`
    /// - `--failure-output immediate`
    /// - `--no-output-indent`
    ///
    /// These settings produce output similar to running tests with `--no-capture`,
    /// showing all output immediately without indentation.
    ///
    /// Explicit `--success-output` and `--failure-output` flags take precedence
    /// over this setting.
    #[arg(
        long,
        name = "no-capture",
        alias = "nocapture",
        help_heading = "Reporter options"
    )]
    pub(crate) no_capture: bool,

    #[clap(flatten)]
    pub(crate) reporter_opts: ReporterCommonOpts,

    #[clap(flatten)]
    pub(crate) common: CommonOpts,
}

/// Executes the replay command.
pub(crate) fn exec_replay(
    early_args: &EarlyArgs,
    replay_opts: ReplayOpts,
    manifest_path: Option<Utf8PathBuf>,
    output: OutputContext,
) -> Result<i32> {
    // Load user config and check the experimental feature early.
    let host_platform =
        Platform::build_target().expect("nextest is built for a supported platform");
    let user_config =
        UserConfig::for_host_platform(&host_platform, early_args.user_config_location())
            .map_err(|e| ExpectedError::UserConfigError { err: Box::new(e) })?;

    // The replay command requires the record experimental feature to be enabled.
    if !user_config.is_experimental_enabled(UserConfigExperimental::Record) {
        return Err(ExpectedError::ExperimentalFeatureNotEnabled {
            name: "cargo nextest replay",
            var_name: UserConfigExperimental::Record.env_var(),
        });
    }

    // Archive-based replay doesn't require a workspace.
    let run_id_selector = match &replay_opts.run_id {
        RunIdOrRecordingSelector::RecordingPath(archive_path) => {
            return exec_replay_from_archive(
                early_args,
                &replay_opts,
                archive_path,
                &user_config,
                output,
            );
        }
        RunIdOrRecordingSelector::RunId(selector) => selector,
    };

    // Workspace-based replay requires locating the workspace.
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

    let state_dir = records_state_dir(workspace_root)
        .map_err(|err| ExpectedError::RecordStateDirNotFound { err })?;

    let store = RunStore::new(&state_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;
    let snapshot = store
        .lock_shared()
        .map_err(|err| ExpectedError::RecordSetupError { err })?
        .into_snapshot();

    let result = snapshot
        .resolve_run_id(run_id_selector)
        .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
    let run_id = result.run_id;

    let run_info = snapshot
        .get_run(run_id)
        .expect("we just looked up the run ID so the info should be available");

    // Check the store format version before opening the archive.
    if let Err(incompatibility) = run_info
        .store_format_version
        .check_readable_by(STORE_FORMAT_VERSION)
    {
        return Err(ExpectedError::StoreVersionIncompatible {
            run_id,
            incompatibility,
        });
    }

    let run_dir = snapshot.runs_dir().run_dir(run_id);
    let mut reader =
        RecordReader::open(&run_dir).map_err(|err| ExpectedError::RecordReadError { err })?;

    reader
        .load_dictionaries()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    let mut events = reader
        .events()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    run_replay_common(
        early_args,
        &replay_opts,
        &user_config,
        output,
        run_id,
        run_info,
        Some(snapshot.run_id_index()),
        &mut reader,
        &mut events,
    )
}

/// Executes replay from a portable recording.
fn exec_replay_from_archive(
    early_args: &EarlyArgs,
    replay_opts: &ReplayOpts,
    archive_path: &Utf8Path,
    user_config: &UserConfig,
    output: OutputContext,
) -> Result<i32> {
    let mut archive = PortableRecording::open(archive_path)
        .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

    let run_info = archive.run_info();
    let run_id = run_info.run_id;

    let run_log = archive
        .read_run_log()
        .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

    let mut store_reader = archive
        .open_store()
        .map_err(|err| ExpectedError::PortableRecordingReadError { err })?;

    store_reader
        .load_dictionaries()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    let mut events = run_log
        .events()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    run_replay_common(
        early_args,
        replay_opts,
        user_config,
        output,
        run_id,
        &run_info,
        None,
        &mut store_reader,
        &mut events,
    )
}

type EventIter<'a> =
    &'a mut dyn Iterator<Item = Result<TestEventSummary<RecordingSpec>, RecordReadError>>;

/// Common replay logic shared between store-based and archive-based replay.
#[expect(clippy::too_many_arguments)]
fn run_replay_common(
    early_args: &EarlyArgs,
    replay_opts: &ReplayOpts,
    user_config: &UserConfig,
    output: OutputContext,
    run_id: ReportUuid,
    run_info: &RecordedRunInfo,
    run_id_index: Option<&RunIdIndex>,
    store_reader: &mut dyn StoreReader,
    events: EventIter<'_>,
) -> Result<i32> {
    let cargo_metadata_json = store_reader
        .read_cargo_metadata()
        .map_err(|err| ExpectedError::RecordReadError { err })?;
    let graph = PackageGraph::from_json(&cargo_metadata_json)
        .map_err(|err| ExpectedError::cargo_metadata_parse_error(None, err))?;

    let test_list_summary = store_reader
        .read_test_list()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    let record_opts = store_reader
        .read_record_opts()
        .map_err(|err| ExpectedError::RecordReadError { err })?;

    let test_list = TestList::from_summary(&graph, &test_list_summary, record_opts.run_mode)
        .map_err(|err| ExpectedError::TestListFromSummaryError { err })?;

    let mut replay_cx = ReplayContext::new(&test_list);
    for (binary_id, suite) in &test_list_summary.rust_suites {
        for test_name in suite.test_cases.keys() {
            replay_cx.register_test(OwnedTestInstanceId {
                binary_id: binary_id.clone(),
                test_name: test_name.clone(),
            });
        }
    }

    let (pager_setting, paginate) = early_args.resolve_pager(&user_config.ui);
    let mut paged_output =
        PagedOutput::request_pager(&pager_setting, paginate, &user_config.ui.streampager);

    let should_colorize = output.color.should_colorize(supports_color::Stream::Stdout);
    let use_unicode = supports_unicode::on(supports_unicode::Stream::Stdout);

    let mut reporter_builder = ReplayReporterBuilder::new();
    reporter_builder.set_colorize(should_colorize);
    replay_opts.reporter_opts.apply_to_replay_builder(
        &mut reporter_builder,
        &user_config.ui,
        replay_opts.no_capture,
    );
    let mut reporter = reporter_builder.build(
        record_opts.run_mode,
        test_list.run_count(),
        ReporterOutput::Writer {
            writer: &mut paged_output,
            use_unicode,
        },
    );

    // Write the replay header through the reporter.
    let header = ReplayHeader::new(run_id, run_info, run_id_index);
    reporter.write_header(&header)?;

    let output_load_decider = reporter.output_load_decider();

    for event_result in events {
        let event_summary = event_result.map_err(|err| ExpectedError::RecordReadError { err })?;

        let load_output = match &event_summary.kind {
            TestEventKindSummary::Output(output_kind) => {
                output_load_decider.should_load_output(output_kind)
            }
            // Core events have no output to load.
            TestEventKindSummary::Core(_) => LoadOutput::Skip,
        };

        match replay_cx.convert_event(&event_summary, store_reader, load_output) {
            Ok(event) => {
                reporter.write_event(&event)?;
            }
            Err(error) => {
                // Warn about conversion errors, but continue replaying.
                //
                // TODO: we should use reporter.write_error here so that it is
                // displayed in paged output.
                warn!(
                    "error converting replay event: {}",
                    DisplayErrorChain::new(error)
                );
            }
        }
    }

    reporter.finish();

    let exit_code = if replay_opts.exit_code {
        run_info
            .status
            .exit_code()
            .unwrap_or(NextestExitCode::INCOMPLETE_RUN)
    } else {
        NextestExitCode::OK
    };

    Ok(exit_code)
}
