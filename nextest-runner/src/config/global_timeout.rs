// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::{Deserialize, de::IntoDeserializer};
use std::{fmt, time::Duration};

/// Type for the global-timeout config key.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct GlobalTimeout {
    #[serde(with = "humantime_serde")]
    pub(crate) period: Duration,
}

pub(super) fn deserialize_global_timeout<'de, D>(
    deserializer: D,
) -> Result<Option<GlobalTimeout>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<GlobalTimeout>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ period = \"60s\"}}) or a string (\"60s\")"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if v.is_empty() {
                Ok(None)
            } else {
                let period = humantime_serde::deserialize(v.into_deserializer())?;
                Ok(Some(GlobalTimeout { period }))
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            GlobalTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }
    }

    deserializer.deserialize_any(V)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        NextestConfig,
        test_helpers::{build_platforms, temp_workspace},
    };
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use test_case::test_case;

    #[test_case(
        "",
        Ok(GlobalTimeout { period: Duration::from_secs(946728000) }),
        None

        ; "empty config is expected to use the hardcoded values"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            global-timeout = "30s"
        "#},
        Ok(GlobalTimeout { period: Duration::from_secs(30) }),
        None

        ; "overrides the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            global-timeout = "30s"

            [profile.ci]
            global-timeout = { period = "60s" }
        "#},
        Ok(GlobalTimeout { period: Duration::from_secs(30) }),
        Some(GlobalTimeout { period: Duration::from_secs(60) })

        ; "adds a custom profile 'ci'"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            global-timeout = { period = "60s" }

            [profile.ci]
            global-timeout = "30s"
        "#},
        Ok(GlobalTimeout { period: Duration::from_secs(60) }),
        Some(GlobalTimeout { period: Duration::from_secs(30) })

        ; "ci profile uses string notation"
    )]
    fn globaltimeout_adheres_to_hierarchy(
        config_contents: &str,
        expected_default: Result<GlobalTimeout, &str>,
        maybe_expected_ci: Option<GlobalTimeout>,
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
                        .global_timeout(),
                    expected_default,
                );

                if let Some(expected_ci) = maybe_expected_ci {
                    assert_eq!(
                        nextest_config
                            .profile("ci")
                            .expect("ci profile should exist")
                            .apply_build_platforms(&build_platforms())
                            .global_timeout(),
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
