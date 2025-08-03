// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::{Deserialize, Deserializer};
use std::time::Duration;

/// Type for the global-timeout config key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalTimeout {
    pub(crate) period: Duration,
}

impl<'de> Deserialize<'de> for GlobalTimeout {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(GlobalTimeout {
            period: humantime_serde::deserialize(deserializer)?,
        })
    }
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
            global-timeout = "60s"
        "#},
        Ok(GlobalTimeout { period: Duration::from_secs(30) }),
        Some(GlobalTimeout { period: Duration::from_secs(60) })

        ; "adds a custom profile 'ci'"
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
