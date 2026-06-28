// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TODO-RAINCLAUDE: integration tests for the cpu-priority pre-run probe and its warnings: the Unix probe-nice IPC contract, and the end-to-end warning pipeline on both platforms.

use crate::temp_project::TempProject;
use integration_tests::{env::set_env_vars_for_test, nextest_cli::CargoNextestCli};
#[cfg(unix)]
use nextest_runner::cpu_priority_probe::{CpuPriorityProbeOutcome, run_cpu_priority_probe};

// TODO-RAINCLAUDE: end-to-end check that the internal run-start probe and the `nextest debug probe-nice` command still agree. The runner builds exactly these args (see cpu_priority_probe::run_probe_subprocess) and parses the child's stdout as a CpuPriorityProbeReport, so a subcommand/flag/output-format drift would silently disable the privilege warning; this asserts the real binary still produces a parseable report.
#[cfg(unix)]
#[test]
fn test_debug_probe_nice_json_contract() {
    let env_info = set_env_vars_for_test();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "debug",
            "probe-nice",
            "--output-format",
            "json",
            "--nice=19",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("probe report parses as JSON ({err}):\n{stdout}"));

    assert!(
        report["baseline"].is_i64(),
        "report carries an integer baseline: {report}"
    );
    let entries = report["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries is an array: {report}"));
    let nice19 = entries
        .iter()
        .find(|entry| entry["nice"] == 19)
        .unwrap_or_else(|| panic!("nice 19 appears in the report: {report}"));

    // TODO-RAINCLAUDE: nice 19 is the lowest priority, so applying it is always a lowering that needs no privilege — deterministic even under root.
    assert_eq!(
        nice19["outcome"], "applied",
        "nice 19 is always applicable: {report}"
    );
}

// TODO-RAINCLAUDE: end-to-end coverage of the runner-side probe pipeline (tally → probe subprocess → warning), which the fixture's default below-normal setting never triggers. The cpu-priority-raise profile requests high (nice -10) for test_success. Whether the raise is permitted depends on the environment, so key the expectation off whether this test process itself can raise to nice -10 — it has the same uid/rlimits/capabilities as the spawned nextest. Either way, "unable to determine" must never appear: that is the probe-broken fallback, and it firing here means the parent/probe-subcommand contract drifted.
#[cfg(unix)]
#[test]
fn test_cpu_priority_raise_probe_warning() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile=cpu-priority-raise",
            "-E",
            "test(=test_success)",
        ])
        .output();

    let stderr = output.stderr_as_str();
    assert!(
        !stderr.contains("unable to determine"),
        "the probe produces a verdict rather than the probe-failed fallback:\n{output}"
    );

    if self_can_set_nice(-10) {
        assert!(
            !stderr.contains("could not be applied"),
            "this environment can raise to nice -10, so no warning is expected:\n{output}"
        );
    } else {
        assert!(
            stderr.contains("the requested CPU priority increase could not be applied for 1 test"),
            "the warning names the affected test count:\n{output}"
        );
        assert!(
            stderr.contains("high (nice -10): 1 test"),
            "the warning breaks out the denied level with its count:\n{output}"
        );
    }
}

// TODO-RAINCLAUDE: Windows end-to-end coverage of the in-process probe (tally → throwaway job → warning). Whether the high class is permitted depends on the token (nextest best-effort enables SeIncreaseBasePriorityPrivilege, which succeeds on admin tokens), so accept both outcomes but pin the invariants: if the probe warns it must name the level and count, and the spawn-time backstop ("unable to apply") must never fire — the probe seeds the denied/warned bitsets precisely so the two warnings are mutually exclusive, and probe/spawn drift is impossible with an unchanged token.
#[cfg(windows)]
#[test]
fn test_cpu_priority_raise_probe_warning_windows() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile=cpu-priority-raise",
            "-E",
            "test(=test_success)",
        ])
        .output();

    let stderr = output.stderr_as_str();
    if stderr.contains("could not be applied") {
        assert!(
            stderr.contains("the requested CPU priority could not be applied for 1 test"),
            "the warning names the affected test count:\n{output}"
        );
        assert!(
            stderr.contains("high (HIGH_PRIORITY_CLASS): 1 test"),
            "the warning breaks out the denied level with its count:\n{output}"
        );
        assert!(
            stderr.contains("SeIncreaseBasePriorityPrivilege"),
            "the warning hints at the required privilege:\n{output}"
        );
    }
    assert!(
        !stderr.contains("unable to apply the"),
        "the spawn-time backstop stays silent; the probe warning (if any) covers the denial:\n{output}"
    );
}

// TODO-RAINCLAUDE: the probe must only count tests that will actually run: with the raising test filtered out, no cpu-priority warning of any kind may appear, pinning the filter_match skip in tally_requested_cpu_priorities. On environments permitted to raise priority the warning wouldn't fire anyway, so this is load-bearing where raising is denied — the common unprivileged-CI case.
#[test]
fn test_cpu_priority_no_warning_when_raising_test_filtered_out() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "run",
            "--profile=cpu-priority-raise",
            "-E",
            "test(=test_cwd)",
        ])
        .output();

    let stderr = output.stderr_as_str();
    // TODO-RAINCLAUDE: match warning-specific fragments, not "cpu-priority" — the reporter prints the profile name (cpu-priority-raise) to stderr. "CPU priority" covers both platforms' probe warnings; the other two cover the Windows spawn-time backstop and the job-assignment warning.
    assert!(
        !stderr.contains("CPU priority")
            && !stderr.contains("cpu-priority class")
            && !stderr.contains("cpu-priority setting"),
        "no cpu-priority warning fires when the raising test is filtered out:\n{output}"
    );
}

// TODO-RAINCLAUDE: returns true if this process may set its own nice to the given value, by running the same probe the runner's warning is based on (which restores the baseline itself).
#[cfg(unix)]
fn self_can_set_nice(nice: i32) -> bool {
    let report = run_cpu_priority_probe(&[nice]);
    report
        .entries
        .iter()
        .any(|entry| entry.nice == nice && entry.outcome == CpuPriorityProbeOutcome::Applied)
}
