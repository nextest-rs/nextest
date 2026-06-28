// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TODO-RAINCLAUDE: module docs — probe whether requested CPU priorities can actually be applied.
// TODO-RAINCLAUDE: on Unix, raising priority (lowering nice) needs privilege that varies by platform (CAP_SYS_NICE/RLIMIT_NICE on Linux, superuser on macOS/FreeBSD/OpenBSD, PRIV_PROC_PRIOUP on illumos); rather than model that matrix (and still miss seccomp/LSM/jail), we measure the real kernel decision.
// TODO-RAINCLAUDE: before a run nextest spawns a single `nextest debug probe-nice` subprocess (a throwaway copy of itself) that attempts setpriority for each requested nice value (the runner requests only sub-baseline ones, i.e. priority raises) and reports the errno per value as JSON.
// TODO-RAINCLAUDE: the probe re-execs nextest once, matching the double-spawn launch path exactly (identical uid/rlimits/capabilities). The pre_exec fallback instead applies setpriority in a plain fork with no re-exec, so for an effective-only CAP_SYS_NICE (dropped on execve but kept across fork) the probe can over-warn; the common cases (root, file/ambient caps) coincide.
// TODO-RAINCLAUDE: Unix-only — the Windows counterpart lives in runner/windows.rs and needs no subprocess: privilege checks happen at job-object creation, in-process, so a throwaway job with the same limits measures the real kernel decision. (Setting a job's priority class to high requires SeIncreaseBasePriorityPrivilege enabled in the token, unlike SetPriorityClass where high is unprivileged and only realtime is gated. MSDN documents the job priority-class privilege requirement without qualification; that only high is gated in practice is empirical — see probe_applies_unprivileged_priority_classes in runner/windows.rs.)

use crate::{
    config::elements::CpuPriorityLevel, double_spawn::DoubleSpawnInfo, helpers::plural,
    run_mode::NextestRunMode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use swrite::{SWrite, swrite};
use thiserror::Error;
use tokio::runtime::Runtime;
use tracing::warn;

// TODO-RAINCLAUDE: outcome of attempting to apply a single nice value in the probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CpuPriorityProbeOutcome {
    // TODO-RAINCLAUDE: setpriority succeeded; this nice value can be applied.
    Applied,

    // TODO-RAINCLAUDE: setpriority failed with this errno.
    Failed {
        // TODO-RAINCLAUDE: the raw errno returned by setpriority.
        errno: i32,
    },
}

// TODO-RAINCLAUDE: result of probing a single nice value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CpuPriorityProbeEntry {
    // TODO-RAINCLAUDE: the nice value that was probed.
    pub nice: i32,

    // TODO-RAINCLAUDE: the outcome of the probe.
    pub outcome: CpuPriorityProbeOutcome,
}

// TODO-RAINCLAUDE: bundle of probe results, emitted by `nextest debug probe-nice --output-format json` as a single JSON line on stdout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CpuPriorityProbeReport {
    // TODO-RAINCLAUDE: baseline nice value observed by the probe (inherited from nextest).
    pub baseline: i32,

    // TODO-RAINCLAUDE: one entry per probed nice value, in the order they were probed.
    pub entries: Vec<CpuPriorityProbeEntry>,
}

// TODO-RAINCLAUDE: attempts to apply each nice value to the current process, returning a report; runs inside `nextest debug probe-nice` (a throwaway nextest process) to measure which setpriority calls the OS permits in the same security context a test would inherit.
// TODO-RAINCLAUDE: values are probed in ascending nice order so the per-value reset to baseline stays reliable. Increasing one's nice is unconditionally permitted; decreasing it is what may need privilege (root, CAP_SYS_NICE, or RLIMIT_NICE headroom on Linux). In ascending order the process stays at or below baseline while the sub-baseline values are probed, so each reset is an increase or a no-op and always succeeds. Once an at-or-above-baseline value drifts the process above baseline, a reset (now a decrease) can fail — but every later value is >= the current nice, so its set still succeeds and reports the same Applied a fresh, baseline-inheriting test process would see.
pub fn run_cpu_priority_probe(nice_values: &[i32]) -> CpuPriorityProbeReport {
    let baseline = current_nice();

    // TODO-RAINCLAUDE: ascending order keeps each reset reliable; see the fn docs.
    let mut sorted: Vec<i32> = nice_values.to_vec();
    sorted.sort_unstable();

    let entries = sorted
        .iter()
        .map(|&nice| {
            // TODO-RAINCLAUDE: reset to baseline so this value is tested from the same start a fresh test process would see.
            let _ = try_set_nice(baseline);
            let outcome = match try_set_nice(nice) {
                Ok(()) => CpuPriorityProbeOutcome::Applied,
                Err(errno) => CpuPriorityProbeOutcome::Failed { errno },
            };
            CpuPriorityProbeEntry { nice, outcome }
        })
        .collect();

    // TODO-RAINCLAUDE: leave the process at its baseline; it's about to exit regardless.
    let _ = try_set_nice(baseline);

    CpuPriorityProbeReport { baseline, entries }
}

// TODO-RAINCLAUDE: reads the current process's nice value. pub(crate) as the single home for the getpriority self-read; also used by runner tests.
pub(crate) fn current_nice() -> i32 {
    // TODO-RAINCLAUDE: SAFETY — getpriority(PRIO_PROCESS, 0) reads the calling process's nice. A -1 return is ambiguous between an error and a real nice of -1, but errors are impossible for self with PRIO_PROCESS, so the raw return is always the nice value and the errno protocol is unnecessary.
    unsafe { libc::getpriority(libc::PRIO_PROCESS as _, 0) }
}

// TODO-RAINCLAUDE: attempts to set the current process's nice value, returning the errno on failure. pub(crate) as the single home for the setpriority call; the double-spawn child and the pre_exec hook call this and discard the result. Fork-safe: a bare syscall plus an errno read, no locks or allocation.
pub(crate) fn try_set_nice(nice: i32) -> Result<(), i32> {
    // TODO-RAINCLAUDE: SAFETY — setpriority(PRIO_PROCESS, 0, ...) sets the calling process's nice and is always safe to call.
    let ret = unsafe { libc::setpriority(libc::PRIO_PROCESS as _, 0, nice as libc::c_int) };
    if ret == 0 {
        Ok(())
    } else {
        // TODO-RAINCLAUDE: setpriority is unambiguous: 0 on success, -1 on error. last_os_error always carries a raw errno on our platforms; expect rather than defaulting to 0, which would render as "Success" in the warning.
        Err(io::Error::last_os_error()
            .raw_os_error()
            .expect("last_os_error after a failed setpriority carries a raw errno"))
    }
}

// TODO-RAINCLAUDE: probes whether the CPU priority levels requested by this run (tallied by the caller) can actually be applied, and warns once if not.
pub(crate) fn maybe_warn_cpu_priority(
    level_counts: &BTreeMap<CpuPriorityLevel, usize>,
    mode: NextestRunMode,
    double_spawn: &DoubleSpawnInfo,
    runtime: &Runtime,
) {
    let baseline = current_nice();
    if let Some(warning) = evaluate_cpu_priority(level_counts, baseline, |targets| {
        run_probe_subprocess(runtime, double_spawn, targets)
    }) {
        warn!("{}", warning.render(mode));
    }
}

// TODO-RAINCLAUDE: why the probe failed to produce a usable verdict; rendered into the probe-failed warning so the user learns the cause without a debug-log rerun.
#[derive(Debug, Error)]
enum ProbeError {
    #[error("could not determine the path to nextest's own executable")]
    ExeResolution,

    #[error("failed to run the probe ({}): {error}", exe.display())]
    Run { exe: PathBuf, error: io::Error },

    #[error("the probe timed out after {} seconds", timeout.as_secs())]
    Timeout { timeout: Duration },

    #[error("the probe exited with {status}: {stderr}")]
    NonZeroExit { status: ExitStatus, stderr: String },

    #[error("failed to parse the probe's output: {error}")]
    Parse { error: serde_json::Error },

    #[error("the probe report is missing an entry for nice {missing_nice}")]
    IncompleteReport { missing_nice: i32 },
}

// TODO-RAINCLAUDE: structured outcome of the pre-run evaluation, so callers and tests work with data rather than rendered strings; render() produces the user-facing message.
#[derive(Debug)]
enum CpuPriorityWarning {
    // TODO-RAINCLAUDE: the probe ran and these levels were denied; baseline is the probe's own measurement.
    LevelsDenied {
        baseline: i32,
        affected: Vec<AffectedLevel>,
    },

    // TODO-RAINCLAUDE: the probe couldn't produce a verdict; warn conservatively about every requested raise, naming the cause.
    ProbeFailed {
        baseline: i32,
        raising: Vec<(CpuPriorityLevel, usize)>,
        cause: ProbeError,
    },
}

impl CpuPriorityWarning {
    fn render(&self, mode: NextestRunMode) -> String {
        match self {
            CpuPriorityWarning::LevelsDenied { baseline, affected } => {
                render_warning(*baseline, affected, mode)
            }
            CpuPriorityWarning::ProbeFailed {
                baseline,
                raising,
                cause,
            } => render_probe_failed_warning(*baseline, raising, mode, cause),
        }
    }
}

// TODO-RAINCLAUDE: pure decision core, separated from I/O so it can be tested without privilege or a real subprocess. Given the requested levels, nextest's baseline nice, and a probe runner, returns the structured warning to emit, if any. Only sub-baseline levels (priority raises) can fail, so only they are probed; when the probe can't render a verdict we still warn rather than stay silent, folding the probe's failure cause into the warning.
fn evaluate_cpu_priority(
    level_counts: &BTreeMap<CpuPriorityLevel, usize>,
    baseline: i32,
    probe: impl FnOnce(&[i32]) -> Result<CpuPriorityProbeReport, ProbeError>,
) -> Option<CpuPriorityWarning> {
    // TODO-RAINCLAUDE: a level is a priority raise (and so might need privilege) only when its nice is below nextest's own; lowering priority never fails.
    let mut raising: Vec<(CpuPriorityLevel, usize)> = level_counts
        .iter()
        .filter(|(level, _)| level.to_nice() < baseline)
        .map(|(&level, &count)| (level, count))
        .collect();
    if raising.is_empty() {
        return None;
    }
    // TODO-RAINCLAUDE: most aggressive (lowest nice) first; explicit so it doesn't depend on the BTreeMap key order.
    raising.sort_by_key(|(level, _)| level.to_nice());

    // TODO-RAINCLAUDE: levels are distinct BTreeMap keys and to_nice is injective, so after the sort above this is already sorted and duplicate-free.
    let targets: Vec<i32> = raising.iter().map(|(level, _)| level.to_nice()).collect();

    match probe(&targets) {
        Ok(report) => {
            // TODO-RAINCLAUDE: a requested raise missing from the report means the probe protocol drifted between the parent and the probe subcommand; treat it exactly like a probe failure (warn) rather than silently assuming the raise works.
            if let Some(&missing) = targets
                .iter()
                .find(|&&nice| !report.entries.iter().any(|entry| entry.nice == nice))
            {
                return Some(CpuPriorityWarning::ProbeFailed {
                    baseline,
                    raising,
                    cause: ProbeError::IncompleteReport {
                        missing_nice: missing,
                    },
                });
            }
            let affected = analyze_probe(level_counts, &report);
            (!affected.is_empty()).then_some(CpuPriorityWarning::LevelsDenied {
                baseline: report.baseline,
                affected,
            })
        }
        // TODO-RAINCLAUDE: the probe couldn't run or parse, so we can't say which raises are permitted; warn rather than stay silent, since the real setpriority calls swallow EACCES and rely on this warning.
        Err(cause) => Some(CpuPriorityWarning::ProbeFailed {
            baseline,
            raising,
            cause,
        }),
    }
}

// TODO-RAINCLAUDE: upper bound on how long the probe subprocess may take; it does a handful of setpriority calls, so seconds means something is wedged (hung network mount, AV interception), and we'd rather degrade to the probe-failed warning than hang the run at startup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

// TODO-RAINCLAUDE: spawns `nextest debug probe-nice` and parses its JSON report. On failure (exe resolution, spawn, timeout, nonzero exit, or unparseable output) returns a structured ProbeError; the caller folds the cause into a user-facing warning rather than staying silent.
fn run_probe_subprocess(
    runtime: &Runtime,
    double_spawn: &DoubleSpawnInfo,
    targets: &[i32],
) -> Result<CpuPriorityProbeReport, ProbeError> {
    // TODO-RAINCLAUDE: the probe must run nextest's own binary so it inherits the same uid/rlimits/capabilities a test will; prefer the path double-spawn already resolved, else re-resolve through the same get_current_exe (e.g. /proc/self/exe on Linux) when double-spawn is disabled.
    let Some(exe) = double_spawn
        .current_exe()
        .map(Path::to_path_buf)
        .or_else(|| crate::double_spawn::get_current_exe().ok())
    else {
        return Err(ProbeError::ExeResolution);
    };

    let mut command = tokio::process::Command::new(&exe);
    // TODO-RAINCLAUDE: reuse the user-facing `nextest debug probe-nice` command with JSON output; a single execve from us, mirroring the double-spawn launch path's privilege context.
    command.args(["nextest", "debug", "probe-nice", "--output-format", "json"]);
    for &nice in targets {
        command.arg(format!("--nice={nice}"));
    }
    command.stdin(Stdio::null());
    // TODO-RAINCLAUDE: kill_on_drop so a timeout (which drops the output future) also kills the wedged probe rather than leaking it.
    command.kill_on_drop(true);

    // TODO-RAINCLAUDE: block SIGTSTP across the spawn like test spawns do, so a Ctrl-Z here can't stop the probe child and wedge our wait; if double-spawn is disabled there's no context, and this brief awaited subprocess accepts the small residual risk.
    let _spawn_context = double_spawn.spawn_context();
    let output = runtime.block_on(async {
        match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
            Ok(result) => result.map_err(|error| ProbeError::Run { exe, error }),
            Err(_) => Err(ProbeError::Timeout {
                timeout: PROBE_TIMEOUT,
            }),
        }
    })?;

    if !output.status.success() {
        return Err(ProbeError::NonZeroExit {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    serde_json::from_slice::<CpuPriorityProbeReport>(&output.stdout)
        .map_err(|error| ProbeError::Parse { error })
}

// TODO-RAINCLAUDE: a CPU priority level whose setpriority probe failed, with how many tests requested it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AffectedLevel {
    level: CpuPriorityLevel,
    nice: i32,
    test_count: usize,
    errno: i32,
}

// TODO-RAINCLAUDE: cross-references requested levels with the probe report to find which levels couldn't be applied and how many tests each affects.
fn analyze_probe(
    level_counts: &BTreeMap<CpuPriorityLevel, usize>,
    report: &CpuPriorityProbeReport,
) -> Vec<AffectedLevel> {
    let mut affected: Vec<AffectedLevel> = level_counts
        .iter()
        .filter_map(|(&level, &test_count)| {
            let nice = level.to_nice();
            let entry = report.entries.iter().find(|e| e.nice == nice)?;
            match entry.outcome {
                CpuPriorityProbeOutcome::Failed { errno } => Some(AffectedLevel {
                    level,
                    nice,
                    test_count,
                    errno,
                }),
                CpuPriorityProbeOutcome::Applied => None,
            }
        })
        .collect();

    // TODO-RAINCLAUDE: report the most aggressive (lowest nice) levels first.
    affected.sort_by_key(|a| a.nice);
    affected
}

// TODO-RAINCLAUDE: renders the user-facing warning for levels that could not be applied.
fn render_warning(baseline: i32, affected: &[AffectedLevel], mode: NextestRunMode) -> String {
    let total: usize = affected.iter().map(|a| a.test_count).sum();

    let mut msg = String::new();
    swrite!(
        msg,
        "the requested CPU priority increase could not be applied for {total} {tests}, \
         which will run at nextest's current priority (nice {baseline}) instead:",
        tests = plural::tests_str(mode, total),
    );
    for a in affected {
        swrite!(
            msg,
            "\n  - {} (nice {}): {} {} ({})",
            a.level.as_str(),
            a.nice,
            a.test_count,
            plural::tests_str(mode, a.test_count),
            io::Error::from_raw_os_error(a.errno),
        );
    }
    swrite!(msg, "\n{}.", privilege_hint());

    msg
}

// TODO-RAINCLAUDE: renders the warning for when the probe itself couldn't run, so we can't tell which raises are permitted. The swallow-EACCES design depends on warning here rather than going silent; the cause tells the user why no verdict was available without needing a debug-log rerun.
fn render_probe_failed_warning(
    baseline: i32,
    raising: &[(CpuPriorityLevel, usize)],
    mode: NextestRunMode,
    cause: &ProbeError,
) -> String {
    let total: usize = raising.iter().map(|(_, count)| count).sum();

    let mut msg = String::new();
    swrite!(
        msg,
        "unable to determine whether the requested CPU priority increase can be applied for \
         {total} {tests} ({cause}); they may run at nextest's current priority (nice {baseline}) \
         instead:",
        tests = plural::tests_str(mode, total),
    );
    for (level, count) in raising {
        swrite!(
            msg,
            "\n  - {} (nice {}): {} {}",
            level.as_str(),
            level.to_nice(),
            count,
            plural::tests_str(mode, *count),
        );
    }
    swrite!(msg, "\n{}.", privilege_hint());

    msg
}

// TODO-RAINCLAUDE: one-line hint on what privilege a CPU priority raise needs on this platform; CAP_SYS_NICE is the Linux-specific way to grant it without root, other Unixes gate raises on the superuser.
fn privilege_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        "raising CPU priority typically requires running as root or the CAP_SYS_NICE capability"
    } else {
        "raising CPU priority typically requires running as root"
    }
}

// TODO-RAINCLAUDE: renders a probe report as a human-readable, level-labeled report; used by `nextest debug probe-nice`. Lives here rather than in cargo-nextest so it shares the level labels and privilege hint with the run-start warning.
pub fn format_probe_report_human(report: &CpuPriorityProbeReport) -> String {
    let mut out = String::new();
    swrite!(out, "baseline CPU priority: nice {}\n", report.baseline);

    let mut any_failed = false;
    for entry in &report.entries {
        // TODO-RAINCLAUDE: label by level when the nice value matches one of nextest's levels, else show the raw nice value.
        let label = match CpuPriorityLevel::ALL
            .iter()
            .copied()
            .find(|&level| level.to_nice() == entry.nice)
        {
            Some(level) => format!("{} (nice {})", level.as_str(), entry.nice),
            None => format!("nice {}", entry.nice),
        };
        match entry.outcome {
            CpuPriorityProbeOutcome::Applied => {
                swrite!(out, "  {label}: can be applied\n");
            }
            CpuPriorityProbeOutcome::Failed { errno } => {
                any_failed = true;
                swrite!(
                    out,
                    "  {label}: cannot be applied ({})\n",
                    io::Error::from_raw_os_error(errno),
                );
            }
        }
    }

    if any_failed {
        swrite!(out, "\n{}.\n", privilege_hint());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(nice: i32, outcome: CpuPriorityProbeOutcome) -> CpuPriorityProbeEntry {
        CpuPriorityProbeEntry { nice, outcome }
    }

    #[test]
    fn human_report_labels_levels_and_notes_failures() {
        // TODO-RAINCLAUDE: errno 13 is EACCES; only the "cannot be applied" wording is asserted, so the exact value is not load-bearing.
        let report = CpuPriorityProbeReport {
            baseline: 0,
            entries: vec![
                entry(-10, CpuPriorityProbeOutcome::Failed { errno: 13 }),
                entry(5, CpuPriorityProbeOutcome::Applied),
                entry(-7, CpuPriorityProbeOutcome::Applied),
            ],
        };

        let rendered = format_probe_report_human(&report);
        assert!(
            rendered.contains("baseline CPU priority: nice 0"),
            "{rendered}"
        );
        assert!(
            rendered.contains("high (nice -10): cannot be applied"),
            "{rendered}"
        );
        assert!(
            rendered.contains("below-normal (nice 5): can be applied"),
            "{rendered}"
        );
        assert!(
            rendered.contains("nice -7: can be applied"),
            "a nice value with no level is labeled by its raw value: {rendered}"
        );
        assert!(
            rendered.contains("root"),
            "the failure footer hints at the privilege requirement: {rendered}"
        );
    }

    #[test]
    fn probe_report_json_round_trips() {
        let report = CpuPriorityProbeReport {
            baseline: 0,
            entries: vec![
                entry(-10, CpuPriorityProbeOutcome::Failed { errno: 13 }),
                entry(-5, CpuPriorityProbeOutcome::Applied),
            ],
        };

        let json = serde_json::to_string(&report).expect("report serializes");
        // TODO-RAINCLAUDE: outcome is externally tagged — bare string for the unit variant, single-key object for the struct variant.
        assert!(
            json.contains("\"applied\""),
            "applied outcome serializes to a bare string: {json}"
        );
        assert!(
            json.contains("\"failed\":{\"errno\":13}"),
            "failed outcome carries the errno: {json}"
        );

        let parsed: CpuPriorityProbeReport =
            serde_json::from_str(&json).expect("report round-trips");
        assert_eq!(parsed, report, "report survives a JSON round trip");
    }

    #[test]
    fn analyze_flags_only_failed_levels() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 3);
        level_counts.insert(CpuPriorityLevel::AboveNormal, 1);
        // TODO-RAINCLAUDE: Low maps to a positive nice that was never probed, so it must never be flagged.
        level_counts.insert(CpuPriorityLevel::Low, 2);

        let report = CpuPriorityProbeReport {
            baseline: 0,
            entries: vec![
                entry(-10, CpuPriorityProbeOutcome::Failed { errno: 13 }),
                entry(-5, CpuPriorityProbeOutcome::Applied),
            ],
        };

        let affected = analyze_probe(&level_counts, &report);
        assert_eq!(
            affected,
            vec![AffectedLevel {
                level: CpuPriorityLevel::High,
                nice: -10,
                test_count: 3,
                errno: 13,
            }],
            "only the denied high level is flagged; applied and unprobed levels are not"
        );
    }

    #[test]
    fn analyze_orders_by_most_aggressive_first() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 1);
        level_counts.insert(CpuPriorityLevel::AboveNormal, 2);

        let report = CpuPriorityProbeReport {
            baseline: 0,
            entries: vec![
                entry(-5, CpuPriorityProbeOutcome::Failed { errno: 13 }),
                entry(-10, CpuPriorityProbeOutcome::Failed { errno: 1 }),
            ],
        };

        let affected = analyze_probe(&level_counts, &report);
        let nices: Vec<i32> = affected.iter().map(|a| a.nice).collect();
        assert_eq!(nices, vec![-10, -5], "lowest nice (highest priority) first");
    }

    #[test]
    fn render_warning_describes_affected_tests() {
        let affected = vec![
            AffectedLevel {
                level: CpuPriorityLevel::High,
                nice: -10,
                test_count: 3,
                errno: libc::EACCES,
            },
            AffectedLevel {
                level: CpuPriorityLevel::AboveNormal,
                nice: -5,
                test_count: 1,
                errno: libc::EACCES,
            },
        ];

        let msg = render_warning(0, &affected, NextestRunMode::Test);
        assert!(
            msg.contains("for 4 tests"),
            "the total affected count is summed across levels: {msg}"
        );
        assert!(
            msg.contains("nice 0"),
            "the baseline priority is reported: {msg}"
        );
        assert!(
            msg.contains("high (nice -10): 3 tests"),
            "the high level is broken out with its count: {msg}"
        );
        assert!(
            msg.contains("above-normal (nice -5): 1 test"),
            "the above-normal level is broken out and singularized: {msg}"
        );
        assert!(
            msg.contains("root"),
            "the message hints at the privilege requirement: {msg}"
        );
    }

    #[test]
    fn evaluate_warns_on_denied_raise() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 3);
        level_counts.insert(CpuPriorityLevel::AboveNormal, 1);
        // TODO-RAINCLAUDE: Low maps to a positive nice, never a raise from baseline 0, so it is never probed or flagged.
        level_counts.insert(CpuPriorityLevel::Low, 2);

        let warning = evaluate_cpu_priority(&level_counts, 0, |targets| {
            assert_eq!(targets, [-10, -5], "only sub-baseline levels are probed");
            Ok(CpuPriorityProbeReport {
                baseline: 0,
                entries: vec![
                    entry(
                        -10,
                        CpuPriorityProbeOutcome::Failed {
                            errno: libc::EACCES,
                        },
                    ),
                    entry(-5, CpuPriorityProbeOutcome::Applied),
                ],
            })
        })
        .expect("a denied high raise produces a warning");

        match warning {
            CpuPriorityWarning::LevelsDenied { baseline, affected } => {
                assert_eq!(baseline, 0, "the probe's baseline is carried through");
                assert_eq!(
                    affected,
                    vec![AffectedLevel {
                        level: CpuPriorityLevel::High,
                        nice: -10,
                        test_count: 3,
                        errno: libc::EACCES,
                    }],
                    "only the denied high level is flagged; the applied above-normal and \
                     non-raise low levels are not"
                );
            }
            other => panic!("expected LevelsDenied, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_is_silent_when_all_raises_apply() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 1);

        let warning = evaluate_cpu_priority(&level_counts, 0, |_| {
            Ok(CpuPriorityProbeReport {
                baseline: 0,
                entries: vec![entry(-10, CpuPriorityProbeOutcome::Applied)],
            })
        });
        assert!(
            warning.is_none(),
            "no warning when every raise can be applied: {warning:?}"
        );
    }

    #[test]
    fn evaluate_warns_when_probe_cannot_run() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 2);

        // TODO-RAINCLAUDE: a probe returning Err models the subprocess failing to spawn, timing out, or producing unparseable output; the cause must be carried into the warning.
        let warning = evaluate_cpu_priority(&level_counts, 0, |_| {
            Err(ProbeError::Timeout {
                timeout: Duration::from_secs(5),
            })
        })
        .expect("a probe failure warns rather than staying silent");

        match warning {
            CpuPriorityWarning::ProbeFailed {
                baseline,
                raising,
                cause,
            } => {
                assert_eq!(baseline, 0);
                assert_eq!(
                    raising,
                    vec![(CpuPriorityLevel::High, 2)],
                    "every requested raise is named in the conservative warning"
                );
                match cause {
                    ProbeError::Timeout { timeout } => {
                        assert_eq!(timeout, Duration::from_secs(5));
                    }
                    other => panic!("expected a Timeout cause, got {other:?}"),
                }
            }
            other => panic!("expected ProbeFailed, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_warns_when_probe_report_is_incomplete() {
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 2);
        level_counts.insert(CpuPriorityLevel::AboveNormal, 1);

        // TODO-RAINCLAUDE: the report parses but is missing a requested raise (here -5); treating it as success would silently disable the warning, so it must route to the probe-failed warning instead.
        let warning = evaluate_cpu_priority(&level_counts, 0, |_| {
            Ok(CpuPriorityProbeReport {
                baseline: 0,
                entries: vec![entry(-10, CpuPriorityProbeOutcome::Applied)],
            })
        })
        .expect("an incomplete probe report warns rather than staying silent");

        match warning {
            CpuPriorityWarning::ProbeFailed {
                baseline,
                raising,
                cause,
            } => {
                assert_eq!(baseline, 0);
                assert_eq!(
                    raising,
                    vec![
                        (CpuPriorityLevel::High, 2),
                        (CpuPriorityLevel::AboveNormal, 1)
                    ],
                    "both requested raises are named, most aggressive first"
                );
                match cause {
                    ProbeError::IncompleteReport { missing_nice } => {
                        assert_eq!(missing_nice, -5, "the missing entry is named");
                    }
                    other => panic!("expected an IncompleteReport cause, got {other:?}"),
                }
            }
            other => panic!("expected ProbeFailed, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_raise_predicate_is_relative_to_baseline() {
        // TODO-RAINCLAUDE: under a positive baseline (nextest itself niced), even below-normal and normal are raises and must be probed; low (19) still is not.
        let mut level_counts = BTreeMap::new();
        for level in CpuPriorityLevel::ALL {
            level_counts.insert(level, 1);
        }

        let warning = evaluate_cpu_priority(&level_counts, 10, |targets| {
            assert_eq!(
                targets,
                [-10, -5, 0, 5],
                "every level below baseline 10 is probed; low (19) is not"
            );
            Ok(CpuPriorityProbeReport {
                baseline: 10,
                entries: targets
                    .iter()
                    .map(|&nice| entry(nice, CpuPriorityProbeOutcome::Applied))
                    .collect(),
            })
        });
        assert!(
            warning.is_none(),
            "all raises apply, so no warning: {warning:?}"
        );
    }

    #[test]
    fn evaluate_treats_equal_nice_as_non_raise() {
        // TODO-RAINCLAUDE: boundary of the strict `to_nice() < baseline` predicate — a level equal to the baseline is not a raise (setting an equal nice always succeeds), so the probe must not run.
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::BelowNormal, 2);

        let warning = evaluate_cpu_priority(&level_counts, 5, |_| {
            panic!("a level whose nice equals the baseline must not be probed")
        });
        assert!(warning.is_none(), "{warning:?}");
    }

    #[test]
    fn evaluate_raise_predicate_under_negative_baseline() {
        // TODO-RAINCLAUDE: with nextest already at nice -5, above-normal (-5) equals the baseline and is not a raise; only high (-10) is.
        let mut level_counts = BTreeMap::new();
        level_counts.insert(CpuPriorityLevel::High, 1);
        level_counts.insert(CpuPriorityLevel::AboveNormal, 4);

        let warning = evaluate_cpu_priority(&level_counts, -5, |targets| {
            assert_eq!(targets, [-10], "only the sub-baseline high level is probed");
            Ok(CpuPriorityProbeReport {
                baseline: -5,
                entries: vec![entry(
                    -10,
                    CpuPriorityProbeOutcome::Failed {
                        errno: libc::EACCES,
                    },
                )],
            })
        })
        .expect("the denied high raise produces a warning");

        match warning {
            CpuPriorityWarning::LevelsDenied { baseline, affected } => {
                assert_eq!(baseline, -5);
                assert_eq!(
                    affected,
                    vec![AffectedLevel {
                        level: CpuPriorityLevel::High,
                        nice: -10,
                        test_count: 1,
                        errno: libc::EACCES,
                    }],
                    "only the denied high level is flagged; the at-baseline above-normal \
                     level is not"
                );
            }
            other => panic!("expected LevelsDenied, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_skips_probe_without_raises() {
        let mut level_counts = BTreeMap::new();
        // TODO-RAINCLAUDE: both map to nice >= baseline 0, so neither is a raise; the probe must not run.
        level_counts.insert(CpuPriorityLevel::BelowNormal, 1);
        level_counts.insert(CpuPriorityLevel::Low, 5);

        let warning = evaluate_cpu_priority(&level_counts, 0, |_| {
            panic!("the probe must not run when no priority raise is requested")
        });
        assert!(warning.is_none(), "{warning:?}");
    }

    #[test]
    fn render_probe_failed_warning_includes_cause() {
        let raising = vec![(CpuPriorityLevel::High, 2)];
        let msg = render_probe_failed_warning(
            0,
            &raising,
            NextestRunMode::Test,
            &ProbeError::Timeout {
                timeout: Duration::from_secs(5),
            },
        );
        assert!(msg.contains("unable to determine"), "{msg}");
        assert!(
            msg.contains("the probe timed out after 5 seconds"),
            "the failure cause is folded into the rendered warning: {msg}"
        );
        assert!(msg.contains("high (nice -10): 2 tests"), "{msg}");
        assert!(msg.contains("root"), "{msg}");
    }

    #[test]
    fn probe_applies_values_at_or_above_baseline() {
        // TODO-RAINCLAUDE: setting nice to the baseline (not a reduction) or above it (lowering priority) never needs privilege, so these are deterministic regardless of environment permissions.
        let baseline = current_nice();
        let report = run_cpu_priority_probe(&[baseline, baseline + 5]);

        assert_eq!(report.baseline, baseline, "the probe reports the baseline");
        assert_eq!(report.entries.len(), 2);
        for entry in &report.entries {
            assert_eq!(
                entry.outcome,
                CpuPriorityProbeOutcome::Applied,
                "nice {} (>= baseline {baseline}) is always applicable",
                entry.nice,
            );
        }
    }
}
