// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Basic tests for the test runner.

use anyhow::Result;
use camino::Utf8Path;
use cargo_metadata::Message;
use duct::cmd;
use maplit::btreemap;
use once_cell::sync::Lazy;
use pretty_assertions::assert_eq;
use std::{
    collections::{BTreeMap, HashMap},
    env, fmt,
    io::Cursor,
};
use testrunner::{
    reporter::TestEvent,
    runner::{TestRunStatus, TestRunnerOpts},
    test_filter::{FilterMatch, RunIgnored, TestFilter},
    test_list::{TestBinary, TestList},
};

#[derive(Copy, Clone, Debug)]
struct TestFixture {
    name: &'static str,
    status: FixtureStatus,
}

impl PartialEq<(&str, FilterMatch)> for TestFixture {
    fn eq(&self, (name, filter_match): &(&str, FilterMatch)) -> bool {
        &self.name == name && self.status.is_skip() != filter_match.is_match()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum FixtureStatus {
    Pass,
    Fail,
    Skip,
}

impl FixtureStatus {
    fn is_skip(self) -> bool {
        matches!(self, FixtureStatus::Skip)
    }
}

static EXPECTED_TESTS: Lazy<BTreeMap<&'static str, Vec<TestFixture>>> = Lazy::new(|| {
    btreemap! {
        "basic" => vec![
            TestFixture { name: "test_cwd", status: FixtureStatus::Pass },
            TestFixture { name: "test_failure_assert", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_error", status: FixtureStatus::Fail },
            TestFixture { name: "test_failure_should_panic", status: FixtureStatus::Fail },
            TestFixture { name: "test_ignored", status: FixtureStatus::Skip },
            TestFixture { name: "test_success", status: FixtureStatus::Pass },
            TestFixture { name: "test_success_should_panic", status: FixtureStatus::Pass },
        ],
        "testrunner-tests" => vec![
            TestFixture { name: "tests::unit_test_success", status: FixtureStatus::Pass },
        ],
    }
});

static FIXTURE_TARGETS: Lazy<BTreeMap<String, TestBinary>> = Lazy::new(init_fixture_targets);

fn init_fixture_targets() -> BTreeMap<String, TestBinary> {
    // TODO: actually productionize this, probably requires moving x into this repo
    let cmd_name = match env::var("CARGO") {
        Ok(v) => v,
        Err(env::VarError::NotPresent) => "cargo".to_owned(),
        Err(err) => panic!("error obtaining CARGO env var: {}", err),
    };

    let dir_name = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/testrunner-tests");

    let expr = cmd!(cmd_name, "test", "--no-run", "--message-format", "json")
        .dir(&dir_name)
        .stdout_capture();
    let output = expr.run().expect("cargo test --no-run failed");
    // stdout has the JSON messages
    let messages = Cursor::new(output.stdout);

    let mut targets = BTreeMap::new();
    for message in Message::parse_stream(messages) {
        let message = message.expect("parsing message off output stream should succeed");
        if let Message::CompilerArtifact(artifact) = message {
            println!("build artifact: {:?}", artifact);
            if let Some(binary) = artifact.executable {
                let mut cwd = artifact.target.src_path;
                // Pop two levels to get the manifest dir.
                cwd.pop();
                cwd.pop();

                let cwd = Some(cwd);
                targets.insert(
                    artifact.target.name,
                    TestBinary {
                        binary,
                        cwd,
                        friendly_name: Some("my-friendly-name".into()),
                    },
                );
            }
        } else if let Message::TextLine(line) = message {
            println!("{}", line);
        }
    }

    targets
}

#[test]
fn test_list_tests() -> Result<()> {
    let test_filter = TestFilter::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter)?;

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        let info = test_list
            .get(&test_binary.binary)
            .unwrap_or_else(|| panic!("test list not found for {}", test_binary.binary));
        let tests: Vec<_> = info
            .tests
            .iter()
            .map(|(name, info)| (name.as_str(), info.filter_match))
            .collect();
        assert_eq!(expected, &tests, "test list matches");
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct InstanceValue<'a> {
    friendly_name: Option<&'a str>,
    cwd: Option<&'a Utf8Path>,
    status: InstanceStatus,
}

#[derive(Clone)]
enum InstanceStatus {
    Skipped,
    Finished(TestRunStatus),
}

impl fmt::Debug for InstanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            InstanceStatus::Skipped => write!(f, "<skipped>"),
            InstanceStatus::Finished(run_status) => {
                write!(
                    f,
                    "---STDOUT---\n{}\n\n---STDERR---\n{}\n\n",
                    String::from_utf8_lossy(&run_status.stdout),
                    String::from_utf8_lossy(&run_status.stderr)
                )
            }
        }
    }
}

impl PartialEq<FixtureStatus> for InstanceStatus {
    fn eq(&self, other: &FixtureStatus) -> bool {
        match self {
            InstanceStatus::Skipped => other.is_skip(),
            InstanceStatus::Finished(status) => {
                status.status.is_success() == (other == &FixtureStatus::Pass)
            }
        }
    }
}

#[test]
fn test_run() -> Result<()> {
    let test_filter = TestFilter::any(RunIgnored::Default);
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter)?;
    let runner = TestRunnerOpts::default().build(&test_list);
    let mut instance_statuses = HashMap::new();
    let run_stats = runner.execute(|event| {
        println!("*** event: {:?}", event);

        let (test_instance, status) = match event {
            TestEvent::TestSkipped { test_instance } => (test_instance, InstanceStatus::Skipped),
            TestEvent::TestFinished {
                test_instance,
                run_status,
            } => (test_instance, InstanceStatus::Finished(run_status)),
            _ => return,
        };

        instance_statuses.insert(
            (test_instance.binary, test_instance.name),
            InstanceValue {
                friendly_name: test_instance.friendly_name,
                cwd: test_instance.cwd,
                status,
            },
        );
    });

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        for fixture in expected {
            let instance_value = &instance_statuses[&(test_binary.binary.as_path(), fixture.name)];
            assert_eq!(
                instance_value.status, fixture.status,
                "for {}, test {}, status matches",
                name, fixture.name,
            );
        }
    }

    assert!(!run_stats.is_success(), "run should be marked failed");
    Ok(())
}
