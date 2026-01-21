// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helper functions for dispatch operations.

use crate::{
    ExpectedError, ExtractOutputFormat, Result,
    cargo_cli::{CargoCli, CargoOptions},
    output::{OutputContext, StderrStyles},
};
use camino::Utf8Path;
use itertools::Itertools;
use nextest_filtering::{Filterset, FiltersetKind, ParseContext};
use nextest_runner::{
    RustcCli,
    cargo_config::{CargoConfigs, TargetTriple},
    errors::TargetTripleError,
    platform::{BuildPlatforms, HostPlatform, Platform, PlatformLibdir, TargetPlatform},
    reporter::{
        TestOutputErrorSlice,
        events::{FinalRunStats, RunStatsFailureKind},
    },
    run_mode::NextestRunMode,
    target_runner::{PlatformRunner, TargetRunner},
    user_config::{UserConfig, UserConfigLocation},
};
use owo_colors::OwoColorize;
use std::io::Write;
use swrite::{SWrite, swrite};
use tracing::{debug, warn};

pub(super) fn acquire_graph_data(
    manifest_path: Option<&Utf8Path>,
    target_dir: Option<&Utf8Path>,
    cargo_opts: &CargoOptions,
    build_platforms: &BuildPlatforms,
    output: OutputContext,
) -> Result<String> {
    let cargo_target_arg = build_platforms.to_cargo_target_arg()?;
    let cargo_target_arg_str = cargo_target_arg.to_string();

    let mut cargo_cli = CargoCli::new("metadata", manifest_path, output);
    cargo_cli
        .add_args(["--format-version=1", "--all-features"])
        .add_args(["--filter-platform", &cargo_target_arg_str])
        .add_generic_cargo_options(cargo_opts);

    // We used to be able to pass in --no-deps in common cases, but that was (a) error-prone and (b)
    // a bit harder to do given that some nextest config options depend on the graph. Maybe we could
    // reintroduce it some day.

    let mut expression = cargo_cli.to_expression().stdout_capture().unchecked();
    // cargo metadata doesn't support "--target-dir" but setting the environment
    // variable works.
    if let Some(target_dir) = target_dir {
        expression = expression.env("CARGO_TARGET_DIR", target_dir);
    }
    // Capture stdout but not stderr.
    let output = expression
        .run()
        .map_err(|err| ExpectedError::cargo_metadata_exec_failed(cargo_cli.all_args(), err))?;
    if !output.status.success() {
        return Err(ExpectedError::cargo_metadata_failed(
            cargo_cli.all_args(),
            output.status,
        ));
    }

    let json = String::from_utf8(output.stdout).map_err(|error| {
        let io_error = std::io::Error::new(std::io::ErrorKind::InvalidData, error);
        ExpectedError::cargo_metadata_exec_failed(cargo_cli.all_args(), io_error)
    })?;
    Ok(json)
}

pub(super) fn detect_build_platforms(
    cargo_configs: &CargoConfigs,
    target_cli_option: Option<&str>,
) -> Result<BuildPlatforms, ExpectedError> {
    let host = HostPlatform::detect(PlatformLibdir::from_rustc_stdout(
        RustcCli::print_host_libdir().read(),
    ))?;
    let triple_info = discover_target_triple(cargo_configs, target_cli_option, &host.platform)?;
    let target = triple_info.map(|triple| {
        let libdir =
            PlatformLibdir::from_rustc_stdout(RustcCli::print_target_libdir(&triple).read());
        TargetPlatform::new(triple, libdir)
    });
    Ok(BuildPlatforms { host, target })
}

/// Loads and resolves user configuration with platform-specific overrides.
pub(super) fn resolve_user_config(
    host_platform: &Platform,
    location: UserConfigLocation<'_>,
) -> Result<UserConfig, ExpectedError> {
    UserConfig::for_host_platform(host_platform, location)
        .map_err(|e| ExpectedError::UserConfigError { err: Box::new(e) })
}

pub(super) fn discover_target_triple(
    cargo_configs: &CargoConfigs,
    target_cli_option: Option<&str>,
    host_platform: &Platform,
) -> Result<Option<TargetTriple>, TargetTripleError> {
    TargetTriple::find(cargo_configs, target_cli_option, host_platform).inspect(|v| {
        if let Some(triple) = v {
            debug!(
                "using target triple `{}` defined by `{}`; {}",
                triple.platform.triple_str(),
                triple.source,
                triple.location,
            );
        } else {
            debug!("no target triple found, assuming no cross-compilation");
        }
    })
}

pub(super) fn runner_for_target(
    cargo_configs: &CargoConfigs,
    build_platforms: &BuildPlatforms,
    styles: &StderrStyles,
) -> TargetRunner {
    match TargetRunner::new(cargo_configs, build_platforms) {
        Ok(runner) => {
            if build_platforms.target.is_some() {
                if let Some(runner) = runner.target() {
                    log_platform_runner("for the target platform, ", runner, styles);
                }
                if let Some(runner) = runner.host() {
                    log_platform_runner("for the host platform, ", runner, styles);
                }
            } else {
                // If triple is None, then the host and target platforms use the same runner if
                // any.
                if let Some(runner) = runner.target() {
                    log_platform_runner("", runner, styles);
                }
            }
            runner
        }
        Err(err) => {
            warn_on_err("target runner", &err, styles);
            TargetRunner::empty()
        }
    }
}

pub(super) fn log_platform_runner(prefix: &str, runner: &PlatformRunner, styles: &StderrStyles) {
    let runner_command = shell_words::join(std::iter::once(runner.binary()).chain(runner.args()));
    tracing::info!(
        "{prefix}using target runner `{}` defined by {}",
        runner_command.style(styles.bold),
        runner.source()
    )
}

pub(super) fn warn_on_err(thing: &str, err: &dyn std::error::Error, styles: &StderrStyles) {
    let mut s = String::with_capacity(256);
    swrite!(s, "could not determine {thing}: {err}");
    let mut next_error = err.source();
    while let Some(err) = next_error {
        swrite!(s, "\n  {} {}", "caused by:".style(styles.warning_text), err);
        next_error = err.source();
    }

    warn!("{}", s);
}

pub(super) fn build_filtersets(
    pcx: &ParseContext<'_>,
    filter_set: &[String],
    kind: FiltersetKind,
) -> Result<Vec<Filterset>> {
    let (exprs, all_errors): (Vec<_>, Vec<_>) = filter_set
        .iter()
        .map(|input| Filterset::parse(input.clone(), pcx, kind))
        .partition_result();

    if !all_errors.is_empty() {
        Err(ExpectedError::filter_expression_parse_error(all_errors))
    } else {
        Ok(exprs)
    }
}

pub(super) fn extract_slice_from_output<'a>(
    stdout: &'a [u8],
    stderr: &'a [u8],
) -> Option<TestOutputErrorSlice<'a>> {
    TestOutputErrorSlice::heuristic_extract(Some(stdout), Some(stderr))
}

pub(super) fn display_output_slice(
    output_slice: Option<TestOutputErrorSlice<'_>>,
    output_format: ExtractOutputFormat,
) -> Result<()> {
    use nextest_runner::reporter::highlight_end;
    use quick_junit::XmlString;

    match output_format {
        ExtractOutputFormat::Raw => {
            if let Some(kind) = output_slice
                && let Some(out) = kind.combined_subslice()
            {
                return std::io::stdout().write_all(out.slice).map_err(|err| {
                    ExpectedError::DebugExtractWriteError {
                        format: output_format,
                        err,
                    }
                });
            }
        }
        ExtractOutputFormat::JunitDescription => {
            if let Some(kind) = output_slice {
                println!("{}", XmlString::new(kind.to_string()).as_str());
            }
        }
        ExtractOutputFormat::Highlight => {
            if let Some(kind) = output_slice
                && let Some(out) = kind.combined_subslice()
            {
                let end = highlight_end(out.slice);
                return std::io::stdout()
                    .write_all(&out.slice[..end])
                    .map_err(|err| ExpectedError::DebugExtractWriteError {
                        format: output_format,
                        err,
                    });
            }
        }
    }

    eprintln!("(no description found)");
    Ok(())
}

/// Converts final run statistics to an error, if the run failed.
///
/// Returns `None` if the run was successful. For `NoTestsRun`, always returns
/// an error with `is_default: true`; callers that want custom `NoTestsBehavior`
/// handling should check for that case separately.
pub(super) fn final_stats_to_error(
    stats: FinalRunStats,
    mode: NextestRunMode,
    rerun_available: bool,
) -> Option<ExpectedError> {
    match stats {
        FinalRunStats::Success => None,
        FinalRunStats::NoTestsRun => Some(ExpectedError::NoTestsRun {
            mode,
            is_default: true,
        }),
        FinalRunStats::Cancelled {
            kind: RunStatsFailureKind::SetupScript,
            ..
        }
        | FinalRunStats::Failed {
            kind: RunStatsFailureKind::SetupScript,
        } => Some(ExpectedError::setup_script_failed()),
        FinalRunStats::Cancelled {
            kind: RunStatsFailureKind::Test { .. },
            ..
        }
        | FinalRunStats::Failed {
            kind: RunStatsFailureKind::Test { .. },
        } => Some(ExpectedError::test_run_failed(rerun_available)),
    }
}
