// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::time::far_future_duration;
use serde::{Deserialize, Serialize, de::IntoDeserializer};
use std::{fmt, num::NonZeroUsize, time::Duration};

/// Type for the slow-timeout config key.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SlowTimeout {
    #[serde(with = "humantime_serde")]
    pub(crate) period: Duration,
    #[serde(default)]
    pub(crate) terminate_after: Option<NonZeroUsize>,
    #[serde(with = "humantime_serde", default = "default_grace_period")]
    pub(crate) grace_period: Duration,
    #[serde(default)]
    pub(crate) on_timeout: SlowTimeoutResult,
}

impl SlowTimeout {
    /// A reasonable value for "maximum slow timeout".
    pub(crate) const VERY_LARGE: Self = Self {
        // See far_future() in pausable_sleep.rs for why this is roughly 30 years.
        period: far_future_duration(),
        terminate_after: None,
        grace_period: Duration::from_secs(10),
        on_timeout: SlowTimeoutResult::Fail,
    };
}

fn default_grace_period() -> Duration {
    Duration::from_secs(10)
}

pub(in crate::config) fn deserialize_slow_timeout<'de, D>(
    deserializer: D,
) -> Result<Option<SlowTimeout>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<SlowTimeout>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ period = \"60s\", terminate-after = 2 }}) or a string (\"60s\")"
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
                Ok(Some(SlowTimeout {
                    period,
                    terminate_after: None,
                    grace_period: default_grace_period(),
                    on_timeout: SlowTimeoutResult::Fail,
                }))
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            SlowTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }
    }

    deserializer.deserialize_any(V)
}

/// The result of controlling slow timeout behavior.
///
/// In most situations a timed out test should be marked failing. However, there are certain
/// classes of tests which are expected to run indefinitely long, like fuzzing, which explores a
/// huge state space. For these tests it's nice to be able to treat a timeout as a success since
/// they generally check for invariants and other properties of the code under test during their
/// execution. A timeout in this context doesn't mean that there are no failing inputs, it just
/// means that they weren't found up until that moment, which is still valuable information.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum SlowTimeoutResult {
    #[default]
    /// The test is marked as failed.
    Fail,

    /// The test is marked as passed.
    Pass,
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
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        None
        ; "empty config is expected to use the hardcoded values"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        None
        ; "overrides the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "30s"

            [profile.ci]
            slow-timeout = { period = "60s", terminate-after = 3 }
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        Some(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: Some(NonZeroUsize::new(3).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "adds a custom profile 'ci'"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 3 }

            [profile.ci]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: Some(NonZeroUsize::new(3).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        Some(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "ci profile uses string notation"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 3, grace-period = "1s" }

            [profile.ci]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: Some(NonZeroUsize::new(3).unwrap()),
            grace_period: Duration::from_secs(1),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        Some(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "timeout grace period"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s" }
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        None
        ; "partial table"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 0 }
        "#},
        Err("original: invalid value: integer `0`, expected a nonzero usize"),
        None
        ; "zero terminate-after should fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", on-timeout = "pass" }
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Pass,
        }),
        None
        ; "timeout result success"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", on-timeout = "fail" }
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        }),
        None
        ; "timeout result failure"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", on-timeout = "pass" }

            [profile.ci]
            slow-timeout = { period = "30s", on-timeout = "fail" }
        "#},
        Ok(SlowTimeout {
            period: Duration::from_secs(60),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Pass,
        }),
        Some(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "override on-timeout option"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "60s"

            [profile.ci]
            slow-timeout = { terminate-after = 3 }
        "#},
        Err("original: missing configuration field \"profile.ci.slow-timeout.period\""),
        None

        ; "partial slow-timeout table should error"
    )]
    fn slowtimeout_adheres_to_hierarchy(
        config_contents: &str,
        expected_default: Result<SlowTimeout, &str>,
        maybe_expected_ci: Option<SlowTimeout>,
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
                        .slow_timeout(NextestRunMode::Test),
                    expected_default,
                );

                if let Some(expected_ci) = maybe_expected_ci {
                    assert_eq!(
                        nextest_config
                            .profile("ci")
                            .expect("ci profile should exist")
                            .apply_build_platforms(&build_platforms())
                            .slow_timeout(NextestRunMode::Test),
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

    // Default test slow-timeout is 60 seconds.
    const DEFAULT_TEST_SLOW_TIMEOUT: SlowTimeout = SlowTimeout {
        period: Duration::from_secs(60),
        terminate_after: None,
        grace_period: Duration::from_secs(10),
        on_timeout: SlowTimeoutResult::Fail,
    };

    /// Expected bench timeout: either a specific value or "very large" (default).
    #[derive(Debug)]
    enum ExpectedBenchTimeout {
        /// Expect a specific timeout value.
        Exact(SlowTimeout),
        /// Expect the default very large timeout (>= VERY_LARGE, accounting for
        /// leap years in humantime parsing).
        VeryLarge,
    }

    #[test_case(
        "",
        DEFAULT_TEST_SLOW_TIMEOUT,
        ExpectedBenchTimeout::VeryLarge
        ; "empty config uses defaults for both modes"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "10s", terminate-after = 2 }
        "#},
        SlowTimeout {
            period: Duration::from_secs(10),
            terminate_after: Some(NonZeroUsize::new(2).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        },
        // bench.slow-timeout should still be 30 years (default).
        ExpectedBenchTimeout::VeryLarge
        ; "slow-timeout does not affect bench.slow-timeout"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            bench.slow-timeout = { period = "20s", terminate-after = 3 }
        "#},
        // slow-timeout should still be 60s (default).
        DEFAULT_TEST_SLOW_TIMEOUT,
        ExpectedBenchTimeout::Exact(SlowTimeout {
            period: Duration::from_secs(20),
            terminate_after: Some(NonZeroUsize::new(3).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "bench.slow-timeout does not affect slow-timeout"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "10s", terminate-after = 2 }
            bench.slow-timeout = { period = "20s", terminate-after = 3 }
        "#},
        SlowTimeout {
            period: Duration::from_secs(10),
            terminate_after: Some(NonZeroUsize::new(2).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        },
        ExpectedBenchTimeout::Exact(SlowTimeout {
            period: Duration::from_secs(20),
            terminate_after: Some(NonZeroUsize::new(3).unwrap()),
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "both slow-timeout and bench.slow-timeout can be set independently"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            bench.slow-timeout = "30s"
        "#},
        DEFAULT_TEST_SLOW_TIMEOUT,
        ExpectedBenchTimeout::Exact(SlowTimeout {
            period: Duration::from_secs(30),
            terminate_after: None,
            grace_period: Duration::from_secs(10),
            on_timeout: SlowTimeoutResult::Fail,
        })
        ; "bench.slow-timeout string notation"
    )]
    fn bench_slowtimeout_is_independent(
        config_contents: &str,
        expected_test_timeout: SlowTimeout,
        expected_bench_timeout: ExpectedBenchTimeout,
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
            profile.slow_timeout(NextestRunMode::Test),
            expected_test_timeout,
            "Test mode slow-timeout mismatch"
        );

        let actual_bench_timeout = profile.slow_timeout(NextestRunMode::Benchmark);
        match expected_bench_timeout {
            ExpectedBenchTimeout::Exact(expected) => {
                assert_eq!(
                    actual_bench_timeout, expected,
                    "Benchmark mode slow-timeout mismatch"
                );
            }
            ExpectedBenchTimeout::VeryLarge => {
                // The default is "30y" which humantime parses accounting for
                // leap years, so it is slightly larger than VERY_LARGE.
                assert!(
                    actual_bench_timeout.period >= SlowTimeout::VERY_LARGE.period,
                    "Benchmark mode slow-timeout should be >= VERY_LARGE, got {:?}",
                    actual_bench_timeout.period
                );
                assert_eq!(
                    actual_bench_timeout.terminate_after, None,
                    "Benchmark mode terminate_after should be None"
                );
            }
        }
    }
}
