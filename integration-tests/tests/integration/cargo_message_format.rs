// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for `--cargo-message-format`.

use crate::temp_project::TempProject;
use integration_tests::{env::set_env_vars_for_test, nextest_cli::CargoNextestCli};
use serde_json::Value;

/// The deprecation warning message that should appear in compiler output.
const DEPRECATION_WARNING_SUBSTRING: &str = "use of deprecated function `deprecated_function`";

/// The note that appears in the warning about the deprecation reason.
const DEPRECATION_NOTE_SUBSTRING: &str = "this is a test warning for --cargo-message-format";

/// ANSI escape sequence prefix.
const ANSI_ESCAPE: &str = "\x1b[";

/// Tests human format (both default and explicit), with and without color.
#[test]
fn test_cargo_message_format_human() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Test 1: Default (no --cargo-message-format) without forced color.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
        ])
        .output();

    let stderr = output.stderr_as_str();
    let stdout = output.stdout_as_str();

    // Human format: diagnostics should appear on stderr, rendered by Cargo.
    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain deprecation warning:\n{stderr}"
    );
    assert!(
        stderr.contains(DEPRECATION_NOTE_SUBSTRING),
        "stderr should contain deprecation note:\n{stderr}"
    );
    // stdout should be the test list JSON, not cargo messages.
    assert!(
        !stdout.contains("compiler-message"),
        "stdout should not contain compiler-message JSON:\n{stdout}"
    );
    // Without --color=always, ANSI codes should not be present (CARGO_TERM_COLOR=never is set).
    assert!(
        !stderr.contains(ANSI_ESCAPE),
        "stderr should not contain ANSI codes without --color=always:\n{stderr}"
    );

    // Test 2: Explicit --cargo-message-format human with --color=always.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "--color=always",
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "human",
        ])
        .output();

    let stderr = output.stderr_as_str();

    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain deprecation warning:\n{stderr}"
    );
    assert!(
        stderr.contains(DEPRECATION_NOTE_SUBSTRING),
        "stderr should contain deprecation note:\n{stderr}"
    );
    // With --color=always, ANSI codes should be present.
    assert!(
        stderr.contains(ANSI_ESCAPE),
        "stderr should contain ANSI codes with --color=always:\n{stderr}"
    );
}

/// Tests short format (alias for human).
///
/// `cargo test --message-format short` produces the same output as human, so
/// `--cargo-message-format short` is treated as an alias for human.
#[test]
fn test_cargo_message_format_short() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Short is an alias for human - Cargo renders diagnostics to stderr.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "short",
        ])
        .output();

    let stderr = output.stderr_as_str();
    let stdout = output.stdout_as_str();

    // Diagnostics should appear on stderr (rendered by Cargo).
    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain deprecation warning:\n{stderr}"
    );
    assert!(
        stderr.contains(DEPRECATION_NOTE_SUBSTRING),
        "stderr should contain deprecation note:\n{stderr}"
    );
    // stdout should be the test list JSON, not cargo messages.
    assert!(
        !stdout.contains("compiler-message"),
        "stdout should not contain compiler-message JSON:\n{stdout}"
    );
}

/// Tests all JSON format variants and their combinations.
#[test]
fn test_cargo_message_format_json_variants() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Test 1: Plain json - compiler messages in stdout, nothing rendered to stderr.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        !stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should not contain deprecation warning in json format:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        !compiler_messages.is_empty(),
        "stdout should contain compiler-message entries:\n{stdout}"
    );
    assert!(
        has_deprecation_warning(&compiler_messages),
        "should find deprecation warning in compiler messages"
    );
    // Plain json should not have ANSI in rendered field.
    let rendered = get_rendered_from_messages(&compiler_messages);
    assert!(
        !rendered.iter().any(|r| r.contains(ANSI_ESCAPE)),
        "plain json should not have ANSI codes in rendered field"
    );

    // Test 2: json-diagnostic-short - short diagnostics in JSON, no stderr rendering.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-diagnostic-short",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        !stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should not contain warning:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        !compiler_messages.is_empty(),
        "stdout should contain compiler-message entries"
    );
    assert!(
        has_deprecation_warning(&compiler_messages),
        "should find deprecation warning"
    );

    // Test 3: json-diagnostic-rendered-ansi - ANSI codes in rendered field.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-diagnostic-rendered-ansi",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        !compiler_messages.is_empty(),
        "stdout should contain compiler-message entries"
    );

    let rendered = get_rendered_from_messages(&compiler_messages);
    assert!(
        rendered.iter().any(|r| r.contains(ANSI_ESCAPE)),
        "json-diagnostic-rendered-ansi should have ANSI codes in rendered field"
    );

    // Test 4: json-render-diagnostics - Cargo renders to stderr, messages
    // removed from stdout.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-render-diagnostics",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain warning with json-render-diagnostics:\n{stderr}"
    );
    assert!(
        stderr.contains(DEPRECATION_NOTE_SUBSTRING),
        "stderr should contain note with json-render-diagnostics:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        compiler_messages.is_empty(),
        "stdout should not contain compiler-message entries with json-render-diagnostics:\n{stdout}"
    );
}

/// Tests combined JSON modifiers.
#[test]
fn test_cargo_message_format_json_combined() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Test 1: json-diagnostic-short,json-diagnostic-rendered-ansi
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-diagnostic-short,json-diagnostic-rendered-ansi",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        !stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should not contain warning:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        !compiler_messages.is_empty(),
        "should have compiler messages"
    );

    let rendered = get_rendered_from_messages(&compiler_messages);
    assert!(
        rendered.iter().any(|r| r.contains(ANSI_ESCAPE)),
        "combined short+ansi should have ANSI codes in rendered field"
    );

    // Test 2: json-render-diagnostics,json-diagnostic-short - renders short to stderr.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-render-diagnostics,json-diagnostic-short",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain warning:\n{stderr}"
    );
    assert!(
        stderr.contains(DEPRECATION_NOTE_SUBSTRING),
        "stderr should contain note:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        compiler_messages.is_empty(),
        "stdout should not contain compiler-message entries"
    );

    // Test 3: All three modifiers combined.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-render-diagnostics,json-diagnostic-short,json-diagnostic-rendered-ansi",
        ])
        .output();

    let stdout = output.stdout_as_str();
    let stderr = output.stderr_as_str();

    assert!(
        stderr.contains(DEPRECATION_WARNING_SUBSTRING),
        "stderr should contain warning:\n{stderr}"
    );

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        compiler_messages.is_empty(),
        "stdout should not contain compiler-message entries"
    );

    // Test 4: Multiple --cargo-message-format arguments instead of comma-separated.
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "--cargo-message-format",
            "json-diagnostic-short",
            "--cargo-message-format",
            "json-diagnostic-rendered-ansi",
        ])
        .output();

    let stdout = output.stdout_as_str();

    let compiler_messages = extract_compiler_messages(&stdout);
    assert!(
        !compiler_messages.is_empty(),
        "should have compiler messages"
    );

    let rendered = get_rendered_from_messages(&compiler_messages);
    assert!(
        rendered.iter().any(|r| r.contains(ANSI_ESCAPE)),
        "multiple args should work like comma-separated"
    );
}

/// Tests error cases for invalid format combinations.
#[test]
fn test_cargo_message_format_errors() {
    let env_info = set_env_vars_for_test();
    let p = TempProject::new(&env_info).unwrap();

    // Test 1: Conflicting base formats (human,json).
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--cargo-message-format",
            "human,json",
        ])
        .unchecked(true)
        .output();

    assert!(
        !output.exit_status.success(),
        "should fail with conflicting formats"
    );
    insta::assert_snapshot!(
        "error_conflicting_base_formats",
        redact_dynamic_fields(&output.stderr_as_str())
    );

    // Test 2: JSON modifier with non-JSON base (human,json-diagnostic-short).
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--cargo-message-format",
            "human,json-diagnostic-short",
        ])
        .unchecked(true)
        .output();

    assert!(
        !output.exit_status.success(),
        "should fail with JSON modifier on human format"
    );
    insta::assert_snapshot!(
        "error_json_modifier_with_non_json",
        redact_dynamic_fields(&output.stderr_as_str())
    );

    // Test 3: Duplicate option (json,json).
    let output = CargoNextestCli::for_test(&env_info)
        .args([
            "--manifest-path",
            p.manifest_path().as_str(),
            "list",
            "--cargo-message-format",
            "json,json",
        ])
        .unchecked(true)
        .output();

    assert!(!output.exit_status.success(), "should fail with duplicate");
    insta::assert_snapshot!(
        "error_duplicate_option",
        redact_dynamic_fields(&output.stderr_as_str())
    );
}

/// Redacts dynamic fields from output for snapshot testing.
///
/// Filters out lines that vary between runs, such as:
/// - "Blocking waiting for file lock"
/// - "info: experimental features enabled"
fn redact_dynamic_fields(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            !line.contains("Blocking waiting for file lock")
                && !line.starts_with("info: experimental features enabled")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extracts all compiler-message JSON objects from the output.
fn extract_compiler_messages(output: &str) -> Vec<Value> {
    output
        .lines()
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            if value.get("reason")?.as_str()? == "compiler-message" {
                Some(value)
            } else {
                None
            }
        })
        .collect()
}

/// Checks if any compiler message contains a deprecation warning.
fn has_deprecation_warning(messages: &[Value]) -> bool {
    messages.iter().any(|msg| {
        msg.get("message")
            .and_then(|m| m.get("message"))
            .and_then(|m| m.as_str())
            .is_some_and(|s| s.contains("deprecated"))
    })
}

/// Extracts the `rendered` field from all compiler messages.
fn get_rendered_from_messages(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|msg| {
            msg.get("message")?
                .get("rendered")?
                .as_str()
                .map(|s| s.to_string())
        })
        .collect()
}
