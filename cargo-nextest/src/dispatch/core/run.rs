// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Run and bench command options and execution.

use super::{
    base::BaseApp,
    filter::TestBuildFilter,
    value_enums::{
        FinalStatusLevelOpt, MessageFormat, NoTestsBehaviorOpt, ShowProgressOpt, StatusLevelOpt,
        TestOutputDisplayOpt,
    },
};
use crate::{
    ExpectedError, Result,
    dispatch::helpers::{build_filtersets, final_stats_to_error, resolve_user_config},
    output::OutputWriter,
    reuse_build::ReuseBuildOpts,
};
use clap::{Args, builder::BoolishValueParser};
use nextest_filtering::{FiltersetKind, ParseContext};
use nextest_runner::{
    cargo_config::EnvironmentMap,
    config::{
        core::ConfigExperimental,
        elements::{MaxFail, RetryPolicy, TestThreads},
    },
    helpers::plural,
    input::InputHandlerKind,
    list::{BinaryList, TestExecuteContext, TestList},
    record::{
        ComputedRerunInfo, RecordOpts, RecordReader, RecordRetentionPolicy, RecordSession,
        RecordSessionConfig, RunIdSelector, RunStore, Styles as RecordStyles,
        format::{RECORD_FORMAT_VERSION, RerunRootInfo},
        records_cache_dir,
    },
    reporter::{
        FinalStatusLevel, MaxProgressRunning, ReporterBuilder, ShowTerminalProgress, StatusLevel,
        TestOutputDisplay,
        events::{FinalRunStats, RunStats},
        structured,
    },
    run_mode::NextestRunMode,
    runner::{
        DebuggerCommand, Interceptor, StressCondition, StressCount, TestRunnerBuilder,
        TracerCommand, configure_handle_inheritance,
    },
    signal::SignalHandlerKind,
    test_filter::TestFilterBuilder,
    test_output::CaptureStrategy,
    user_config::{UserConfigExperimental, elements::UiConfig},
};
use quick_junit::ReportUuid;
use std::{collections::BTreeMap, io::IsTerminal, sync::Arc, time::Duration};
use tracing::{debug, info, warn};

/// Options for the run command.
#[derive(Debug, Args)]
pub(crate) struct RunOpts {
    #[clap(flatten)]
    pub(crate) cargo_options: crate::cargo_cli::CargoOptions,

    /// Rerun tests that failed or didn't complete in a previous recorded run.
    ///
    /// Accepts a run ID (full UUID, short prefix like "abc1234", or "latest").
    /// Only tests that didn't pass in the specified run will be executed.
    ///
    /// New tests (not in the parent run) are also included by default.
    #[arg(
        long,
        short = 'R',
        value_name = "RUN_ID",
        alias = "run-id",
        help_heading = "Filter options"
    )]
    pub(crate) rerun: Option<RunIdSelector>,

    #[clap(flatten)]
    pub(crate) build_filter: TestBuildFilter,

    #[clap(flatten)]
    pub(crate) runner_opts: TestRunnerOpts,

    /// Run tests serially and do not capture output.
    #[arg(
        long,
        name = "no-capture",
        alias = "nocapture",
        help_heading = "Runner options",
        display_order = 100
    )]
    pub(crate) no_capture: bool,

    #[clap(flatten)]
    pub(crate) reporter_opts: ReporterOpts,

    #[clap(flatten)]
    pub(crate) reuse_build: ReuseBuildOpts,
}

/// Options for the bench command.
#[derive(Debug, Args)]
pub(crate) struct BenchOpts {
    #[clap(flatten)]
    pub(crate) cargo_options: crate::cargo_cli::CargoOptions,

    #[clap(flatten)]
    pub(crate) build_filter: TestBuildFilter,

    #[clap(flatten)]
    pub(crate) runner_opts: BenchRunnerOpts,

    /// Run benchmarks serially and do not capture output (always enabled).
    ///
    /// Benchmarks in nextest always run serially, so this flag is kept only for
    /// compatibility and has no effect.
    #[arg(
        long,
        name = "no-capture",
        alias = "nocapture",
        help_heading = "Runner options",
        display_order = 100
    )]
    pub(crate) no_capture: bool,

    #[clap(flatten)]
    pub(crate) reporter_opts: BenchReporterOpts,
    // Note: no reuse_build for benchmarks since archive extraction is not supported.
}

/// Test runner options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Runner options")]
pub struct TestRunnerOpts {
    /// Compile, but don't run tests.
    #[arg(long, name = "no-run")]
    pub(crate) no_run: bool,

    /// Number of tests to run simultaneously [possible values: integer or "num-cpus"]
    /// [default: from profile].
    #[arg(
        long,
        short = 'j',
        visible_alias = "jobs",
        value_name = "N",
        env = "NEXTEST_TEST_THREADS",
        allow_negative_numbers = true
    )]
    test_threads: Option<TestThreads>,

    /// Number of retries for failing tests [default: from profile].
    #[arg(long, env = "NEXTEST_RETRIES", value_name = "N")]
    retries: Option<u32>,

    /// Cancel test run on the first failure.
    #[arg(
        long,
        visible_alias = "ff",
        name = "fail-fast",
        // TODO: It would be nice to warn rather than error if fail-fast is used
        // with no-run, so that this matches the other options like
        // test-threads. But there seem to be issues with that: clap 4.5 doesn't
        // appear to like `Option<bool>` very much. With `ArgAction::SetTrue` it
        // always sets the value to false or true rather than leaving it unset.
        conflicts_with = "no-run"
    )]
    fail_fast: bool,

    /// Run all tests regardless of failure.
    #[arg(
        long,
        visible_alias = "nff",
        name = "no-fail-fast",
        conflicts_with = "no-run",
        overrides_with = "fail-fast"
    )]
    no_fail_fast: bool,

    /// Number of tests that can fail before exiting test run.
    ///
    /// To control whether currently running tests are waited for or terminated
    /// immediately, append ':wait' (default) or ':immediate' to the number
    /// (e.g., '5:immediate').
    ///
    /// [possible values: integer, "all", "N:wait", "N:immediate"]
    #[arg(
        long,
        name = "max-fail",
        value_name = "N[:MODE]",
        conflicts_with_all = &["no-run", "fail-fast", "no-fail-fast"],
    )]
    max_fail: Option<MaxFail>,

    /// Interceptor options (debugger or tracer).
    #[clap(flatten)]
    pub(crate) interceptor: InterceptorOpt,

    /// Behavior if there are no tests to run [default: auto].
    #[arg(long, value_enum, value_name = "ACTION", env = "NEXTEST_NO_TESTS")]
    pub(crate) no_tests: Option<NoTestsBehaviorOpt>,

    /// Stress testing options.
    #[clap(flatten)]
    pub(crate) stress: StressOptions,
}

impl TestRunnerOpts {
    pub(crate) fn to_builder(&self, cap_strat: CaptureStrategy) -> Option<TestRunnerBuilder> {
        // Warn on conflicts between options. This is a warning and not an error
        // because these options can be specified via environment variables as
        // well.
        if self.test_threads.is_some()
            && let Some(reasons) =
                no_run_no_capture_reasons(self.no_run, cap_strat == CaptureStrategy::None)
        {
            warn!("ignoring --test-threads because {reasons}");
        }

        if self.retries.is_some() && self.no_run {
            warn!("ignoring --retries because --no-run is specified");
        }
        if self.no_tests.is_some() && self.no_run {
            warn!("ignoring --no-tests because --no-run is specified");
        }

        // ---

        if self.no_run {
            return None;
        }

        let mut builder = TestRunnerBuilder::default();
        builder.set_capture_strategy(cap_strat);
        if let Some(retries) = self.retries {
            builder.set_retries(RetryPolicy::new_without_delay(retries));
        }

        if let Some(max_fail) = self.max_fail {
            builder.set_max_fail(max_fail);
            debug!(max_fail = ?max_fail, "set max fail");
        } else if self.no_fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(false));
            debug!("set max fail via from_fail_fast(false)");
        } else if self.fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(true));
            debug!("set max fail via from_fail_fast(true)");
        }

        if let Some(test_threads) = self.test_threads {
            builder.set_test_threads(test_threads);
        }

        if let Some(condition) = self.stress.condition.as_ref() {
            builder.set_stress_condition(condition.stress_condition());
        }

        builder.set_interceptor(self.interceptor.to_interceptor());

        Some(builder)
    }
}

/// Benchmark runner options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Runner options")]
pub(crate) struct BenchRunnerOpts {
    /// Compile, but don't run benchmarks.
    #[arg(long, name = "no-run")]
    pub(crate) no_run: bool,

    /// Cancel benchmark run on the first failure.
    #[arg(
        long,
        visible_alias = "ff",
        name = "fail-fast",
        // TODO: It would be nice to warn rather than error if fail-fast is used
        // with no-run, so that this matches the other options like
        // test-threads. But there seem to be issues with that: clap 4.5 doesn't
        // appear to like `Option<bool>` very much. With `ArgAction::SetTrue` it
        // always sets the value to false or true rather than leaving it unset.
        conflicts_with = "no-run"
    )]
    fail_fast: bool,

    /// Run all benchmarks regardless of failure.
    #[arg(
        long,
        visible_alias = "nff",
        name = "no-fail-fast",
        conflicts_with = "no-run",
        overrides_with = "fail-fast"
    )]
    no_fail_fast: bool,

    /// Number of benchmarks that can fail before exiting run [possible
    /// values: integer or "all"].
    #[arg(
        long,
        name = "max-fail",
        value_name = "N",
        conflicts_with_all = &["no-run", "fail-fast", "no-fail-fast"],
    )]
    max_fail: Option<MaxFail>,

    /// Behavior if there are no benchmarks to run [default: auto].
    #[arg(long, value_enum, value_name = "ACTION", env = "NEXTEST_NO_TESTS")]
    pub(crate) no_tests: Option<NoTestsBehaviorOpt>,

    /// Stress testing options.
    #[clap(flatten)]
    pub(crate) stress: StressOptions,

    #[clap(flatten)]
    pub(crate) interceptor: InterceptorOpt,
}

impl BenchRunnerOpts {
    pub(crate) fn to_builder(&self, cap_strat: CaptureStrategy) -> Option<TestRunnerBuilder> {
        if self.no_tests.is_some() && self.no_run {
            warn!("ignoring --no-tests because --no-run is specified");
        }

        // ---

        if self.no_run {
            return None;
        }
        let mut builder = TestRunnerBuilder::default();
        builder.set_capture_strategy(cap_strat);
        if let Some(max_fail) = self.max_fail {
            builder.set_max_fail(max_fail);
            debug!(max_fail = ?max_fail, "set max fail");
        } else if self.no_fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(false));
            debug!("set max fail via from_fail_fast(false)");
        } else if self.fail_fast {
            builder.set_max_fail(MaxFail::from_fail_fast(true));
            debug!("set max fail via from_fail_fast(true)");
        }

        // Benchmarks always run serially and use 1 test thread.
        builder.set_test_threads(TestThreads::Count(1));

        if let Some(condition) = self.stress.condition.as_ref() {
            builder.set_stress_condition(condition.stress_condition());
        }

        builder.set_interceptor(self.interceptor.to_interceptor());

        Some(builder)
    }
}

/// Interceptor options (debugger or tracer).
#[derive(Debug, Default, Args)]
#[group(id = "interceptor", multiple = false)]
pub(crate) struct InterceptorOpt {
    /// Debug a single test using a text-based or graphical debugger.
    ///
    /// Debugger mode automatically:
    ///
    /// - disables timeouts
    /// - disables output capture
    /// - passes standard input through to the debugger
    ///
    /// Example: `--debugger "rust-gdb --args"`
    #[arg(long, value_name = "DEBUGGER", conflicts_with_all = ["stress_condition", "no-run"])]
    pub(crate) debugger: Option<DebuggerCommand>,

    /// Trace a single test using a syscall tracer like `strace` or `truss`.
    ///
    /// Tracer mode automatically:
    ///
    /// - disables timeouts
    /// - disables output capture
    ///
    /// Unlike `--debugger`, tracers do not need stdin passthrough or special signal handling.
    ///
    /// Example: `--tracer "strace -tt"`
    #[arg(long, value_name = "TRACER", conflicts_with_all = ["stress_condition", "no-run"])]
    pub(crate) tracer: Option<TracerCommand>,
}

impl InterceptorOpt {
    /// Returns true if either a debugger or a tracer is active.
    pub(crate) fn is_active(&self) -> bool {
        self.debugger.is_some() || self.tracer.is_some()
    }

    /// Converts to an [`Interceptor`] enum.
    pub(crate) fn to_interceptor(&self) -> Interceptor {
        match (&self.debugger, &self.tracer) {
            (Some(debugger), None) => Interceptor::Debugger(debugger.clone()),
            (None, Some(tracer)) => Interceptor::Tracer(tracer.clone()),
            (None, None) => Interceptor::None,
            (Some(_), Some(_)) => {
                unreachable!("clap group ensures debugger and tracer are mutually exclusive")
            }
        }
    }
}

/// Stress testing options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Stress testing options")]
pub(crate) struct StressOptions {
    /// Stress testing condition.
    #[clap(flatten)]
    pub(crate) condition: Option<StressConditionOpt>,
    // TODO: modes other than serial.
}

/// Stress condition options.
#[derive(Clone, Debug, Default, Args)]
#[group(id = "stress_condition", multiple = false)]
pub(crate) struct StressConditionOpt {
    /// The number of times to run each test, or `infinite` to run indefinitely.
    #[arg(long, value_name = "COUNT")]
    stress_count: Option<StressCount>,

    /// How long to run stress tests until (e.g. 24h).
    #[arg(long, value_name = "DURATION", value_parser = non_zero_duration)]
    stress_duration: Option<Duration>,
}

impl StressConditionOpt {
    fn stress_condition(&self) -> StressCondition {
        if let Some(count) = self.stress_count {
            StressCondition::Count(count)
        } else if let Some(duration) = self.stress_duration {
            StressCondition::Duration(duration)
        } else {
            unreachable!(
                "if StressOptions::condition is Some, \
                 one of these should be set"
            )
        }
    }
}

fn non_zero_duration(input: &str) -> std::result::Result<Duration, String> {
    let duration = humantime::parse_duration(input).map_err(|error| error.to_string())?;
    if duration.is_zero() {
        Err("duration must be non-zero".to_string())
    } else {
        Ok(duration)
    }
}

fn no_run_no_capture_reasons(no_run: bool, no_capture: bool) -> Option<&'static str> {
    match (no_run, no_capture) {
        (true, true) => Some("--no-run and --no-capture are specified"),
        (true, false) => Some("--no-run is specified"),
        (false, true) => Some("--no-capture is specified"),
        (false, false) => None,
    }
}

/// Common reporter options shared between `run` and `show` (replay) commands.
///
/// These options control how test output is displayed and can be used both
/// during live test runs and when replaying recorded runs.
#[derive(Debug, Default, Args)]
pub(crate) struct ReporterCommonOpts {
    /// Output stdout and stderr on failure.
    #[arg(long, value_enum, value_name = "WHEN", env = "NEXTEST_FAILURE_OUTPUT")]
    pub(crate) failure_output: Option<TestOutputDisplayOpt>,

    /// Output stdout and stderr on success.
    #[arg(long, value_enum, value_name = "WHEN", env = "NEXTEST_SUCCESS_OUTPUT")]
    pub(crate) success_output: Option<TestOutputDisplayOpt>,

    // status_level does not conflict with --no-capture because pass vs skip still makes sense.
    /// Test statuses to output.
    #[arg(long, value_enum, value_name = "LEVEL", env = "NEXTEST_STATUS_LEVEL")]
    pub(crate) status_level: Option<StatusLevelOpt>,

    /// Test statuses to output at the end of the run.
    #[arg(
        long,
        value_enum,
        value_name = "LEVEL",
        env = "NEXTEST_FINAL_STATUS_LEVEL"
    )]
    pub(crate) final_status_level: Option<FinalStatusLevelOpt>,

    /// Do not indent captured test output.
    ///
    /// By default, test output produced by **--failure-output** and
    /// **--success-output** is indented for visual clarity. This flag disables
    /// that behavior.
    ///
    /// This option has no effect with **--no-capture**, since that passes
    /// through standard output and standard error.
    #[arg(long, env = "NEXTEST_NO_OUTPUT_INDENT", value_parser = BoolishValueParser::new())]
    pub(crate) no_output_indent: bool,
}

impl ReporterCommonOpts {
    /// Applies these common options to a reporter builder.
    ///
    /// The `no_output_indent` parameter from `resolved_ui` is combined with the
    /// CLI option (CLI takes precedence if set).
    pub(crate) fn apply_to_builder(&self, builder: &mut ReporterBuilder, resolved_ui: &UiConfig) {
        // Note: CLI uses --no-output-indent (negative), resolved config uses
        // output_indent (positive).
        let no_output_indent = self.no_output_indent || !resolved_ui.output_indent;

        if let Some(failure_output) = self.failure_output {
            builder.set_failure_output(failure_output.into());
        }
        if let Some(success_output) = self.success_output {
            builder.set_success_output(success_output.into());
        }
        if let Some(status_level) = self.status_level {
            builder.set_status_level(status_level.into());
        }
        if let Some(final_status_level) = self.final_status_level {
            builder.set_final_status_level(final_status_level.into());
        }
        builder.set_no_output_indent(no_output_indent);
    }

    /// Applies these common options to a replay reporter builder.
    ///
    /// The `no_output_indent` parameter from `resolved_ui` is combined with the
    /// CLI option (CLI takes precedence if set).
    ///
    /// If `no_capture` is true, it simulates no-capture mode by setting:
    /// - `failure_output` to `Immediate` (if not explicitly set)
    /// - `success_output` to `Immediate` (if not explicitly set)
    /// - `no_output_indent` to `true`
    pub(crate) fn apply_to_replay_builder(
        &self,
        builder: &mut nextest_runner::record::ReplayReporterBuilder,
        resolved_ui: &UiConfig,
        no_capture: bool,
    ) {
        // Note: CLI uses --no-output-indent (negative), resolved config uses
        // output_indent (positive). --no-capture also implies no indentation.
        let no_output_indent = self.no_output_indent || no_capture || !resolved_ui.output_indent;

        // Apply failure_output: explicit CLI > no_capture default > builder default.
        if let Some(failure_output) = self.failure_output {
            builder.set_failure_output(failure_output.into());
        } else if no_capture {
            builder.set_failure_output(TestOutputDisplay::Immediate);
        }

        // Apply success_output: explicit CLI > no_capture default > builder default.
        if let Some(success_output) = self.success_output {
            builder.set_success_output(success_output.into());
        } else if no_capture {
            builder.set_success_output(TestOutputDisplay::Immediate);
        }

        if let Some(status_level) = self.status_level {
            builder.set_status_level(status_level.into());
        }
        if let Some(final_status_level) = self.final_status_level {
            builder.set_final_status_level(final_status_level.into());
        }
        builder.set_no_output_indent(no_output_indent);
    }
}

/// Reporter options for the run command.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Reporter options")]
pub(crate) struct ReporterOpts {
    #[command(flatten)]
    pub(crate) common: ReporterCommonOpts,

    /// Show nextest progress in the specified manner.
    ///
    /// This can also be set via user config at `~/.config/nextest/config.toml`.
    /// See <https://nexte.st/docs/user-config>.
    #[arg(long, env = "NEXTEST_SHOW_PROGRESS")]
    show_progress: Option<ShowProgressOpt>,

    /// Do not display the progress bar. Deprecated, use **--show-progress** instead.
    #[arg(long, env = "NEXTEST_HIDE_PROGRESS_BAR", value_parser = BoolishValueParser::new())]
    hide_progress_bar: bool,

    /// Disable handling of input keys from the terminal.
    ///
    /// By default, when running a terminal, nextest accepts the `t` key to dump
    /// test information. This flag disables that behavior.
    #[arg(long, env = "NEXTEST_NO_INPUT_HANDLER", value_parser = BoolishValueParser::new())]
    pub(crate) no_input_handler: bool,

    /// Maximum number of running tests to display progress for.
    ///
    /// When more tests are running than this limit, the progress bar will show
    /// the first N tests and a summary of remaining tests (e.g. "... and 24
    /// more tests running"). Set to **0** to hide running tests, or
    /// **infinite** for unlimited. This applies when using
    /// `--show-progress=bar` or `--show-progress=only`.
    ///
    /// This can also be set via user config at `~/.config/nextest/config.toml`.
    /// See <https://nexte.st/docs/user-config>.
    #[arg(
        long = "max-progress-running",
        value_name = "N",
        env = "NEXTEST_MAX_PROGRESS_RUNNING"
    )]
    max_progress_running: Option<MaxProgressRunning>,

    /// Format to use for test results (experimental).
    #[arg(
        long,
        name = "message-format",
        value_enum,
        value_name = "FORMAT",
        env = "NEXTEST_MESSAGE_FORMAT"
    )]
    pub(crate) message_format: Option<MessageFormat>,

    /// Version of structured message-format to use (experimental).
    ///
    /// This allows the machine-readable formats to use a stable structure for consistent
    /// consumption across changes to nextest. If not specified, the latest version is used.
    #[arg(
        long,
        requires = "message-format",
        value_name = "VERSION",
        env = "NEXTEST_MESSAGE_FORMAT_VERSION"
    )]
    pub(crate) message_format_version: Option<String>,
}

impl ReporterOpts {
    pub(crate) fn to_builder(
        &self,
        no_run: bool,
        no_capture: bool,
        should_colorize: bool,
        resolved_ui: &UiConfig,
    ) -> ReporterBuilder {
        // Warn on conflicts between options. This is a warning and not an error
        // because these options can be specified via environment variables as
        // well.
        if no_run && no_capture {
            warn!("ignoring --no-capture because --no-run is specified");
        }

        let reasons = no_run_no_capture_reasons(no_run, no_capture);

        if self.common.failure_output.is_some()
            && let Some(reasons) = reasons
        {
            warn!("ignoring --failure-output because {}", reasons);
        }
        if self.common.success_output.is_some()
            && let Some(reasons) = reasons
        {
            warn!("ignoring --success-output because {}", reasons);
        }
        if self.common.status_level.is_some() && no_run {
            warn!("ignoring --status-level because --no-run is specified");
        }
        if self.common.final_status_level.is_some() && no_run {
            warn!("ignoring --final-status-level because --no-run is specified");
        }
        if self.message_format.is_some() && no_run {
            warn!("ignoring --message-format because --no-run is specified");
        }
        if self.message_format_version.is_some() && no_run {
            warn!("ignoring --message-format-version because --no-run is specified");
        }

        // Determine show_progress with precedence: CLI/env > resolved config.
        // Use UiShowProgress to preserve the "only" variant's special behavior.
        let ui_show_progress = match (self.show_progress, self.hide_progress_bar) {
            (Some(show_progress), true) => {
                warn!("ignoring --hide-progress-bar because --show-progress is specified");
                show_progress.into()
            }
            (Some(show_progress), false) => show_progress.into(),
            (None, true) => nextest_runner::user_config::elements::UiShowProgress::None,
            (None, false) => resolved_ui.show_progress,
        };

        // Determine max_progress_running with precedence: CLI/env > resolved config.
        let max_progress_running = self
            .max_progress_running
            .unwrap_or(resolved_ui.max_progress_running);

        // Note: CLI uses --no-output-indent (negative), resolved config uses
        // output_indent (positive).
        let no_output_indent = self.common.no_output_indent || !resolved_ui.output_indent;

        debug!(
            ?ui_show_progress,
            ?max_progress_running,
            ?no_output_indent,
            "resolved reporter UI settings"
        );

        // ---

        let mut builder = ReporterBuilder::default();
        builder.set_no_capture(no_capture);
        builder.set_colorize(should_colorize);

        if ui_show_progress == nextest_runner::user_config::elements::UiShowProgress::Only {
            // "only" implies --status-level=slow and --final-status-level=none.
            // But we allow overriding these options explicitly as well.
            builder.set_status_level(StatusLevel::Slow);
            builder.set_final_status_level(FinalStatusLevel::None);
        }

        // Apply the common display options (failure_output, success_output,
        // status_level, final_status_level, no_output_indent). These can
        // override the "only" defaults set above.
        self.common.apply_to_builder(&mut builder, resolved_ui);

        builder.set_show_progress(ui_show_progress.into());
        builder.set_max_progress_running(max_progress_running);
        builder
    }
}

/// Benchmark reporter options.
#[derive(Debug, Default, Args)]
#[command(next_help_heading = "Reporter options")]
pub(crate) struct BenchReporterOpts {
    /// Show nextest progress in the specified manner.
    ///
    /// For benchmarks, the default is "counter" which shows the benchmark index
    /// (e.g., "(1/10)") but no progress bar.
    ///
    /// This can also be set via user config at `~/.config/nextest/config.toml`.
    /// See <https://nexte.st/docs/user-config>.
    #[arg(long, env = "NEXTEST_SHOW_PROGRESS")]
    show_progress: Option<ShowProgressOpt>,

    /// Disable handling of input keys from the terminal.
    ///
    /// By default, when running a terminal, nextest accepts the `t` key to dump
    /// test information. This flag disables that behavior.
    #[arg(long, env = "NEXTEST_NO_INPUT_HANDLER", value_parser = BoolishValueParser::new())]
    pub(crate) no_input_handler: bool,
}

impl BenchReporterOpts {
    pub(crate) fn to_builder(
        &self,
        should_colorize: bool,
        resolved_ui: &UiConfig,
    ) -> ReporterBuilder {
        let mut builder = ReporterBuilder::default();
        builder.set_no_capture(true);
        builder.set_colorize(should_colorize);
        // Determine show_progress with precedence: CLI/env > resolved config.
        let ui_show_progress = self
            .show_progress
            .map(nextest_runner::user_config::elements::UiShowProgress::from)
            .unwrap_or(resolved_ui.show_progress);
        if ui_show_progress == nextest_runner::user_config::elements::UiShowProgress::Only {
            // "only" implies --status-level=slow and --final-status-level=none.
            builder.set_status_level(StatusLevel::Slow);
            builder.set_final_status_level(FinalStatusLevel::None);
        }
        builder.set_show_progress(ui_show_progress.into());
        builder
    }
}

// (_output is not used, but must be passed in to ensure that the output is properly initialized
// before calling this method)
fn check_experimental_filtering(_output: crate::output::OutputContext) {
    const EXPERIMENTAL_ENV: &str = "NEXTEST_EXPERIMENTAL_FILTER_EXPR";
    if std::env::var(EXPERIMENTAL_ENV).is_ok() {
        warn!(
            "filtersets are no longer experimental: NEXTEST_EXPERIMENTAL_FILTER_EXPR does not need to be set"
        );
    }
}

/// Captures environment variables that affect nextest behavior (`NEXTEST_*` and `CARGO_*`).
///
/// Excludes variables ending with `_TOKEN` to avoid capturing sensitive tokens.
fn capture_env_vars_for_recording() -> BTreeMap<String, String> {
    filter_env_vars_for_recording(std::env::vars())
}

/// Filters environment variables for recording.
///
/// Includes only `NEXTEST_*` and `CARGO_*` variables, excluding those ending
/// with `_TOKEN` to avoid capturing sensitive tokens.
pub(super) fn filter_env_vars_for_recording(
    vars: impl Iterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    vars.filter(|(key, _)| {
        (key.starts_with("NEXTEST_") || key.starts_with("CARGO_")) && !key.ends_with("_TOKEN")
    })
    .collect()
}

/// Application for running tests (run/list/bench).
pub(crate) struct App {
    pub(crate) base: BaseApp,
    pub(crate) build_filter: TestBuildFilter,
}

impl App {
    pub(crate) fn new(base: BaseApp, build_filter: TestBuildFilter) -> Result<Self> {
        check_experimental_filtering(base.output);

        Ok(Self { base, build_filter })
    }

    pub(crate) fn build_test_list(
        &self,
        ctx: &TestExecuteContext<'_>,
        binary_list: Arc<BinaryList>,
        test_filter_builder: &TestFilterBuilder,
        profile: &nextest_runner::config::core::EvaluatableProfile<'_>,
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

    pub(crate) fn exec_run(
        &self,
        no_capture: bool,
        rerun: Option<&RunIdSelector>,
        runner_opts: &TestRunnerOpts,
        reporter_opts: &ReporterOpts,
        cli_args: Vec<String>,
        output_writer: &mut OutputWriter,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());
        let (version_only_config, config) = self
            .base
            .load_config(&pcx, &std::collections::BTreeSet::new())?;
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
        let resolved_user_config = resolve_user_config(
            &self.base.build_platforms.host.platform,
            self.base.early_args.user_config_location(),
        )?;

        // The -R/--rerun option requires the record experimental feature to be enabled.
        if rerun.is_some()
            && !resolved_user_config.is_experimental_enabled(UserConfigExperimental::Record)
        {
            return Err(ExpectedError::ExperimentalFeatureNotEnabled {
                name: "rerunning tests (-R/--rerun)",
                var_name: UserConfigExperimental::Record.env_var(),
            });
        }

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
        let mut test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;
        let (rerun_state, expected_outstanding) = if let Some(run_id_selector) = rerun {
            let (rerun_state, outstanding_tests) = self.resolve_rerun(run_id_selector)?;
            let expected = outstanding_tests.expected_test_ids();
            test_filter_builder.set_outstanding_tests(outstanding_tests);
            (Some(rerun_state), Some(expected))
        } else {
            (None, None)
        };

        // Start running Cargo commands at this point, once all initial
        // validation is complete.
        let rerun_build_scope = rerun_state
            .as_ref()
            .map(|s| s.root_info.build_scope_args.as_slice());
        let binary_list = self
            .base
            .build_binary_list_with_rerun("test", rerun_build_scope)?;
        let build_platforms = &binary_list.rust_build_meta.build_platforms.clone();
        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(build_platforms);

        let profile = profile.apply_build_platforms(build_platforms);
        let ctx = TestExecuteContext {
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, &test_filter_builder, &profile)?;

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
                InputHandlerKind::Noop
            } else if resolved_user_config.ui.input_handler {
                InputHandlerKind::Standard
            } else {
                InputHandlerKind::Noop
            };

        let Some(mut runner_builder) = runner_builder else {
            return Ok(());
        };

        // Set expected outstanding tests for rerun tracking.
        if let Some(expected) = expected_outstanding {
            runner_builder.set_expected_outstanding(expected);
        }

        // Save cli_args for recording before moving them to the runner.
        let cli_args_for_recording = cli_args.clone();
        let runner = runner_builder.build(
            &test_list,
            &profile,
            cli_args,
            signal_handler,
            input_handler,
            double_spawn.clone(),
            target_runner.clone(),
        )?;

        // Set up recording if the experimental feature is enabled (via env var or user config)
        // AND recording is enabled in the config.
        let (recording_session, run_id_unique_prefix) = if resolved_user_config
            .is_experimental_enabled(UserConfigExperimental::Record)
            && resolved_user_config.record.enabled
        {
            let env_vars_for_recording = capture_env_vars_for_recording();

            let outstanding_tests = test_filter_builder.into_rerun_info();
            let rerun_info = if let Some(outstanding) = outstanding_tests {
                let rerun_state =
                    rerun_state.expect("rerun_state is Some iff outstanding_tests is Some");
                Some(outstanding.into_rerun_info(rerun_state.parent_run_id, rerun_state.root_info))
            } else {
                None
            };

            let config = RecordSessionConfig {
                workspace_root: &self.base.workspace_root,
                run_id: runner.run_id(),
                nextest_version: self.base.current_version.clone(),
                started_at: runner.started_at().fixed_offset(),
                cli_args: cli_args_for_recording,
                build_scope_args: self.base.build_scope_args(),
                env_vars: env_vars_for_recording,
                max_output_size: resolved_user_config.record.max_output_size,
                rerun_info,
            };
            match RecordSession::setup(config) {
                Ok(setup) => {
                    let record = structured::RecordReporter::new(setup.recorder);
                    let opts = RecordOpts::new(test_list.mode());
                    record.write_meta(
                        self.base.cargo_metadata_json.clone(),
                        test_list.to_summary(),
                        opts,
                    );
                    structured_reporter.set_record(record);
                    (Some(setup.session), Some(setup.run_id_unique_prefix))
                }
                Err(err) => match err.disabled_error() {
                    Some(reason) => {
                        // Recording is disabled due to a format version mismatch.
                        // Log a warning and continue without recording.
                        warn!("recording disabled: {reason}");
                        (None, None)
                    }
                    None => return Err(ExpectedError::RecordSessionSetupError { err }),
                },
            }
        } else {
            (None, None)
        };

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

        // Set the run ID unique prefix for highlighting if a recording session is active.
        if let Some(prefix) = run_id_unique_prefix {
            reporter.set_run_id_unique_prefix(prefix);
        }

        configure_handle_inheritance(no_capture)?;
        let run_stats = runner.try_execute(|event| reporter.report_event(event))?;
        let reporter_stats = reporter.finish();

        let outstanding_not_seen_count = reporter_stats
            .run_finished
            .and_then(|rf| rf.outstanding_not_seen_count);
        let rerun_available = recording_session.is_some();
        let result = final_result(
            NextestRunMode::Test,
            run_stats,
            runner_opts.no_tests,
            outstanding_not_seen_count,
            rerun_available,
        );

        let exit_code = result.as_ref().err().map_or(0, |e| e.process_exit_code());

        if let Some(session) = recording_session {
            let policy = RecordRetentionPolicy::from(&resolved_user_config.record);
            let mut styles = RecordStyles::default();
            if should_colorize {
                styles.colorize();
            }
            session
                .finalize(
                    reporter_stats.recording_sizes,
                    reporter_stats.run_finished,
                    exit_code,
                    &policy,
                )
                .log(&styles);
        }
        self.base
            .check_version_config_final(version_only_config.nextest_version())?;

        result
    }

    pub(crate) fn exec_bench(
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
        let mut structured_reporter = structured::StructuredReporter::new();
        // TODO: support message format for benchmarks.
        // TODO: maybe support capture strategy for benchmarks?
        let cap_strat = CaptureStrategy::None;

        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stderr);

        // Load and resolve user config with platform-specific overrides.
        let resolved_user_config = resolve_user_config(
            &self.base.build_platforms.host.platform,
            self.base.early_args.user_config_location(),
        )?;

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

        let test_list = self.build_test_list(&ctx, binary_list, &test_filter_builder, &profile)?;

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
                InputHandlerKind::Noop
            } else if resolved_user_config.ui.input_handler {
                InputHandlerKind::Standard
            } else {
                InputHandlerKind::Noop
            };

        let Some(runner_builder) = runner_builder else {
            return Ok(());
        };
        // Save cli_args for recording before moving them to the runner.
        let cli_args_for_recording = cli_args.clone();
        let runner = runner_builder.build(
            &test_list,
            &profile,
            cli_args,
            signal_handler,
            input_handler,
            double_spawn.clone(),
            target_runner.clone(),
        )?;

        // Set up recording if the experimental feature is enabled AND recording is enabled in
        // the config.
        let (recording_session, run_id_unique_prefix) = if resolved_user_config
            .is_experimental_enabled(UserConfigExperimental::Record)
            && resolved_user_config.record.enabled
        {
            let env_vars_for_recording = capture_env_vars_for_recording();
            let config = RecordSessionConfig {
                workspace_root: &self.base.workspace_root,
                run_id: runner.run_id(),
                nextest_version: self.base.current_version.clone(),
                started_at: runner.started_at().fixed_offset(),
                cli_args: cli_args_for_recording,
                build_scope_args: self.base.build_scope_args(),
                env_vars: env_vars_for_recording,
                max_output_size: resolved_user_config.record.max_output_size,
                // TODO: support reruns? value seems dubious.
                rerun_info: None,
            };
            match RecordSession::setup(config) {
                Ok(setup) => {
                    let record = structured::RecordReporter::new(setup.recorder);
                    let opts = RecordOpts::new(test_list.mode());
                    record.write_meta(
                        self.base.cargo_metadata_json.clone(),
                        test_list.to_summary(),
                        opts,
                    );
                    structured_reporter.set_record(record);
                    (Some(setup.session), Some(setup.run_id_unique_prefix))
                }
                Err(err) => match err.disabled_error() {
                    Some(reason) => {
                        // Recording is disabled due to a format version mismatch.
                        // Log a warning and continue without recording.
                        warn!("recording disabled: {reason}");
                        (None, None)
                    }
                    None => return Err(ExpectedError::RecordSessionSetupError { err }),
                },
            }
        } else {
            (None, None)
        };

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

        // Set the run ID unique prefix for highlighting if a recording session is active.
        if let Some(prefix) = run_id_unique_prefix {
            reporter.set_run_id_unique_prefix(prefix);
        }

        // TODO: no_capture is always true for benchmarks for now.
        configure_handle_inheritance(true)?;
        let run_stats = runner.try_execute(|event| reporter.report_event(event))?;
        let reporter_stats = reporter.finish();

        // Benchmarks don't support reruns, so outstanding_not_seen_count is
        // always None.
        let rerun_available = recording_session.is_some();
        let result = final_result(
            NextestRunMode::Benchmark,
            run_stats,
            runner_opts.no_tests,
            None,
            rerun_available,
        );

        let exit_code = result.as_ref().err().map_or(0, |e| e.process_exit_code());

        if let Some(session) = recording_session {
            let policy = RecordRetentionPolicy::from(&resolved_user_config.record);
            let mut styles = RecordStyles::default();
            if should_colorize {
                styles.colorize();
            }
            session
                .finalize(
                    reporter_stats.recording_sizes,
                    reporter_stats.run_finished,
                    exit_code,
                    &policy,
                )
                .log(&styles);
        }

        self.base
            .check_version_config_final(version_only_config.nextest_version())?;

        result
    }

    fn resolve_rerun(
        &self,
        run_id_selector: &RunIdSelector,
    ) -> Result<(RerunState, ComputedRerunInfo), ExpectedError> {
        let cache_dir = records_cache_dir(&self.base.workspace_root)
            .map_err(|err| ExpectedError::RecordCacheDirNotFound { err })?;

        let store =
            RunStore::new(&cache_dir).map_err(|err| ExpectedError::RecordSetupError { err })?;

        // First, acquire a shared lock to resolve the run ID and read the
        // parent run.
        //
        // TODO: should this be an exclusive lock based on if *this* run is
        // going to be recorded?
        let snapshot = store
            .lock_shared()
            .map_err(|err| ExpectedError::RecordSetupError { err })?
            .into_snapshot();

        let resolved = snapshot
            .resolve_run_id(run_id_selector)
            .map_err(|err| ExpectedError::RunIdResolutionError { err })?;
        let parent_run_id = resolved.run_id;

        // Check the format version.
        let run_info = snapshot
            .runs()
            .iter()
            .find(|r| r.run_id == parent_run_id)
            .expect("resolved run ID must be in the snapshot");

        if run_info.store_format_version != RECORD_FORMAT_VERSION {
            return Err(ExpectedError::UnsupportedStoreFormatVersion {
                run_id: parent_run_id,
                found: run_info.store_format_version,
                supported: RECORD_FORMAT_VERSION,
            });
        }

        let run_dir = snapshot.runs_dir().run_dir(parent_run_id);
        let mut reader =
            RecordReader::open(&run_dir).map_err(|err| ExpectedError::RecordReadError { err })?;

        let (outstanding_tests, root_info) = ComputedRerunInfo::compute(&mut reader)
            .map_err(|err| ExpectedError::RecordReadError { err })?;
        let root_info = root_info.unwrap_or_else(|| {
            RerunRootInfo::new(parent_run_id, run_info.build_scope_args.clone())
        });

        Ok((
            RerunState {
                parent_run_id,
                root_info,
            },
            outstanding_tests,
        ))
    }
}

#[derive(Debug)]
struct RerunState {
    parent_run_id: ReportUuid,
    root_info: RerunRootInfo,
}

/// Determines the final result of a test run.
fn final_result(
    mode: NextestRunMode,
    run_stats: RunStats,
    no_tests: Option<NoTestsBehaviorOpt>,
    outstanding_not_seen_count: Option<usize>,
    rerun_available: bool,
) -> Result<(), ExpectedError> {
    let final_stats = run_stats.summarize_final();
    let is_rerun = outstanding_not_seen_count.is_some();

    // Handle no-tests-run case first.
    if matches!(final_stats, FinalRunStats::NoTestsRun) {
        match no_tests {
            Some(NoTestsBehaviorOpt::Pass) => return Ok(()),
            Some(NoTestsBehaviorOpt::Warn) => {
                warn!("no {} to run", plural::tests_plural(mode));
                return Ok(());
            }
            Some(NoTestsBehaviorOpt::Fail) => {
                return Err(ExpectedError::NoTestsRun {
                    mode,
                    is_default: false,
                });
            }
            // For reruns, Auto/None checks outstanding tests below.
            // For non-reruns, Auto/None should fail.
            Some(NoTestsBehaviorOpt::Auto) => {
                if !is_rerun {
                    return Err(ExpectedError::NoTestsRun {
                        mode,
                        is_default: false,
                    });
                }
                // is_rerun: fall through to outstanding check
            }
            None => {
                if !is_rerun {
                    return Err(ExpectedError::NoTestsRun {
                        mode,
                        is_default: true,
                    });
                }
                // is_rerun: fall through to outstanding check
            }
        }
    } else {
        // Tests ran. Check if the run failed.
        if let Some(err) = final_stats_to_error(final_stats, mode, rerun_available) {
            return Err(err);
        }
    }

    // Run succeeded (or no tests ran on a rerun). Check for outstanding tests.
    match outstanding_not_seen_count {
        Some(0) => {
            info!("no outstanding tests remain");
            Ok(())
        }
        Some(count) => Err(ExpectedError::RerunTestsOutstanding { count }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nextest_runner::reporter::events::RunStats;

    fn make_run_stats(initial_run_count: usize, finished_count: usize, passed: usize) -> RunStats {
        RunStats {
            initial_run_count,
            finished_count,
            passed,
            ..Default::default()
        }
    }

    #[test]
    fn test_final_result() {
        // --no-tests=pass always succeeds.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Pass),
            None,
            false,
        );
        assert!(result.is_ok(), "--no-tests=pass should succeed");

        // --no-tests=warn succeeds (with a warning).
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Warn),
            None,
            false,
        );
        assert!(result.is_ok(), "--no-tests=warn should succeed");

        // --no-tests=fail fails.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Fail),
            None,
            false,
        );
        assert!(
            matches!(
                result,
                Err(ExpectedError::NoTestsRun {
                    is_default: false,
                    ..
                })
            ),
            "--no-tests=fail should fail"
        );

        // --no-tests=auto (not a rerun) fails.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Auto),
            None,
            false,
        );
        assert!(
            matches!(
                result,
                Err(ExpectedError::NoTestsRun {
                    is_default: false,
                    ..
                })
            ),
            "--no-tests=auto (not rerun) should fail"
        );

        // --no-tests=auto (rerun with outstanding) returns RerunTestsOutstanding.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Auto),
            Some(5),
            false,
        );
        assert!(
            matches!(
                result,
                Err(ExpectedError::RerunTestsOutstanding { count: 5 })
            ),
            "--no-tests=auto (rerun with outstanding) should return RerunTestsOutstanding"
        );

        // --no-tests=auto (rerun with no outstanding) succeeds.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(
            NextestRunMode::Test,
            stats,
            Some(NoTestsBehaviorOpt::Auto),
            Some(0),
            false,
        );
        assert!(
            result.is_ok(),
            "--no-tests=auto (rerun, no outstanding) should succeed"
        );

        // Default (not a rerun) fails with is_default: true.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(NextestRunMode::Test, stats, None, None, false);
        assert!(
            matches!(
                result,
                Err(ExpectedError::NoTestsRun {
                    is_default: true,
                    ..
                })
            ),
            "default (not rerun) should fail with is_default: true"
        );

        // Default (rerun with outstanding) returns RerunTestsOutstanding.
        let stats = make_run_stats(0, 0, 0);
        let result = final_result(NextestRunMode::Test, stats, None, Some(3), false);
        assert!(
            matches!(
                result,
                Err(ExpectedError::RerunTestsOutstanding { count: 3 })
            ),
            "default (rerun with outstanding) should return RerunTestsOutstanding"
        );

        // Not a rerun: succeeds.
        let stats = make_run_stats(5, 5, 5);
        let result = final_result(NextestRunMode::Test, stats, None, None, false);
        assert!(
            result.is_ok(),
            "all tests passed (not rerun) should succeed"
        );

        // Rerun with no outstanding: succeeds.
        let stats = make_run_stats(5, 5, 5);
        let result = final_result(NextestRunMode::Test, stats, None, Some(0), false);
        assert!(
            result.is_ok(),
            "all tests passed (rerun, no outstanding) should succeed"
        );

        // Rerun with outstanding: returns RerunTestsOutstanding.
        let stats = make_run_stats(5, 5, 5);
        let result = final_result(NextestRunMode::Test, stats, None, Some(2), false);
        assert!(
            matches!(
                result,
                Err(ExpectedError::RerunTestsOutstanding { count: 2 })
            ),
            "all tests passed (rerun with outstanding) should return RerunTestsOutstanding"
        );

        // Failures return TestRunFailed (no rerun available).
        let mut stats = make_run_stats(5, 5, 3);
        stats.failed = 2;
        let result = final_result(NextestRunMode::Test, stats, None, None, false);
        assert!(
            matches!(
                result,
                Err(ExpectedError::TestRunFailed {
                    rerun_available: false
                })
            ),
            "test failures should return TestRunFailed"
        );

        // Failures return TestRunFailed (rerun available).
        let mut stats = make_run_stats(5, 5, 3);
        stats.failed = 2;
        let result = final_result(NextestRunMode::Test, stats, None, None, true);
        assert!(
            matches!(
                result,
                Err(ExpectedError::TestRunFailed {
                    rerun_available: true
                })
            ),
            "test failures with rerun available should return TestRunFailed with rerun_available: true"
        );

        // Failures take precedence over outstanding tests.
        let mut stats = make_run_stats(5, 5, 3);
        stats.failed = 2;
        let result = final_result(NextestRunMode::Test, stats, None, Some(10), false);
        assert!(
            matches!(
                result,
                Err(ExpectedError::TestRunFailed {
                    rerun_available: false
                })
            ),
            "test failures should take precedence over outstanding tests"
        );
    }
}
