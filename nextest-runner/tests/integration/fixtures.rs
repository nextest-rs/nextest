// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{Context, Result};
use duct::cmd;
use fixture_data::models::TestCaseFixtureStatus;
use guppy::{graph::PackageGraph, MetadataCommand};
use maplit::btreeset;
use nextest_filtering::{CompiledExpr, EvalContext};
use nextest_metadata::{MismatchReason, RustBinaryId};
use nextest_runner::{
    cargo_config::{CargoConfigs, EnvironmentMap},
    config::{get_num_cpus, ConfigExperimental, NextestConfig},
    double_spawn::DoubleSpawnInfo,
    list::{
        BinaryList, RustBuildMeta, RustTestArtifact, TestExecuteContext, TestList, TestListState,
    },
    platform::BuildPlatforms,
    reporter::TestEventKind,
    reuse_build::PathMapper,
    runner::{
        configure_handle_inheritance, AbortStatus, ExecutionResult, ExecutionStatuses, RunStats,
        TestRunner,
    },
    target_runner::TargetRunner,
    test_filter::{FilterBound, TestFilterBuilder},
    test_output::TestOutput,
};
use once_cell::sync::Lazy;
use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    io::Cursor,
    sync::Arc,
};

pub(crate) fn make_execution_result(
    status: TestCaseFixtureStatus,
    total_attempts: usize,
) -> ExecutionResult {
    match status {
        TestCaseFixtureStatus::Pass | TestCaseFixtureStatus::IgnoredPass => ExecutionResult::Pass,
        TestCaseFixtureStatus::Flaky { pass_attempt } => {
            if pass_attempt <= total_attempts {
                ExecutionResult::Pass
            } else {
                ExecutionResult::Fail {
                    abort_status: None,
                    leaked: false,
                }
            }
        }
        TestCaseFixtureStatus::Segfault => {
            cfg_if::cfg_if! {
                if #[cfg(unix)] {
                    // SIGSEGV is 11.
                    let abort_status = Some(AbortStatus::UnixSignal(11));
                } else if #[cfg(windows)] {
                    // A segfault is an access violation on Windows.
                    let abort_status = Some(AbortStatus::WindowsNtStatus(
                        windows_sys::Win32::Foundation::STATUS_ACCESS_VIOLATION,
                    ));
                } else {
                    let abort_status = None;
                }
            }
            ExecutionResult::Fail {
                abort_status,
                leaked: false,
            }
        }
        TestCaseFixtureStatus::Fail | TestCaseFixtureStatus::IgnoredFail => ExecutionResult::Fail {
            abort_status: None,
            leaked: false,
        },
        TestCaseFixtureStatus::Leak => ExecutionResult::Leak,
    }
}

#[track_caller]
pub(crate) fn set_env_vars() {
    // The dynamic library tests require this flag.
    std::env::set_var("RUSTFLAGS", "-C prefer-dynamic");

    std::env::set_var(
        "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_NO_OVERRIDE",
        "test-PASSED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_OVERRIDDEN",
        "test-FAILED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_RELATIVE_NO_OVERRIDE",
        "test-PASSED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_RELATIVE_OVERRIDDEN",
        "test-FAILED-value-set-by-environment",
    );

    std::env::set_var(
        "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_EXTRA",
        "test-FAILED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_MAIN",
        "test-PASSED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_BOTH",
        "test-FAILED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_NONE",
        "test-PASSED-value-set-by-environment",
    );
    std::env::set_var(
        "__NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_FALSE",
        "test-PASSED-value-set-by-environment",
    );

    // Remove OUT_DIR from the environment, as it interferes with tests (some of them expect that
    // OUT_DIR isn't set.)
    std::env::remove_var("OUT_DIR");
}

pub(crate) fn workspace_root() -> Utf8PathBuf {
    // one level up from the manifest dir -> into fixtures/nextest-tests
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

pub(crate) fn load_config() -> NextestConfig {
    NextestConfig::from_sources(
        workspace_root(),
        &PACKAGE_GRAPH,
        None,
        [],
        // Enable setup scripts.
        &btreeset! { ConfigExperimental::SetupScripts },
    )
    .expect("loaded fixture config")
}

pub(crate) static PACKAGE_GRAPH: Lazy<PackageGraph> = Lazy::new(|| {
    let mut metadata_command = MetadataCommand::new();
    // Construct a package graph with --no-deps since we don't need full dependency
    // information.
    metadata_command
        .manifest_path(workspace_root().join("Cargo.toml"))
        .no_deps()
        .build_graph()
        .expect("building package graph failed")
});

pub(crate) static FIXTURE_RAW_CARGO_TEST_OUTPUT: Lazy<Vec<u8>> =
    Lazy::new(init_fixture_raw_cargo_test_output);

fn init_fixture_raw_cargo_test_output() -> Vec<u8> {
    // This is a simple version of what cargo does.
    let cmd_name = match env::var("CARGO") {
        Ok(v) => v,
        Err(env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {err}"),
    };

    let expr = cmd!(
        cmd_name,
        "test",
        "--no-run",
        "--workspace",
        "--message-format",
        "json-render-diagnostics",
        "--all-targets",
    )
    // This environment variable is required to test the #[bench] fixture. Note that THIS IS FOR
    // TEST CODE ONLY. NEVER USE THIS IN PRODUCTION.
    .env("RUSTC_BOOTSTRAP", "1")
    .dir(workspace_root())
    .stdout_capture();

    let output = expr.run().expect("cargo test --no-run failed");
    output.stdout
}

pub(crate) static FIXTURE_TARGETS: Lazy<FixtureTargets> = Lazy::new(FixtureTargets::new);

#[derive(Debug)]
pub(crate) struct FixtureTargets {
    pub(crate) rust_build_meta: RustBuildMeta<TestListState>,
    pub(crate) test_artifacts: BTreeMap<RustBinaryId, RustTestArtifact<'static>>,
    pub(crate) env: EnvironmentMap,
}

impl FixtureTargets {
    fn new() -> Self {
        let graph = &*PACKAGE_GRAPH;
        let cargo_configs = CargoConfigs::new_with_isolation(
            [workspace_root().join(".cargo/extra-config.toml")],
            &workspace_root(),
            &workspace_root(),
            Vec::new(),
        )
        .unwrap();
        let env = EnvironmentMap::new(&cargo_configs);
        let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
        let binary_list = Arc::new(
            BinaryList::from_messages(
                Cursor::new(&*FIXTURE_RAW_CARGO_TEST_OUTPUT),
                graph,
                build_platforms,
            )
            .unwrap(),
        );
        let rust_build_meta = binary_list.rust_build_meta.clone();

        let path_mapper = PathMapper::noop();
        let rust_build_meta = rust_build_meta.map_paths(&path_mapper);

        let test_artifacts = RustTestArtifact::from_binary_list(
            graph,
            binary_list,
            &rust_build_meta,
            &path_mapper,
            None,
        )
        .unwrap();

        let test_artifacts = test_artifacts
            .into_iter()
            .map(|bin| (bin.binary_id.clone(), bin))
            .collect();
        Self {
            rust_build_meta,
            test_artifacts,
            env,
        }
    }

    pub(crate) fn make_test_list(
        &self,
        test_filter: &TestFilterBuilder,
        target_runner: &TargetRunner,
    ) -> Result<TestList<'_>> {
        let test_bins: Vec<_> = self.test_artifacts.values().cloned().collect();
        let double_spawn = DoubleSpawnInfo::disabled();
        let ctx = TestExecuteContext {
            double_spawn: &double_spawn,
            target_runner,
        };
        let ecx = EvalContext {
            default_filter: &CompiledExpr::ALL,
        };

        TestList::new(
            &ctx,
            test_bins,
            self.rust_build_meta.clone(),
            test_filter,
            workspace_root(),
            self.env.to_owned(),
            &ecx,
            FilterBound::All,
            get_num_cpus(),
        )
        .context("Failed to make test list")
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct InstanceValue<'a> {
    pub(crate) binary_id: &'a str,
    pub(crate) cwd: &'a Utf8Path,
    pub(crate) status: InstanceStatus,
}

#[derive(Clone)]
pub(crate) enum InstanceStatus {
    Skipped(MismatchReason),
    Finished(ExecutionStatuses),
}

impl fmt::Debug for InstanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            InstanceStatus::Skipped(reason) => write!(f, "skipped: {reason}"),
            InstanceStatus::Finished(run_statuses) => {
                for run_status in run_statuses.iter() {
                    let Some(TestOutput::Split { stdout, stderr }) = &run_status.output else {
                        panic!("this test should always use split output")
                    };
                    write!(
                        f,
                        "({}/{}) {:?}\n---STDOUT---\n{}\n\n---STDERR---\n{}\n\n",
                        run_status.retry_data.attempt,
                        run_status.retry_data.total_attempts,
                        run_status.result,
                        stdout.to_str_lossy(),
                        stderr.to_str_lossy(),
                    )?;
                }
                Ok(())
            }
        }
    }
}

pub(crate) fn execute_collect(
    runner: TestRunner<'_>,
) -> (
    HashMap<(&'_ Utf8Path, &'_ str), InstanceValue<'_>>,
    RunStats,
) {
    let mut instance_statuses = HashMap::new();
    configure_handle_inheritance(false).expect("configuring handle inheritance on Windows failed");
    let run_stats = runner.execute(|event| {
        let (test_instance, status) = match event.kind {
            TestEventKind::TestSkipped {
                test_instance,
                reason,
            } => (test_instance, InstanceStatus::Skipped(reason)),
            TestEventKind::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => (test_instance, InstanceStatus::Finished(run_statuses)),
            _ => return,
        };

        instance_statuses.insert(
            (
                test_instance.suite_info.binary_path.as_path(),
                test_instance.name,
            ),
            InstanceValue {
                binary_id: test_instance.suite_info.binary_id.as_str(),
                cwd: test_instance.suite_info.cwd.as_path(),
                status,
            },
        );
    });

    (instance_statuses, run_stats)
}
