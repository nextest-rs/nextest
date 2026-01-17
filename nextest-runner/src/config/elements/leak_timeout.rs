// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Leak timeout configuration.

use serde::{Deserialize, Serialize, de::IntoDeserializer};
use std::{fmt, time::Duration};

/// Controls leak timeout behavior.
///
/// Includes a period and a result (pass or fail).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct LeakTimeout {
    /// The leak timeout period.
    #[serde(with = "humantime_serde")]
    pub(crate) period: Duration,

    /// The result of terminating the test after the leak timeout period.
    #[serde(default)]
    pub(crate) result: LeakTimeoutResult,
}

impl Default for LeakTimeout {
    fn default() -> Self {
        Self {
            period: Duration::from_millis(100),
            result: LeakTimeoutResult::default(),
        }
    }
}

/// The result of controlling leak timeout behavior.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum LeakTimeoutResult {
    /// The test is marked as failed.
    Fail,

    #[default]
    /// The test is marked as passed.
    Pass,
}

pub(in crate::config) fn deserialize_leak_timeout<'de, D>(
    deserializer: D,
) -> Result<Option<LeakTimeout>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<LeakTimeout>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> std::fmt::Result {
            write!(
                formatter,
                "a table ({{ period = \"500ms\", result = \"fail\" }}) or a string (\"100ms\")"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let period = humantime_serde::deserialize(v.into_deserializer())?;
            Ok(Some(LeakTimeout {
                period,
                result: LeakTimeoutResult::default(),
            }))
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            LeakTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }
    }

    deserializer.deserialize_any(V)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{core::NextestConfig, utils::test_helpers::*};
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use test_case::test_case;

    #[test_case(
        "",
        Ok(LeakTimeout { period: Duration::from_millis(200), result: LeakTimeoutResult::Pass}),
        None

        ; "empty config is expected to use the hardcoded values"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = "5s"
        "#},
        Ok(LeakTimeout { period: Duration::from_secs(5), result: LeakTimeoutResult::Pass }),
        None

        ; "overrides the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = "5s"

            [profile.ci]
            leak-timeout = { period = "1s", result = "fail" }
        "#},
        Ok(LeakTimeout { period: Duration::from_secs(5), result: LeakTimeoutResult::Pass }),
        Some(LeakTimeout { period: Duration::from_secs(1), result: LeakTimeoutResult::Fail })

        ; "adds a custom profile 'ci'"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = { period = "5s", result = "fail" }

            [profile.ci]
            leak-timeout = "1s"
        "#},
        Ok(LeakTimeout { period: Duration::from_secs(5), result: LeakTimeoutResult::Fail }),
        Some(LeakTimeout { period: Duration::from_secs(1), result: LeakTimeoutResult::Pass })

        ; "ci profile uses string notation"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = { period = "5s" }
        "#},
        Ok(LeakTimeout { period: Duration::from_secs(5), result: LeakTimeoutResult::Pass }),
        None

        ; "partial table"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = "1s"

            [profile.ci]
            leak-timeout = { result = "fail" }
        "#},
        Err(r#"original: missing configuration field "profile.ci.leak-timeout.period""#),
        None

        ; "partial leak-timeout table should error"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            leak-timeout = 123
        "#},
        Err("original: invalid type: integer `123`, expected a table"),
        None

        ; "incorrect leak-timeout format"
    )]
    fn leak_timeout_adheres_to_hierarchy(
        config_contents: &str,
        expected_default: Result<LeakTimeout, &str>,
        maybe_expected_ci: Option<LeakTimeout>,
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let nextest_config_result = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        );

        match expected_default {
            Ok(expected_default) => {
                let nextest_config = nextest_config_result.expect("config file should parse");

                assert_eq!(
                    nextest_config
                        .profile("default")
                        .expect("default profile should exist")
                        .apply_build_platforms(&build_platforms())
                        .leak_timeout(),
                    expected_default,
                );

                if let Some(expected_ci) = maybe_expected_ci {
                    assert_eq!(
                        nextest_config
                            .profile("ci")
                            .expect("ci profile should exist")
                            .apply_build_platforms(&build_platforms())
                            .leak_timeout(),
                        expected_ci,
                    );
                }
            }

            Err(expected_err_str) => {
                let err_str = format!("{:?}", nextest_config_result.unwrap_err());

                assert!(
                    err_str.contains(expected_err_str),
                    "expected error string not found: {err_str}",
                )
            }
        }
    }
}
