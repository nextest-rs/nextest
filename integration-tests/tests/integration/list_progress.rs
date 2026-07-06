// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for list progress.

use crate::temp_project::TempProject;
use anstyle_progress::TermProgress;
use integration_tests::{env::set_env_vars_for_test, nextest_cli::CargoNextestCli};

#[test]
fn test_list_emits_terminal_progress() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    let output = CargoNextestCli::for_test(&env_info)
        .env("__NEXTEST_LIST_PROGRESS_DELAY_MS", "0")
        .env("CARGO_TERM_PROGRESS_TERM_INTEGRATION", "true")
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
        ])
        .output();

    let completed = TermProgress::start().percent(100).to_string();
    let removed = TermProgress::remove().to_string();

    let Some(completed_pos) = find_subslice(&output.stderr, completed.as_bytes()) else {
        panic!(
            "stderr should contain an OSC 9;4 completion sequence:\n{}",
            output.stderr_as_str()
        );
    };
    let Some(removed_pos) = rfind_subslice(&output.stderr, removed.as_bytes()) else {
        panic!(
            "stderr should contain an OSC 9;4 remove sequence once listing finishes:\n{}",
            output.stderr_as_str()
        );
    };

    assert!(
        completed_pos < removed_pos,
        "the completion sequence (at byte {completed_pos}) should appear before the remove \
         sequence (at byte {removed_pos}):\n{}",
        output.stderr_as_str()
    );

    const OSC_9_4_INTRODUCER: &[u8] = b"\x1b]9;4;";
    let last_osc_pos = rfind_subslice(&output.stderr, OSC_9_4_INTRODUCER)
        .expect("an OSC 9;4 introducer exists, since the remove sequence was found above");
    assert_eq!(
        last_osc_pos,
        removed_pos,
        "the remove sequence should be the last OSC 9;4 emission on stderr:\n{}",
        output.stderr_as_str()
    );
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}
