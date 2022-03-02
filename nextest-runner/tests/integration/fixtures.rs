// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Path, Utf8PathBuf};

use duct::cmd;
use guppy::{graph::PackageGraph, MetadataCommand};
use maplit::btreemap;
use nextest_metadata::{FilterMatch, MismatchReason};
use nextest_runner::{
    reporter::TestEvent,
    runner::{ExecutionResult, ExecutionStatuses, RunStats, TestRunner},
    test_list::{BinaryList, RustTestArtifact},
};
use once_cell::sync::Lazy;
use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    io::Cursor,
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
                    ExecutionResult::Fail
                }
            }
            FixtureStatus::Fail | FixtureStatus::IgnoredFail => ExecutionResult::Fail,
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
                TestFixture { name: "test_failure_assert", status: FixtureStatus::Fail },
                TestFixture { name: "test_failure_error", status: FixtureStatus::Fail },
                TestFixture { name: "test_failure_should_panic", status: FixtureStatus::Fail },
                TestFixture { name: "test_flaky_mod_2", status: FixtureStatus::Flaky { pass_attempt: 2 } },
                TestFixture { name: "test_flaky_mod_3", status: FixtureStatus::Flaky { pass_attempt: 3 } },
                TestFixture { name: "test_ignored", status: FixtureStatus::IgnoredPass },
                TestFixture { name: "test_ignored_fail", status: FixtureStatus::IgnoredFail },
                TestFixture { name: "test_success", status: FixtureStatus::Pass },
                TestFixture { name: "test_success_should_panic", status: FixtureStatus::Pass },
            ],
            "nextest-tests::other" => vec![
                TestFixture { name: "other_test_success", status: FixtureStatus::Pass },
            ],
            // Unit tests
            "nextest-tests" => vec![
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
            "nextest-derive::proc-macro/nextest-derive" => vec![
                TestFixture { name: "it_works", status: FixtureStatus::Pass },
            ],
        }
    },
);

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

pub(crate) fn workspace_root() -> Utf8PathBuf {
    // one level up from the manifest dir -> into fixtures/nextest-tests
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
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
    // TODO: actually productionize this, probably requires moving x into this repo
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
        "json-render-diagnostics"
    )
    .dir(workspace_root())
    .stdout_capture();

    let output = expr.run().expect("cargo test --no-run failed");
    output.stdout
}

pub(crate) static FIXTURE_TARGETS: Lazy<BTreeMap<String, RustTestArtifact<'static>>> =
    Lazy::new(init_fixture_targets);

fn init_fixture_targets() -> BTreeMap<String, RustTestArtifact<'static>> {
    let graph = &*PACKAGE_GRAPH;
    let binary_list =
        BinaryList::from_messages(Cursor::new(&*FIXTURE_RAW_CARGO_TEST_OUTPUT), graph).unwrap();
    let test_artifacts =
        RustTestArtifact::from_binary_list(graph, binary_list, None, None).unwrap();

    test_artifacts
        .into_iter()
        .map(|bin| (bin.binary_id.clone(), bin))
        .inspect(|(k, _)| println!("{}", k))
        .collect()
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
                        run_status.attempt,
                        run_status.total_attempts,
                        run_status.result,
                        String::from_utf8_lossy(run_status.stdout()),
                        String::from_utf8_lossy(run_status.stderr())
                    )?;
                }
                Ok(())
            }
        }
    }
}

pub(crate) fn execute_collect<'a>(
    runner: &TestRunner<'a>,
) -> (
    HashMap<(&'a Utf8Path, &'a str), InstanceValue<'a>>,
    RunStats,
) {
    let mut instance_statuses = HashMap::new();
    let run_stats = runner.execute(|event| {
        let (test_instance, status) = match event {
            TestEvent::TestSkipped {
                test_instance,
                reason,
            } => (test_instance, InstanceStatus::Skipped(reason)),
            TestEvent::TestFinished {
                test_instance,
                run_statuses,
            } => (test_instance, InstanceStatus::Finished(run_statuses)),
            _ => return,
        };

        instance_statuses.insert(
            (test_instance.binary, test_instance.name),
            InstanceValue {
                binary_id: test_instance.bin_info.binary_id.as_str(),
                cwd: test_instance.bin_info.cwd.as_path(),
                status,
            },
        );
    });

    (instance_statuses, run_stats)
}
