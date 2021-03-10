// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Basic tests for the test runner.

use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::Message;
use duct::cmd;
use maplit::btreemap;
use once_cell::sync::Lazy;
use pretty_assertions::assert_eq;
use std::{collections::BTreeMap, env, io::Cursor};
use testrunner::{
    dispatch::{Opts, TestBinFilter},
    output::{OutputFormat, SerializableFormat},
    runner::TestRunnerOpts,
    test_list::TestList,
};

static EXPECTED_TESTS: Lazy<BTreeMap<&'static str, Vec<&'static str>>> = Lazy::new(|| {
    btreemap! {
        "basic" => vec![
            "test_failure_assert",
            "test_failure_error",
            "test_failure_should_panic",
            "test_success",
            "test_success_should_panic",
        ],
        "testrunner-tests" => vec!["tests::unit_test_success"],
    }
});

static FIXTURE_TARGETS: Lazy<BTreeMap<String, Utf8PathBuf>> = Lazy::new(init_fixture_targets);

fn init_fixture_targets() -> BTreeMap<String, Utf8PathBuf> {
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
            if let Some(executable) = artifact.executable {
                targets.insert(artifact.target.name, executable);
            }
        }
    }

    targets
}

#[test]
fn test_list_tests() {
    let opts = Opts::ListTests {
        bin_filter: TestBinFilter {
            test_bin: FIXTURE_TARGETS.values().cloned().collect(),
            filter: vec![],
        },
        format: OutputFormat::Serializable(SerializableFormat::Json),
    };

    let mut out: Vec<u8> = vec![];
    opts.exec(&mut out).expect("execution was successful");
    let out = String::from_utf8(out).expect("invalid utf8");
    let test_list: TestList = serde_json::from_str(&out).expect("JSON parsing successful");

    for (name, expected) in &*EXPECTED_TESTS {
        let path = FIXTURE_TARGETS
            .get(*name)
            .unwrap_or_else(|| panic!("unexpected test name {}", name));
        let actual = test_list
            .get(path)
            .unwrap_or_else(|| panic!("test list not found for {}", path));
        assert_eq!(expected, actual, "test list matches");
    }
}

#[test]
fn test_run() {
    let opts = Opts::Run {
        bin_filter: TestBinFilter {
            test_bin: FIXTURE_TARGETS.values().cloned().collect(),
            filter: vec![],
        },
        opts: TestRunnerOpts::default(),
    };

    let mut out: Vec<u8> = vec![];
    opts.exec(&mut out).expect("execution was successful");
    // TODO: expand this test, check results and outputs
    println!("{}", String::from_utf8_lossy(&out));
}
