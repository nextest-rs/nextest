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
    use crate::{
        config::{core::NextestConfig, utils::test_helpers::*},
        run_mode::NextestRunMode,
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
                        .global_timeout(NextestRunMode::Test),
                    expected_default,
                );

                if let Some(expected_ci) = maybe_expected_ci {
                    assert_eq!(
                        nextest_config
                            .profile("ci")
                            .expect("ci profile should exist")
                            .apply_build_platforms(&build_platforms())
                            .global_timeout(NextestRunMode::Test),
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

    // Default global-timeout is 30 years (946728000 seconds).
    const DEFAULT_GLOBAL_TIMEOUT: GlobalTimeout = GlobalTimeout {
        period: Duration::from_secs(946728000),
    };

    /// Expected bench global-timeout: either a specific value or "very large"
    /// (default).
    #[derive(Debug)]
    enum ExpectedBenchGlobalTimeout {
        /// Expect a specific timeout value.
        Exact(GlobalTimeout),
        /// Expect the default very large timeout (>= 30 years, accounting for
        /// leap years in humantime parsing).
        VeryLarge,
    }

    #[test_case(
        "",
        DEFAULT_GLOBAL_TIMEOUT,
        ExpectedBenchGlobalTimeout::VeryLarge
        ; "empty config uses defaults for both modes"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            global-timeout = "10s"
        "#},
        GlobalTimeout { period: Duration::from_secs(10) },
        // bench.global-timeout should still be 30 years (default).
        ExpectedBenchGlobalTimeout::VeryLarge
        ; "global-timeout does not affect bench.global-timeout"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            bench.global-timeout = "20s"
        "#},
        // global-timeout should still be 30 years (default).
        DEFAULT_GLOBAL_TIMEOUT,
        ExpectedBenchGlobalTimeout::Exact(GlobalTimeout {
            period: Duration::from_secs(20),
        })
        ; "bench.global-timeout does not affect global-timeout"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            global-timeout = "10s"
            bench.global-timeout = "20s"
        "#},
        GlobalTimeout { period: Duration::from_secs(10) },
        ExpectedBenchGlobalTimeout::Exact(GlobalTimeout {
            period: Duration::from_secs(20),
        })
        ; "both global-timeout and bench.global-timeout can be set independently"
    )]
    fn bench_globaltimeout_is_independent(
        config_contents: &str,
        expected_test_timeout: GlobalTimeout,
        expected_bench_timeout: ExpectedBenchGlobalTimeout,
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);

        let pcx = ParseContext::new(&graph);

        let nextest_config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .expect("config file should parse");

        let profile = nextest_config
            .profile("default")
            .expect("default profile should exist")
            .apply_build_platforms(&build_platforms());

        assert_eq!(
            profile.global_timeout(NextestRunMode::Test),
            expected_test_timeout,
            "Test mode global-timeout mismatch"
        );

        let actual_bench_timeout = profile.global_timeout(NextestRunMode::Benchmark);
        match expected_bench_timeout {
            ExpectedBenchGlobalTimeout::Exact(expected) => {
                assert_eq!(
                    actual_bench_timeout, expected,
                    "Benchmark mode global-timeout mismatch"
                );
            }
            ExpectedBenchGlobalTimeout::VeryLarge => {
                // The default is "30y" which humantime parses accounting for
                // leap years, so it is slightly larger than DEFAULT_GLOBAL_TIMEOUT.
                assert!(
                    actual_bench_timeout.period >= DEFAULT_GLOBAL_TIMEOUT.period,
                    "Benchmark mode global-timeout should be >= default, got {:?}",
                    actual_bench_timeout.period
                );
            }
        }
    }
}
