// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8PathBuf;
use color_eyre::eyre::{self, Context};
use nextest_runner::{config::core::NextestConfig, user_config::UserConfig};
use std::fs;

const REPO_SCHEMA_LABEL: &str =
    "embedded repo-config schema (nextest-runner/jsonschemas/repo-config.json)";
const USER_SCHEMA_LABEL: &str =
    "embedded user-config schema (nextest-runner/jsonschemas/user-config.json)";

fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::var("NEXTEST_WORKSPACE_ROOT")
            .expect("NEXTEST_WORKSPACE_ROOT is set (running under cargo nextest run)"),
    )
}

fn build_validator(schema_text: &str, schema_label: &str) -> eyre::Result<jsonschema::Validator> {
    let schema: serde_json::Value = serde_json::from_str(schema_text)
        .wrap_err_with(|| format!("error parsing {schema_label}"))?;
    jsonschema::validator_for(&schema)
        .wrap_err_with(|| format!("error building validator from {schema_label}"))
}

fn assert_validates(
    validator: &jsonschema::Validator,
    schema_label: &str,
    label: &str,
    toml_text: &str,
) -> eyre::Result<()> {
    let config: serde_json::Value =
        toml::from_str(toml_text).wrap_err_with(|| format!("error parsing {label}"))?;
    let errors: Vec<_> = validator.iter_errors(&config).collect();
    assert!(
        errors.is_empty(),
        "{label} does not validate against {schema_label}:\n{errors:#?}",
    );
    Ok(())
}

#[test]
fn test_validate_nextest_config() -> eyre::Result<()> {
    let validator = build_validator(NextestConfig::SCHEMA, REPO_SCHEMA_LABEL)?;
    let config_path = workspace_root().join(".config/nextest.toml");
    let config_text = fs::read_to_string(&config_path)
        .wrap_err_with(|| format!("error reading config file at {config_path}"))?;
    assert_validates(
        &validator,
        REPO_SCHEMA_LABEL,
        config_path.as_str(),
        &config_text,
    )
}

#[test]
fn test_validate_default_config() -> eyre::Result<()> {
    let validator = build_validator(NextestConfig::SCHEMA, REPO_SCHEMA_LABEL)?;
    assert_validates(
        &validator,
        REPO_SCHEMA_LABEL,
        "default config (nextest-runner/default-config.toml)",
        NextestConfig::DEFAULT_CONFIG,
    )
}

#[test]
fn test_validate_default_user_config() -> eyre::Result<()> {
    let validator = build_validator(UserConfig::SCHEMA, USER_SCHEMA_LABEL)?;
    assert_validates(
        &validator,
        USER_SCHEMA_LABEL,
        "default user config (nextest-runner/default-user-config.toml)",
        UserConfig::DEFAULT_CONFIG,
    )
}

/// A non-default user config that exercises every section, every variant the
/// schema declares, and the three forms `pager` and `paginate` can take. The
/// default user config (`default-user-config.toml`) only uses one variant per
/// field, so it doesn't pin schema regressions in alternative forms; this
/// fixture does.
const EXHAUSTIVE_USER_CONFIG: &str = r#"
[experimental]
record = true

[ui]
show-progress = "only"
max-progress-running = "infinite"
input-handler = false
output-indent = false
pager = ["less", "-FRX"]
paginate = "never"

[ui.streampager]
interface = "full-screen-clear-output"
wrapping = "anywhere"
show-ruler = false

[record]
enabled = true
max-records = 50
max-total-size = "512MB"
max-age = "7d"
max-output-size = "5MB"

[[overrides]]
platform = "cfg(windows)"
ui.pager = ":builtin"
ui.show-progress = "counter"
ui.max-progress-running = 4
ui.streampager.wrapping = "none"
record.enabled = false
record.max-age = "1d"

[[overrides]]
platform = "x86_64-unknown-linux-gnu"
ui.pager = "less -R"
"#;

#[test]
fn test_validate_exhaustive_user_config() -> eyre::Result<()> {
    let validator = build_validator(UserConfig::SCHEMA, USER_SCHEMA_LABEL)?;
    assert_validates(
        &validator,
        USER_SCHEMA_LABEL,
        "exhaustive user config (inline)",
        EXHAUSTIVE_USER_CONFIG,
    )
}

/// Asserts that the schema rejects `toml_text` with at least one error that:
///
/// - Fires at the given JSON-pointer `instance_path` (e.g. `/ui/pager`).
/// - Fires under the given JSON Schema `keyword` (e.g.
///   `"additionalProperties"`, `"required"`, `"anyOf"`).
/// - Renders a message that contains `message_contains`.
///
/// The path and keyword pin the structural reason for rejection (which rule
/// fired, on what instance). The substring pins the *content* of the rejected
/// value or field name — catching the case where the right rule fires for the
/// wrong reason (e.g. a typo in the negative fixture makes some unrelated
/// field unknown, satisfying path/keyword but missing the rule under test).
fn assert_rejects(
    validator: &jsonschema::Validator,
    label: &str,
    toml_text: &str,
    instance_path: &str,
    keyword: &str,
    message_contains: &str,
) {
    let config: serde_json::Value =
        toml::from_str(toml_text).unwrap_or_else(|err| panic!("error parsing {label}: {err}"));
    let errors: Vec<_> = validator.iter_errors(&config).collect();
    let matched = errors.iter().any(|e| {
        e.instance_path().as_str() == instance_path
            && e.kind().keyword() == keyword
            && e.to_string().contains(message_contains)
    });
    assert!(
        matched,
        "{label}: expected at least one validation error at path {instance_path:?} \
         with keyword {keyword:?} containing {message_contains:?}; got {} error(s):\n{}",
        errors.len(),
        errors
            .iter()
            .map(|e| format!(
                "  - path={:?} keyword={:?} msg={}",
                e.instance_path().as_str(),
                e.kind().keyword(),
                e
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// Ensures that the optional `[record]` size and duration fields accept `null`.
/// (This doesn't affect TOML since that can't express `null`, so we test
/// against a hypothetical JSON config.)
///
/// `max-age`, `max-output-size`, and `max-total-size` are `Option<Duration>` or
/// `Option<ByteSize>` in Rust. The fields use a custom `schemars(with = ...)` to
/// declare a string-typed schema. The `with` must specify `Option<String>`
/// so the generated schema is `{"type": ["string", "null"]}` rather than
/// `{"type": "string"}` with a contradictory `"default": null`.
///
/// `max-records` (an `Option<usize>` without a custom helper) is a known-good
/// baseline; including it here ensures the test doesn't vacuously pass against
/// a validator that ignores nullability everywhere.
#[test]
fn test_optional_record_fields_accept_null() -> eyre::Result<()> {
    let validator = build_validator(UserConfig::SCHEMA, USER_SCHEMA_LABEL)?;
    // Base `[record]` table.
    let config = serde_json::json!({
        "record": {
            "max-records": null,
            "max-age": null,
            "max-output-size": null,
            "max-total-size": null,
        },
        // Same fields, also reachable through `[[overrides]] record.*`, must
        // be nullable in `DeserializedRecordOverrideData` too.
        "overrides": [{
            "platform": "cfg(unix)",
            "record": {
                "max-records": null,
                "max-age": null,
                "max-output-size": null,
                "max-total-size": null,
            },
        }],
    });
    let errors: Vec<_> = validator.iter_errors(&config).collect();
    assert!(
        errors.is_empty(),
        "optional `[record]` fields should accept null, got errors:\n{}",
        errors
            .iter()
            .map(|e| format!(
                "  - path={:?} keyword={:?} msg={}",
                e.instance_path().as_str(),
                e.kind().keyword(),
                e
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    Ok(())
}

#[test]
fn test_invalid_user_configs_are_rejected() -> eyre::Result<()> {
    let validator = build_validator(UserConfig::SCHEMA, USER_SCHEMA_LABEL)?;

    // Unknown top-level key (deny_unknown_fields on DeserializedUserConfig).
    assert_rejects(
        &validator,
        "unknown top-level key",
        r#"
not-a-real-section = true
"#,
        "",
        "additionalProperties",
        "not-a-real-section",
    );

    // Unknown key inside [ui] (deny_unknown_fields on DeserializedUiConfig).
    assert_rejects(
        &validator,
        "unknown [ui] key",
        r#"
[ui]
not-a-real-ui-setting = "yes"
"#,
        "/ui",
        "additionalProperties",
        "not-a-real-ui-setting",
    );

    // Invalid enum value for `ui.show-progress`: each `UiShowProgress` variant
    // is a string `const`, so a non-matching string fails the `anyOf` over the
    // nullable-union the schema wraps the type in.
    assert_rejects(
        &validator,
        "invalid ui.show-progress variant",
        r#"
[ui]
show-progress = "rainbow"
"#,
        "/ui/show-progress",
        "anyOf",
        "rainbow",
    );

    // An empty `command` array on the structured pager form violates
    // `minItems: 1` in the `CommandNameAndArgs` schema, which surfaces as an
    // `anyOf` failure on the parent `PagerSetting`. The error message echoes
    // the rejected value, so we look for the empty-array literal to confirm
    // it's *this* value being rejected (and not, say, the pager `:builtin`
    // branch failing for an unrelated reason).
    assert_rejects(
        &validator,
        "empty pager command array",
        r#"
[ui]
pager = { command = [] }
"#,
        "/ui/pager",
        "anyOf",
        r#""command":[]"#,
    );

    // `pager = ""` is rejected by the runtime visitor (`shell_words::split`
    // returns an empty Vec). The schema should reject it too so editors flag
    // it before nextest runs.
    assert_rejects(
        &validator,
        "empty pager string",
        r#"
[ui]
pager = ""
"#,
        "/ui/pager",
        "anyOf",
        r#""""#,
    );

    // `pager = "   "` (whitespace-only) is likewise rejected by the runtime
    // (`shell_words::split` returns an empty Vec after splitting on
    // whitespace). The schema must match.
    assert_rejects(
        &validator,
        "whitespace-only pager string",
        r#"
[ui]
pager = "   "
"#,
        "/ui/pager",
        "anyOf",
        r#""   ""#,
    );

    // `[[overrides]]` is required to have a `platform` field.
    assert_rejects(
        &validator,
        "override missing platform",
        r#"
[[overrides]]
ui.show-progress = "bar"
"#,
        "/overrides/0",
        "required",
        "platform",
    );

    Ok(())
}
