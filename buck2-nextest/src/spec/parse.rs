// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading and validating a Buck2 test spec.

use crate::{
    errors::{ExpectedError, Result},
    spec::types::{Buck2TestSpec, ConfiguredTarget, ExternalRunnerSpec, SpecValue},
};
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

/// The test type `buck2-nextest` supports.
///
/// Rust test binaries speak the libtest protocol, which is what the runner
/// drives. Other types would need a different listing and invocation protocol.
pub const SUPPORTED_TEST_TYPE: &str = "rust";

/// A validated test target, ready to be converted into nextest's types.
///
/// Every value here is resolved: no handles remain, and the command is known to
/// be non-empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buck2TestTarget {
    /// The target's label, in `cell//package:target` form.
    pub label: String,

    /// The configured target label, broken into parts.
    pub target: ConfiguredTarget,

    /// The program to run.
    pub program: String,

    /// Arguments that go before nextest's libtest arguments.
    pub leading_args: Vec<String>,

    /// Environment variables for the test process.
    pub env: BTreeMap<String, String>,

    /// The directory the test runs in, if the source of the target named one.
    ///
    /// A spec file does not carry a working directory, so the file-based path
    /// leaves this `None` and lets the conversion fall back to the target's
    /// package directory. Buck2 states it outright over gRPC.
    pub cwd: Option<Utf8PathBuf>,
}

/// Reads a spec from a file, or from standard input if the path is `-`.
pub fn read_spec(path: &Utf8Path) -> Result<Vec<Buck2TestTarget>> {
    let (contents, source_name) = if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|error| ExpectedError::SpecStdinReadError { error })?;
        (buf, "standard input".to_owned())
    } else {
        let contents =
            std::fs::read_to_string(path).map_err(|error| ExpectedError::SpecReadError {
                path: path.to_owned(),
                error,
            })?;
        (contents, format!("`{path}`"))
    };

    parse_spec(&contents, &source_name)
}

/// Parses and validates a spec from JSON text.
pub fn parse_spec(contents: &str, source_name: &str) -> Result<Vec<Buck2TestTarget>> {
    let deserializer = &mut serde_json::Deserializer::from_str(contents);
    let spec: Buck2TestSpec = serde_path_to_error::deserialize(deserializer).map_err(|error| {
        ExpectedError::SpecParseError {
            source_name: source_name.to_owned(),
            error,
        }
    })?;

    let mut targets = Vec::with_capacity(spec.targets.len());
    let mut seen = BTreeSet::new();

    for target in spec.targets {
        let validated = validate_target(target)?;
        if !seen.insert(validated.label.clone()) {
            return Err(ExpectedError::DuplicateTarget {
                label: validated.label,
            });
        }
        targets.push(validated);
    }

    // Sort by label so the binary list order is deterministic regardless of the
    // order Buck2 emitted targets in.
    targets.sort_by(|a, b| a.label.cmp(&b.label));

    Ok(targets)
}

fn validate_target(spec: ExternalRunnerSpec) -> Result<Buck2TestTarget> {
    let label = spec.target.label();

    if spec.test_type != SUPPORTED_TEST_TYPE {
        return Err(ExpectedError::UnsupportedTestType {
            label,
            test_type: spec.test_type,
        });
    }

    let mut command = spec.command.into_iter();
    let Some(program) = command.next() else {
        return Err(ExpectedError::EmptyCommand { label });
    };

    let program = resolve(&program, &label, "command")?.to_owned();
    let leading_args = command
        .map(|value| Ok(resolve(&value, &label, "command")?.to_owned()))
        .collect::<Result<Vec<_>>>()?;

    let env = spec
        .env
        .iter()
        .map(|(name, value)| {
            let resolved = resolve(value, &label, &format!("env var `{name}`"))?;
            Ok((name.clone(), resolved.to_owned()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    Ok(Buck2TestTarget {
        label,
        target: spec.target,
        program,
        leading_args,
        env,
        cwd: None,
    })
}

fn resolve<'a>(value: &'a SpecValue, label: &str, location: &str) -> Result<&'a str> {
    value
        .as_verbatim()
        .ok_or_else(|| ExpectedError::UnresolvedSpecValue {
            label: label.to_owned(),
            kind: value.kind(),
            location: location.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn parse(contents: &str) -> Result<Vec<Buck2TestTarget>> {
        parse_spec(contents, "test input")
    }

    static GOLDEN: &str = indoc! {r#"
        {
          "targets": [
            {
              "target": {
                "cell": "fbcode",
                "package": "buck2/app",
                "target": "my_test",
                "configuration": "cfg:linux"
              },
              "test_type": "rust",
              "command": [
                {"verbatim": "buck-out/gen/my_test"},
                {"verbatim": "--nocapture-passthrough"}
              ],
              "env": {"RUST_BACKTRACE": {"verbatim": "1"}},
              "labels": ["rust"],
              "contacts": ["someone"],
              "oncall": "rust_oncall",
              "working_dir_cell": "fbcode"
            }
          ]
        }
    "#};

    #[test]
    fn golden_spec_parses() {
        let targets = parse(GOLDEN).expect("golden spec is valid");
        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(target.label, "fbcode//buck2/app:my_test");
        assert_eq!(target.program, "buck-out/gen/my_test");
        assert_eq!(target.leading_args, vec!["--nocapture-passthrough"]);
        assert_eq!(
            target.env.get("RUST_BACKTRACE").map(String::as_str),
            Some("1")
        );
    }

    /// A bare string is shorthand for a verbatim value.
    #[test]
    fn bare_strings_are_verbatim() {
        let contents = indoc! {r#"
            {
              "targets": [
                {
                  "target": {"cell": "c", "package": "p", "target": "t"},
                  "test_type": "rust",
                  "command": ["bin", "--arg"],
                  "env": {"KEY": "value"}
                }
              ]
            }
        "#};
        let targets = parse(contents).expect("bare strings are accepted");
        assert_eq!(targets[0].program, "bin");
        assert_eq!(targets[0].leading_args, vec!["--arg"]);
        assert_eq!(targets[0].env.get("KEY").map(String::as_str), Some("value"));
    }

    #[test]
    fn non_rust_test_type_is_rejected() {
        let contents = GOLDEN.replace(r#""test_type": "rust""#, r#""test_type": "gtest""#);
        let error = parse(&contents).expect_err("gtest is not supported");
        assert!(
            matches!(
                &error,
                ExpectedError::UnsupportedTestType { label, test_type }
                    if label == "fbcode//buck2/app:my_test" && test_type == "gtest"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn arg_handle_is_rejected() {
        let contents = GOLDEN.replace(
            r#"{"verbatim": "--nocapture-passthrough"}"#,
            r#"{"arg_handle": 7}"#,
        );
        let error = parse(&contents).expect_err("handles cannot be resolved");
        assert!(
            matches!(
                &error,
                ExpectedError::UnresolvedSpecValue { kind, location, .. }
                    if *kind == "arg_handle" && location == "command"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn env_handle_names_the_variable() {
        let contents = GOLDEN.replace(
            r#""env": {"RUST_BACKTRACE": {"verbatim": "1"}}"#,
            r#""env": {"RUST_BACKTRACE": {"env_handle": "h"}}"#,
        );
        let error = parse(&contents).expect_err("handles cannot be resolved");
        assert!(
            matches!(
                &error,
                ExpectedError::UnresolvedSpecValue { kind, location, .. }
                    if *kind == "env_handle" && location == "env var `RUST_BACKTRACE`"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn empty_command_is_rejected() {
        let contents = indoc! {r#"
            {
              "targets": [
                {
                  "target": {"cell": "c", "package": "p", "target": "t"},
                  "test_type": "rust",
                  "command": []
                }
              ]
            }
        "#};
        let error = parse(contents).expect_err("an empty command has no binary");
        assert!(
            matches!(&error, ExpectedError::EmptyCommand { label } if label == "c//p:t"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let contents = GOLDEN.replace(
            r#""test_type": "rust""#,
            r#""test_type": "rust", "bogus": 1"#,
        );
        let error = parse(&contents).expect_err("unknown fields are a mistake worth reporting");
        assert!(
            matches!(&error, ExpectedError::SpecParseError { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn duplicate_targets_are_rejected() {
        let one = indoc! {r#"
            {
              "target": {"cell": "c", "package": "p", "target": "t"},
              "test_type": "rust",
              "command": ["bin"]
            }
        "#};
        let contents = format!("{{\"targets\": [{one}, {one}]}}");
        let error = parse(&contents).expect_err("duplicate labels are ambiguous");
        assert!(
            matches!(&error, ExpectedError::DuplicateTarget { label } if label == "c//p:t"),
            "unexpected error: {error:?}"
        );
    }

    /// Buck2 may emit targets in any order; the resulting list must not depend
    /// on it, since binary IDs are expected to be sorted.
    #[test]
    fn targets_are_sorted_by_label() {
        let contents = indoc! {r#"
            {
              "targets": [
                {
                  "target": {"cell": "c", "package": "p", "target": "zebra"},
                  "test_type": "rust",
                  "command": ["z"]
                },
                {
                  "target": {"cell": "c", "package": "p", "target": "aardvark"},
                  "test_type": "rust",
                  "command": ["a"]
                }
              ]
            }
        "#};
        let targets = parse(contents).expect("valid spec");
        let labels: Vec<_> = targets.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, vec!["c//p:aardvark", "c//p:zebra"]);
    }

    #[test]
    fn a_spec_value_map_must_have_exactly_one_key() {
        let contents = GOLDEN.replace(
            r#"{"verbatim": "buck-out/gen/my_test"}"#,
            r#"{"verbatim": "a", "arg_handle": 1}"#,
        );
        let error = parse(&contents).expect_err("ambiguous value");
        assert!(
            matches!(&error, ExpectedError::SpecParseError { .. }),
            "unexpected error: {error:?}"
        );
    }
}
