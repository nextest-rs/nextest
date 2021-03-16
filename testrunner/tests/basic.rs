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
    env,
    io::Cursor,
};
use testrunner::{
    dispatch::{Opts, TestBinFilter},
    output::{OutputFormat, SerializableFormat},
    runner::{TestRunnerOpts, TestStatus},
    test_filter::TestFilter,
    test_list::{TestBinary, TestInstance, TestList},
};

#[derive(Copy, Clone, Debug)]
struct TestFixture {
    name: &'static str,
    status: TestStatus,
}

impl PartialEq<String> for TestFixture {
    fn eq(&self, other: &String) -> bool {
        self.name == other
    }
}

static EXPECTED_TESTS: Lazy<BTreeMap<&'static str, Vec<TestFixture>>> = Lazy::new(|| {
    btreemap! {
        "basic" => vec![
            TestFixture { name: "test_cwd", status: TestStatus::Success },
            TestFixture { name: "test_failure_assert", status: TestStatus::Failure },
            TestFixture { name: "test_failure_error", status: TestStatus::Failure },
            TestFixture { name: "test_failure_should_panic", status: TestStatus::Failure },
            TestFixture { name: "test_success", status: TestStatus::Success },
            TestFixture { name: "test_success_should_panic", status: TestStatus::Success },
        ],
        "testrunner-tests" => vec![
            TestFixture { name: "tests::unit_test_success", status: TestStatus::Success },
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
                targets.insert(artifact.target.name, TestBinary { binary, cwd });
            }
        } else if let Message::TextLine(line) = message {
            println!("{}", line);
        }
    }

    targets
}

#[test]
fn test_list_tests() {
    let opts = Opts::ListTests {
        bin_filter: TestBinFilter {
            test_bin: FIXTURE_TARGETS
                .values()
                .map(|test_binary| test_binary.binary.clone())
                .collect(),
            filter: vec![],
        },
        format: OutputFormat::Serializable(SerializableFormat::Json),
    };

    let mut out: Vec<u8> = vec![];
    opts.exec(&mut out).expect("execution was successful");
    let out = String::from_utf8(out).expect("invalid utf8");
    let test_list: TestList = serde_json::from_str(&out).expect("JSON parsing successful");

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        let info = test_list
            .get(&test_binary.binary)
            .unwrap_or_else(|| panic!("test list not found for {}", test_binary.binary));
        assert_eq!(expected, &info.test_names, "test list matches");
    }
}

#[test]
fn test_run() -> Result<()> {
    let test_filter = TestFilter::any();
    let test_bins: Vec<_> = FIXTURE_TARGETS.values().cloned().collect();
    let test_list = TestList::new(test_bins, &test_filter)?;
    let runner = TestRunnerOpts::default().build(&test_list);
    let mut instance_statuses = HashMap::new();
    runner.execute(|test_instance, run_status| {
        instance_statuses.insert(test_instance, run_status);
    });

    for (name, expected) in &*EXPECTED_TESTS {
        let test_binary = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        for fixture in expected {
            let instance = TestInstance {
                binary: &test_binary.binary,
                test_name: fixture.name,
                cwd: test_binary.cwd.as_deref(),
            };
            let run_status = &instance_statuses[&instance];
            assert_eq!(
                run_status.status,
                fixture.status,
                "for {}, test {}, status matches\n\n---STDOUT---\n{}\n\n---STDERR---\n{}\n\n",
                name,
                fixture.name,
                String::from_utf8_lossy(&run_status.stdout),
                String::from_utf8_lossy(&run_status.stderr)
            );
        }
    }

    Ok(())
}
