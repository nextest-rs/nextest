// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parsed text reported by run wrapper scripts.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

/// A note reported by a run wrapper.
///
/// Written as JSON to the path in `NEXTEST_RUN_WRAPPER_REPORT`; an absent
/// report means the wrapper ran the test normally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct RunWrapperReport {
    /// A short label displayed on the per-test status line.
    pub label: RunWrapperLabel,
    /// An optional category used to aggregate counts in the final run summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<RunWrapperCategory>,
}

/// An invalid label or category in a wrapper report.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error(
    "the {field} must contain 1 to {max_len} printable ASCII characters, and must start and end with a letter or digit"
)]
pub struct RunWrapperTextError {
    field: &'static str,
    max_len: usize,
}

fn parse_text(
    value: String,
    field: &'static str,
    max_len: usize,
) -> Result<String, RunWrapperTextError> {
    if value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        Ok(value)
    } else {
        Err(RunWrapperTextError { field, max_len })
    }
}

/// A validated label from a run wrapper report.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RunWrapperLabel(String);

impl RunWrapperLabel {
    /// Maximum length of the report text in bytes.
    pub(crate) const MAX_LEN: usize = 256;

    /// Returns the report text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RunWrapperLabel {
    type Error = RunWrapperTextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        parse_text(value, "label", Self::MAX_LEN).map(Self)
    }
}

impl FromStr for RunWrapperLabel {
    type Err = RunWrapperTextError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

impl fmt::Display for RunWrapperLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A validated category from a run wrapper report.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RunWrapperCategory(String);

impl RunWrapperCategory {
    /// Maximum length of the report text in bytes.
    pub(crate) const MAX_LEN: usize = 64;

    /// Returns the report text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RunWrapperCategory {
    type Error = RunWrapperTextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        parse_text(value, "category", Self::MAX_LEN).map(Self)
    }
}

impl FromStr for RunWrapperCategory {
    type Err = RunWrapperTextError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

impl fmt::Display for RunWrapperCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use test_strategy::proptest;

    impl Arbitrary for RunWrapperLabel {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: ()) -> Self::Strategy {
            "[a-zA-Z0-9]([ -~]{0,254}[a-zA-Z0-9])?"
                .prop_map(|text| text.parse().unwrap())
                .boxed()
        }
    }

    impl Arbitrary for RunWrapperCategory {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: ()) -> Self::Strategy {
            "[a-zA-Z0-9]([ -~]{0,62}[a-zA-Z0-9])?"
                .prop_map(|text| text.parse().unwrap())
                .boxed()
        }
    }

    #[proptest]
    fn report_text_roundtrips(label: RunWrapperLabel, category: RunWrapperCategory) {
        let report = RunWrapperReport {
            label,
            category: Some(category),
        };
        let value = serde_json::to_value(&report).unwrap();
        prop_assert_eq!(value["label"].as_str(), Some(report.label.as_str()));
        prop_assert_eq!(
            value["category"].as_str(),
            report.category.as_ref().map(RunWrapperCategory::as_str)
        );
        prop_assert_eq!(
            serde_json::from_value::<RunWrapperReport>(value).unwrap(),
            report
        );
    }

    #[test]
    fn invalid_text_cannot_be_constructed_or_deserialized() {
        for text in [
            "",
            " leading",
            "trailing ",
            "bad\nlabel",
            "escape\u{1b}",
            "café",
            "punctuation)",
        ] {
            assert!(text.parse::<RunWrapperLabel>().is_err(), "{text:?}");
            assert!(text.parse::<RunWrapperCategory>().is_err(), "{text:?}");
            assert!(
                serde_json::from_value::<RunWrapperReport>(serde_json::json!({"label": text}))
                    .is_err()
            );
            assert!(
                serde_json::from_value::<RunWrapperReport>(
                    serde_json::json!({"label": "valid", "category": text})
                )
                .is_err()
            );
        }
        for (length, label_ok, category_ok) in [
            (1, true, true),
            (64, true, true),
            (65, true, false),
            (256, true, false),
            (257, false, false),
        ] {
            let text = "x".repeat(length);
            assert_eq!(text.parse::<RunWrapperLabel>().is_ok(), label_ok);
            assert_eq!(text.parse::<RunWrapperCategory>().is_ok(), category_ok);
        }
    }
}
