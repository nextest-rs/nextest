// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::Result;
use duct::cmd;
use guppy::{graph::PackageGraph, MetadataCommand};
use maplit::btreemap;
use nextest_metadata::{EnvironmentMap, FilterMatch, MismatchReason};
use nextest_runner::{
    cargo_config::CargoConfigs,
    config::NextestConfig,
    list::{BinaryList, RustBuildMeta, RustTestArtifact, TestList, TestListState},
    reporter::TestEvent,
    reuse_build::PathMapper,
    runner::{
        configure_handle_inheritance, AbortStatus, ExecutionResult, ExecutionStatuses, RunStats,
        TestRunner,
    },
    target_runner::TargetRunner,
    test_filter::TestFilterBuilder,
};
use once_cell::sync::{Lazy, OnceCell};
use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    io::Cursor,
    sync::{Arc, Mutex},
};

#[derive(Copy, Clone, Debug)]
pub(crate) struct TestFixture {
    pub(crate) name: &'static str,
    pub(crate) status: FixtureStatus,
}

impl PartialEq<(&str, FilterMatch)> for TestFixture {
    fn eq(&self, (name, filter_match): &(&str, FilterMatch)) -> bool {
        &self.name == name && self.status.is_ignored() != filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum FixtureStatus {
    Pass,
    Fail,
    Flaky { pass_attempt: usize },
    Leak,
    Segfault,
    IgnoredPass,
    IgnoredFail,
}

impl FixtureStatus {
    pub(crate) fn to_test_status(self, total_attempts: usize) -> ExecutionResult {
        match self {
            FixtureStatus::Pass | FixtureStatus::IgnoredPass => ExecutionResult::Pass,
            FixtureStatus::Flaky { pass_attempt } => {
                if pass_attempt <= total_attempts {
                    ExecutionResult::Pass
                } else {
                    ExecutionResult::Fail {
                        abort_status: None,
                        leaked: false,
                    }
                }
            }
            FixtureStatus::Segfault => {
                cfg_if::cfg_if! {
                    if #[cfg(unix)] {
                        // SIGSEGV is 11.
                        let abort_status = Some(AbortStatus::UnixSignal(11));
                    } else if #[cfg(windows)] {
                        // A segfault is an access violation on Windows.
                        let abort_status = Some(AbortStatus::WindowsNtStatus(
                            windows::Win32::Foundation::STATUS_ACCESS_VIOLATION,
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
            FixtureStatus::Fail | FixtureStatus::IgnoredFail => ExecutionResult::Fail {
                abort_status: None,
                leaked: false,
            },
            FixtureStatus::Leak => ExecutionResult::Leak,
        }
    }

    pub(crate) fn is_ignored(self) -> bool {
        matches!(
            self,
            FixtureStatus::IgnoredPass | FixtureStatus::IgnoredFail
        )
    }
}

pub(crate) static EXPECTED_TESTS: Lazy<BTreeMap<&'static str, Vec<TestFixture>>> = Lazy::new(
    || {
        btreemap! {
            // Integration tests
            "nextest-tests::basic" => vec![
                TestFixture { name: "test_cargo_env_vars", status: FixtureStatus::Pass },
                TestFixture { name: "test_cwd", status: FixtureStatus::Pass },
                TestFixture { name: "test_execute_bin", status: FixtureStatus::Pass },
                TestFixture { name: "test_failure_assert", status: FixtureStatus::Fail },
                TestFixture { name: "test_failure_error", status: FixtureStatus::Fail },
                TestFixture { name: "test_failure_should_panic", status: FixtureStatus::Fail },
                TestFixture { name: "test_flaky_mod_4", status: FixtureStatus::Flaky { pass_attempt: 4 } },
                TestFixture { name: "test_flaky_mod_6", status: FixtureStatus::Flaky { pass_attempt: 6 } },
                TestFixture { name: "test_ignored", status: FixtureStatus::IgnoredPass },
                TestFixture { name: "test_ignored_fail", status: FixtureStatus::IgnoredFail },
                TestFixture { name: "test_result_failure", status: FixtureStatus::Fail },
                TestFixture { name: "test_slow_timeout", status: FixtureStatus::IgnoredPass },
                TestFixture { name: "test_slow_timeout_2", status: FixtureStatus::IgnoredPass },
                TestFixture { name: "test_slow_timeout_subprocess", status: FixtureStatus::IgnoredPass },
                TestFixture { name: "test_stdin_closed", status: FixtureStatus::Pass },
                TestFixture { name: "test_subprocess_doesnt_exit", status: FixtureStatus::Leak },
                TestFixture { name: "test_success", status: FixtureStatus::Pass },
                TestFixture { name: "test_success_should_panic", status: FixtureStatus::Pass },
            ],
            "nextest-tests::other" => vec![
                TestFixture { name: "other_test_success", status: FixtureStatus::Pass },
            ],
            "nextest-tests::segfault" => vec![
                TestFixture { name: "test_segfault", status: FixtureStatus::Segfault },
            ],
            // Unit tests
            "nextest-tests" => vec![
                TestFixture { name: "tests::call_dylib_add_two", status: FixtureStatus::Pass },
                TestFixture { name: "tests::unit_test_success", status: FixtureStatus::Pass },
            ],
            // Binary tests
            "nextest-tests::bin/nextest-tests" => vec![
                TestFixture { name: "tests::bin_success", status: FixtureStatus::Pass },
            ],
            "nextest-tests::bin/other" => vec![
                TestFixture { name: "tests::other_bin_success", status: FixtureStatus::Pass },
            ],
            // Example tests
            "nextest-tests::example/nextest-tests" => vec![
                TestFixture { name: "tests::example_success", status: FixtureStatus::Pass },
            ],
            "nextest-tests::example/other" => vec![
                TestFixture { name: "tests::other_example_success", status: FixtureStatus::Pass },
            ],
            // Benchmarks
            "nextest-tests::bench/my-bench" => vec![
                TestFixture { name: "bench_add_two", status: FixtureStatus::Pass },
                TestFixture { name: "tests::test_execute_bin", status: FixtureStatus::Pass },
            ],
            // Proc-macro tests
            "nextest-derive::proc-macro/nextest-derive" => vec![
                TestFixture { name: "it_works", status: FixtureStatus::Pass },
            ],
            // Dynamic library tests
            "cdylib-link" => vec![
                TestFixture { name: "test_multiply_two", status: FixtureStatus::Pass },
            ],
            "dylib-test" => vec![],
            "cdylib-example" => vec![
                TestFixture { name: "tests::test_multiply_two_cdylib", status: FixtureStatus::Pass },
            ]
        }
    },
);

pub(crate) fn get_expected_test(binary_id: &str, test_name: &str) -> &'static TestFixture {
    let v = EXPECTED_TESTS
        .get(binary_id)
        .unwrap_or_else(|| panic!("binary id {binary_id} not found"));
    v.iter()
        .find(|fixture| fixture.name == test_name)
        .unwrap_or_else(|| panic!("for binary id {binary_id}, test name {test_name} not found"))
}

pub(crate) static EXPECTED_BINARY_LIST: [(&str, &str, bool); 8] = [
    (
        "nextest-derive::proc-macro/nextest-derive",
        "nextest-derive",
        false,
    ),
    ("nextest-tests", "nextest-tests", true),
    ("nextest-tests::basic", "basic", true),
    ("nextest-tests::bin/nextest-tests", "nextest-tests", true),
    ("nextest-tests::bin/other", "other", true),
    (
        "nextest-tests::example/nextest-tests",
        "nextest-tests",
        true,
    ),
    ("nextest-tests::example/other", "other", true),
    ("nextest-tests::other", "other", true),
];

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
}

pub(crate) fn workspace_root() -> Utf8PathBuf {
    // one level up from the manifest dir -> into fixtures/nextest-tests
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

pub(crate) fn load_config() -> NextestConfig {
    NextestConfig::from_sources(workspace_root(), &*PACKAGE_GRAPH, None, [])
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
        Err(err) => panic!("error obtaining CARGO env var: {}", err),
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
    pub(crate) test_artifacts: BTreeMap<String, RustTestArtifact<'static>>,
    pub(crate) env: EnvironmentMap,
}

impl FixtureTargets {
    fn new() -> Self {
        let graph = &*PACKAGE_GRAPH;
        let cargo_configs = CargoConfigs::new_with_isolation(
            [workspace_root().join(".cargo/extra-config.toml")],
            &workspace_root(),
            &workspace_root(),
        )
        .unwrap();
        let env = cargo_configs.env().unwrap();
        let binary_list = Arc::new(
            BinaryList::from_messages(Cursor::new(&*FIXTURE_RAW_CARGO_TEST_OUTPUT), graph, None)
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
    ) -> TestList<'_> {
        let test_bins: Vec<_> = self.test_artifacts.values().cloned().collect();
        TestList::new(
            test_bins,
            self.rust_build_meta.clone(),
            test_filter,
            target_runner,
            self.env.to_owned(),
            num_cpus::get(),
        )
        .expect("test list successfully created")
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
            InstanceStatus::Skipped(reason) => write!(f, "skipped: {}", reason),
            InstanceStatus::Finished(run_statuses) => {
                for run_status in run_statuses.iter() {
                    write!(
                        f,
                        "({}/{}) {:?}\n---STDOUT---\n{}\n\n---STDERR---\n{}\n\n",
                        run_status.retry_data.attempt,
                        run_status.retry_data.total_attempts,
                        run_status.result,
                        String::from_utf8_lossy(&run_status.stdout),
                        String::from_utf8_lossy(&run_status.stderr)
                    )?;
                }
                Ok(())
            }
        }
    }
}

pub(crate) fn execute_collect<'a>(
    runner: &mut TestRunner<'a>,
) -> (
    HashMap<(&'a Utf8Path, &'a str), InstanceValue<'a>>,
    RunStats,
) {
    let mut instance_statuses = HashMap::new();
    configure_handle_inheritance(false).expect("configuring handle inheritance on Windows failed");
    let run_stats = runner.execute(|event| {
        let (test_instance, status) = match event {
            TestEvent::TestSkipped {
                test_instance,
                reason,
            } => (test_instance, InstanceStatus::Skipped(reason)),
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => (test_instance, InstanceStatus::Finished(run_statuses)),
            _ => return,
        };

        instance_statuses.insert(
            (test_instance.binary, test_instance.name),
            InstanceValue {
                binary_id: test_instance.suite_info.binary_id.as_str(),
                cwd: test_instance.suite_info.cwd.as_path(),
                status,
            },
        );
    });

    (instance_statuses, run_stats)
}

fn env_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceCell<Mutex<()>> = OnceCell::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn with_env<T>(
    vars: impl IntoIterator<Item = (impl Into<String>, impl AsRef<str>)>,
    func: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = env_mutex().lock().unwrap();

    let keys: Vec<_> = vars
        .into_iter()
        .map(|(key, val)| {
            let key = key.into();
            env::set_var(&key, val.as_ref());
            key
        })
        .collect();

    let res = func();

    for key in keys {
        env::remove_var(key);
    }
    drop(lock);

    res
}
