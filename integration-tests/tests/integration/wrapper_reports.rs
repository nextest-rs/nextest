// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end coverage of run wrapper reports, including recorded results.

use crate::{current_runner_env_var, temp_project::TempProject};
use camino::Utf8PathBuf;
use integration_tests::{
    env::{TestEnvInfo, set_env_vars_for_test},
    nextest_cli::CargoNextestCli,
};
use nextest_metadata::NextestExitCode;
use nextest_runner::{
    output_spec::RecordingSpec,
    record::{OutputEventKind, TestEventKindSummary, TestEventSummary, encode_workspace_path},
    reporter::events::{ExecutionResultDescription, ExecutionStatuses},
};
use std::{collections::BTreeSet, fs};

const RUN_ID: &str = "abcdef00-0000-4000-8000-000000000001";
const SUCCESS_FILTER: &str = "binary(=basic) & test(=test_success)";

fn cli(env: &TestEnvInfo, project: &TempProject, mode: &str) -> CargoNextestCli {
    let config = project.temp_root().join("reports.toml");
    fs::write(&config, format!(r#"
experimental = ["wrapper-scripts"]
[scripts.wrapper.report]
command = {{ command-line = 'debug/wrapper{} report', relative-to = "target", env = {{ WRAPPER_CMD_ENV_VAR = "report" }} }}
target-runner = "overrides-wrapper"
[[profile.default.scripts]]
filter = "all()"
run-wrapper = "report"
"#, std::env::consts::EXE_SUFFIX)).unwrap();
    let user_config = project.temp_root().join("record.toml");
    fs::write(
        &user_config,
        "[experimental]\nrecord = true\n[record]\nenabled = true\n",
    )
    .unwrap();
    let audit = project.temp_root().join(format!("audit-{mode}"));
    fs::create_dir_all(&audit).unwrap();
    let mut cli = CargoNextestCli::for_test(env);
    cli.args([
        "--manifest-path",
        project.manifest_path().as_str(),
        "--config-file",
        config.as_str(),
        "--user-config-file",
        user_config.as_str(),
    ])
    .env("NEXTEST_STATE_DIR", project.temp_root().join(mode))
    .env("__NEXTEST_FORCE_RUN_ID", RUN_ID)
    .env("__NEXTEST_WRAPPER_REPORT_MODE", mode)
    .env("__NEXTEST_WRAPPER_REPORT_AUDIT", audit)
    .env_remove("NEXTEST_RUN_WRAPPER_REPORT");
    cli
}

fn recorded_statuses(project: &TempProject, mode: &str) -> Vec<ExecutionStatuses<RecordingSpec>> {
    let workspace = project.workspace_root().canonicalize_utf8().unwrap();
    let log = project
        .temp_root()
        .join(mode)
        .join("projects")
        .join(encode_workspace_path(&workspace))
        .join("records/runs")
        .join(RUN_ID)
        .join("run.log.zst");
    let contents = zstd::stream::decode_all(fs::File::open(log).unwrap()).unwrap();
    String::from_utf8(contents)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let event: TestEventSummary<RecordingSpec> = serde_json::from_str(line).unwrap();
            match event.kind {
                TestEventKindSummary::Output(OutputEventKind::TestFinished {
                    run_statuses,
                    ..
                }) => Some(run_statuses),
                _ => None,
            }
        })
        .collect()
}

fn check_report_cleanup(project: &TempProject, mode: &str, attempts: usize) {
    let paths: BTreeSet<_> = fs::read_dir(project.temp_root().join(format!("audit-{mode}")))
        .unwrap()
        .map(|entry| Utf8PathBuf::from(fs::read_to_string(entry.unwrap().path()).unwrap()))
        .collect();
    assert_eq!(
        paths.len(),
        attempts,
        "a distinct report path for each attempt"
    );
    for path in paths {
        assert!(
            !path.parent().unwrap().exists(),
            "report directory was removed: {path}"
        );
    }
}

#[test]
fn wrapper_reports_record_and_replay() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    for mode in ["valid", "label-only", "absent"] {
        let run = cli(&env, &project, mode)
            .args(["run", "-E", SUCCESS_FILTER])
            .output();
        let statuses = recorded_statuses(&project, mode);
        assert_eq!(statuses.len(), 1);
        let report = statuses[0].last_status().run_wrapper_report.as_ref();
        assert_eq!(
            report.map(|r| r.label.as_str()),
            (mode != "absent").then_some("wrapped")
        );
        assert_eq!(
            report
                .and_then(|r| r.category.as_ref())
                .map(|category| category.as_str()),
            (mode == "valid").then_some("wrapped")
        );
        check_report_cleanup(&project, mode, 1);
        let replay = cli(&env, &project, mode)
            .args(["replay", "-R", RUN_ID])
            .output();
        for (output, text) in [
            (&run, run.stderr_as_str()),
            (&replay, replay.stdout_as_str()),
        ] {
            assert_eq!(text.contains("(wrapped)"), mode != "absent", "{output}");
            assert_eq!(
                text.contains("1 passed (wrapper: 1 wrapped)"),
                mode == "valid",
                "{output}"
            );
        }
        check_report_cleanup(&project, mode, 1);
    }
}

#[test]
fn wrapper_report_errors_fail_the_attempt_and_survive_replay() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    for mode in [
        "invalid",
        "invalid-label",
        "invalid-category",
        "oversized",
        "directory",
    ] {
        let run = cli(&env, &project, mode)
            .args(["run", "-E", SUCCESS_FILTER])
            .unchecked(true)
            .output();
        assert_eq!(
            run.exit_status.code(),
            Some(NextestExitCode::TEST_RUN_FAILED),
            "{run}"
        );
        let statuses = recorded_statuses(&project, mode);
        assert_eq!(
            statuses[0].last_status().result,
            ExecutionResultDescription::ExecFail
        );
        let replay = cli(&env, &project, mode)
            .args(["replay", "-R", RUN_ID])
            .output();
        for (output, text) in [
            (&run, run.stderr_as_str()),
            (&replay, replay.stdout_as_str()),
        ] {
            assert!(
                text.contains("error reading run wrapper report"),
                "{output}"
            );
            assert!(text.contains("1 exec failed"), "{output}");
        }
        check_report_cleanup(&project, mode, 1);
    }
}

#[test]
fn wrapper_report_errors_preserve_test_failures() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    let run = cli(&env, &project, "invalid")
        .args(["run", "-E", "binary(=basic) & test(=test_failure_assert)"])
        .unchecked(true)
        .output();
    assert_eq!(
        run.exit_status.code(),
        Some(NextestExitCode::TEST_RUN_FAILED),
        "{run}"
    );
    let statuses = recorded_statuses(&project, "invalid");
    assert!(matches!(
        statuses[0].last_status().result,
        ExecutionResultDescription::Fail { .. }
    ));
    assert!(
        run.stderr_as_str()
            .contains("error reading run wrapper report"),
        "{run}"
    );
}

#[test]
fn wrapper_reports_are_isolated_between_retries() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    for mode in ["retry", "retry-absent"] {
        let run = cli(&env, &project, mode)
            .args(["run", "-E", SUCCESS_FILTER, "--retries", "1"])
            .output();
        let statuses = recorded_statuses(&project, mode);
        assert_eq!(statuses[0].len(), 2);
        let report = statuses[0].last_status().run_wrapper_report.as_ref();
        assert_eq!(report.is_some(), mode == "retry");
        assert_eq!(
            run.stderr_as_str()
                .contains("1 passed (1 flaky; wrapper: 1 wrapped)"),
            mode == "retry",
            "{run}"
        );
        assert!(
            !run.stderr_as_str().contains("1 previous"),
            "only the final attempt contributes to the summary: {run}"
        );
        let replay = cli(&env, &project, mode)
            .args(["replay", "-R", RUN_ID])
            .output();
        assert_eq!(
            replay
                .stdout_as_str()
                .contains("1 passed (1 flaky; wrapper: 1 wrapped)"),
            mode == "retry",
            "{replay}"
        );
        check_report_cleanup(&project, mode, 2);
    }
}

#[test]
fn wrapper_reports_are_isolated_between_stress_runs() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    let run = cli(&env, &project, "valid")
        .args(["run", "-E", SUCCESS_FILTER, "--stress-count", "2"])
        .output();
    assert_eq!(
        run.stderr_as_str()
            .matches("1 passed (wrapper: 1 wrapped)")
            .count(),
        2,
        "{run}"
    );
    assert_eq!(recorded_statuses(&project, "valid").len(), 2);
    check_report_cleanup(&project, "valid", 2);
}

#[test]
fn target_runner_can_override_report_wrapper() {
    let env = set_env_vars_for_test();
    let project = TempProject::new(&env).unwrap();
    let runner = shell_words::join([env.passthrough_bin.as_str(), "--ensure-this-arg-is-sent"]);
    let run = cli(&env, &project, "invalid")
        .env(current_runner_env_var(), runner)
        .args(["run", "-E", SUCCESS_FILTER])
        .output();
    assert!(run.exit_status.success(), "{run}");
    assert!(
        recorded_statuses(&project, "invalid")[0]
            .last_status()
            .run_wrapper_report
            .is_none()
    );
    check_report_cleanup(&project, "invalid", 0);
}
