// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt, time::Duration};

/// Type for the retry config key.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RetryPolicy {
    /// Fixed backoff.
    Fixed {
        /// Maximum retry count.
        count: u32,

        /// Delay between retries.
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        jitter: bool,
    },

    /// Exponential backoff.
    Exponential {
        /// Maximum retry count.
        count: u32,

        /// Delay between retries. Not optional for exponential backoff.
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        jitter: bool,

        /// If set, limits the delay between retries.
        max_delay: Option<Duration>,
    },
}

impl Default for RetryPolicy {
    #[inline]
    fn default() -> Self {
        Self::new_without_delay(0)
    }
}

impl RetryPolicy {
    /// Create new policy with no delay between retries.
    pub fn new_without_delay(count: u32) -> Self {
        Self::Fixed {
            count,
            delay: Duration::ZERO,
            jitter: false,
        }
    }

    /// Returns the number of retries.
    pub fn count(&self) -> u32 {
        match self {
            Self::Fixed { count, .. } | Self::Exponential { count, .. } => *count,
        }
    }
}

/// Controls whether a flaky test is treated as a pass or a failure.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum FlakyResult {
    /// The test is marked as passed.
    #[default]
    Pass,
}

/// Serde-compatible intermediate type for the `retries` config field. After
/// deserialization, this is converted into a `RetryPolicy`.
#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(tag = "backoff", rename_all = "kebab-case", deny_unknown_fields)]
enum RetryPolicySerde {
    #[serde(rename_all = "kebab-case")]
    Fixed {
        count: u32,
        #[serde(default, with = "humantime_serde")]
        delay: Duration,
        #[serde(default)]
        jitter: bool,
    },
    #[serde(rename_all = "kebab-case")]
    Exponential {
        count: u32,
        #[serde(with = "humantime_serde")]
        delay: Duration,
        #[serde(default)]
        jitter: bool,
        #[serde(default, with = "humantime_serde")]
        max_delay: Option<Duration>,
    },
}

impl RetryPolicySerde {
    fn into_policy(self) -> RetryPolicy {
        match self {
            RetryPolicySerde::Fixed {
                count,
                delay,
                jitter,
            } => RetryPolicy::Fixed {
                count,
                delay,
                jitter,
            },
            RetryPolicySerde::Exponential {
                count,
                delay,
                jitter,
                max_delay,
            } => RetryPolicy::Exponential {
                count,
                delay,
                jitter,
                max_delay,
            },
        }
    }
}

pub(in crate::config) fn deserialize_retry_policy<'de, D>(
    deserializer: D,
) -> Result<Option<RetryPolicy>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<RetryPolicy>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ backoff = \"fixed\", count = 5 }}) or a number (5)"
            )
        }

        // Note that TOML uses i64, not u64.
        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match v.cmp(&0) {
                Ordering::Greater | Ordering::Equal => {
                    let v = u32::try_from(v).map_err(|_| {
                        serde::de::Error::invalid_value(
                            serde::de::Unexpected::Signed(v),
                            &"a positive u32",
                        )
                    })?;
                    Ok(Some(RetryPolicy::new_without_delay(v)))
                }
                Ordering::Less => Err(serde::de::Error::invalid_value(
                    serde::de::Unexpected::Signed(v),
                    &self,
                )),
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            RetryPolicySerde::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                .map(|s| Some(s.into_policy()))
        }
    }

    // Post-deserialize validation of retry policy.
    let policy = deserializer.deserialize_any(V)?;
    match &policy {
        Some(RetryPolicy::Fixed {
            count: _,
            delay,
            jitter,
        }) => {
            // Jitter can't be specified if delay is 0.
            if delay.is_zero() && *jitter {
                return Err(serde::de::Error::custom(
                    "`jitter` cannot be true if `delay` isn't specified or is zero",
                ));
            }
        }
        Some(RetryPolicy::Exponential {
            count,
            delay,
            jitter: _,
            max_delay,
        }) => {
            // Count can't be zero.
            if *count == 0 {
                return Err(serde::de::Error::custom(
                    "`count` cannot be zero with exponential backoff",
                ));
            }
            // Delay can't be zero.
            if delay.is_zero() {
                return Err(serde::de::Error::custom(
                    "`delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay, if specified, can't be zero.
            if max_delay.is_some_and(|f| f.is_zero()) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay can't be less than delay.
            if max_delay.is_some_and(|max_delay| max_delay < *delay) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be less than delay with exponential backoff",
                ));
            }
        }
        None => {}
    }

    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{core::NextestConfig, utils::test_helpers::*},
        errors::ConfigParseErrorKind,
        run_mode::NextestRunMode,
    };
    use camino_tempfile::tempdir;
    use config::ConfigError;
    use guppy::graph::cargo::BuildPlatform;
    use indoc::indoc;
    use nextest_filtering::{ParseContext, TestQuery};
    use nextest_metadata::TestCaseName;
    use test_case::test_case;

    #[test]
    fn parse_retries_valid() {
        let config_contents = indoc! {r#"
            [profile.default]
            retries = { backoff = "fixed", count = 3 }

            [profile.no-retries]
            retries = 0

            [profile.fixed-with-delay]
            retries = { backoff = "fixed", count = 3, delay = "1s" }

            [profile.exp]
            retries = { backoff = "exponential", count = 4, delay = "2s" }

            [profile.exp-with-max-delay]
            retries = { backoff = "exponential", count = 5, delay = "3s", max-delay = "10s" }

            [profile.exp-with-max-delay-and-jitter]
            retries = { backoff = "exponential", count = 6, delay = "4s", max-delay = "1m", jitter = true }

            [profile.with-flaky-result-pass]
            retries = { backoff = "fixed", count = 2 }
            flaky-result = "pass"

            [profile.flaky-result-only]
            flaky-result = "pass"
        "#};

        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            [],
            &Default::default(),
        )
        .expect("config is valid");

        let default_profile = config
            .profile("default")
            .expect("default profile exists")
            .apply_build_platforms(&build_platforms());
        assert_eq!(
            default_profile.retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::ZERO,
                jitter: false,
            },
            "default retries matches"
        );
        assert_eq!(
            default_profile.flaky_result(),
            FlakyResult::Pass,
            "default flaky_result matches"
        );

        assert_eq!(
            config
                .profile("no-retries")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::new_without_delay(0),
            "no-retries retries matches"
        );

        assert_eq!(
            config
                .profile("fixed-with-delay")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::from_secs(1),
                jitter: false,
            },
            "fixed-with-delay retries matches"
        );

        assert_eq!(
            config
                .profile("exp")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 4,
                delay: Duration::from_secs(2),
                jitter: false,
                max_delay: None,
            },
            "exp retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 5,
                delay: Duration::from_secs(3),
                jitter: false,
                max_delay: Some(Duration::from_secs(10)),
            },
            "exp-with-max-delay retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay-and-jitter")
                .expect("profile exists")
                .apply_build_platforms(&build_platforms())
                .retries(),
            RetryPolicy::Exponential {
                count: 6,
                delay: Duration::from_secs(4),
                jitter: true,
                max_delay: Some(Duration::from_secs(60)),
            },
            "exp-with-max-delay-and-jitter retries matches"
        );

        let with_flaky_result_pass = config
            .profile("with-flaky-result-pass")
            .expect("profile exists")
            .apply_build_platforms(&build_platforms());
        assert_eq!(
            with_flaky_result_pass.retries(),
            RetryPolicy::new_without_delay(2),
            "with-flaky-result-pass retries matches"
        );
        assert_eq!(
            with_flaky_result_pass.flaky_result(),
            FlakyResult::Pass,
            "with-flaky-result-pass flaky_result matches"
        );

        // flaky-result-only: retries inherited from default (count=3), flaky
        // result set to pass.
        let flaky_result_only = config
            .profile("flaky-result-only")
            .expect("profile exists")
            .apply_build_platforms(&build_platforms());
        assert_eq!(
            flaky_result_only.retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::ZERO,
                jitter: false,
            },
            "flaky-result-only retries inherited from default"
        );
        assert_eq!(
            flaky_result_only.flaky_result(),
            FlakyResult::Pass,
            "flaky-result-only flaky_result matches"
        );
    }

    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "foo" }
        "#},
        ConfigErrorKind::Message,
        "unknown variant `foo`, expected `fixed` or `exponential`"
        ; "invalid value for backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed" }
        "#},
        ConfigErrorKind::NotFound,
        "profile.default.retries.count"
        ; "fixed specified without count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, delay = "foobar" }
        "#},
        ConfigErrorKind::Message,
        "invalid value: string \"foobar\", expected a duration"
        ; "delay is not a valid duration")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, jitter = true }
        "#},
        ConfigErrorKind::Message,
        "`jitter` cannot be true if `delay` isn't specified or is zero"
        ; "jitter specified without delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, max-delay = "10s" }
        "#},
        ConfigErrorKind::Message,
        "unknown field `max-delay`, expected one of `count`, `delay`, `jitter`"
        ; "max-delay is incompatible with fixed backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1 }
        "#},
        ConfigErrorKind::NotFound,
        "profile.default.retries.delay"
        ; "exponential backoff must specify delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", delay = "1s" }
        "#},
        ConfigErrorKind::NotFound,
        "profile.default.retries.count"
        ; "exponential backoff must specify count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 0, delay = "1s" }
        "#},
        ConfigErrorKind::Message,
        "`count` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "0s" }
        "#},
        ConfigErrorKind::Message,
        "`delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "1s", max-delay = "0s" }
        "#},
        ConfigErrorKind::Message,
        "`max-delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero max delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "4s", max-delay = "2s", jitter = true }
        "#},
        ConfigErrorKind::Message,
        "`max-delay` cannot be less than delay"
        ; "max-delay greater than delay")]
    fn parse_retries_invalid(
        config_contents: &str,
        expected_kind: ConfigErrorKind,
        expected_message: &str,
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let config_err = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            [],
            &Default::default(),
        )
        .expect_err("config expected to be invalid");

        let message = match config_err.kind() {
            ConfigParseErrorKind::DeserializeError(path_error) => {
                match (path_error.inner(), expected_kind) {
                    (ConfigError::Message(message), ConfigErrorKind::Message) => message,
                    (ConfigError::NotFound(message), ConfigErrorKind::NotFound) => message,
                    (other, expected) => {
                        panic!(
                            "for config error {config_err:?}, expected \
                             ConfigErrorKind::{expected:?} for inner error {other:?}"
                        );
                    }
                }
            }
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::DeserializeError"
                );
            }
        };

        assert!(
            message.contains(expected_message),
            "expected message \"{message}\" to contain \"{expected_message}\""
        );
    }

    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(2)

        ; "my_test matches exactly"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "!test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(0)

        ; "not match"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(0)

        ; "no retries specified"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(2)

        ; "earlier configs override later ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "test(=my_test)"
            retries = 3
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(3)

        ; "profile-specific configs override default ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "(!package(test-package)) and test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "!test(=my_test_2)"
            retries = 3
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(3)

        ; "no overrides match my_test exactly"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "x86_64-unknown-linux-gnu"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Host,
        RetryPolicy::new_without_delay(2)

        ; "earlier config applied because it matches host triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "aarch64-apple-darwin"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Host,
        RetryPolicy::new_without_delay(3)

        ; "earlier config ignored because it doesn't match host triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "aarch64-apple-darwin"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(2)

        ; "earlier config applied because it matches target triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = "x86_64-unknown-linux-gnu"
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(3)

        ; "earlier config ignored because it doesn't match target triple"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = 'cfg(target_os = "macos")'
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(2)

        ; "earlier config applied because it matches target cfg expr"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            platform = 'cfg(target_arch = "x86_64")'
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        BuildPlatform::Target,
        RetryPolicy::new_without_delay(3)

        ; "earlier config ignored because it doesn't match target cfg expr"
    )]
    fn overrides_retries(
        config_contents: &str,
        build_platform: BuildPlatform,
        retries: RetryPolicy,
    ) {
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();
        let pcx = ParseContext::new(&graph);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .unwrap();
        let binary_query = binary_query(&graph, package_id, "lib", "my-binary", build_platform);
        let test_name = TestCaseName::new("my_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let profile = config
            .profile("ci")
            .expect("ci profile is defined")
            .apply_build_platforms(&build_platforms());
        let settings_for = profile.settings_for(NextestRunMode::Test, &query);
        assert_eq!(
            settings_for.retries(),
            retries,
            "actual retries don't match expected retries"
        );
    }

    #[test]
    fn overrides_flaky_result() {
        let config_contents = indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = { backoff = "fixed", count = 3 }
            flaky-result = "pass"

            [[profile.default.overrides]]
            filter = "test(=other_test)"
            retries = 2

            [profile.ci]
        "#};
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();
        let pcx = ParseContext::new(&graph);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .unwrap();

        let profile = config
            .profile("ci")
            .expect("ci profile is defined")
            .apply_build_platforms(&build_platforms());

        // my_test has flaky-result = "pass" set explicitly.
        let binary_query = binary_query(
            &graph,
            package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );
        let test_name = TestCaseName::new("my_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let settings = profile.settings_for(NextestRunMode::Test, &query);
        assert_eq!(
            settings.flaky_result(),
            FlakyResult::Pass,
            "my_test flaky_result is pass"
        );

        // other_test uses shorthand retries = 2, which does not set
        // flaky-result.
        let test_name = TestCaseName::new("other_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let settings = profile.settings_for(NextestRunMode::Test, &query);
        assert_eq!(
            settings.flaky_result(),
            FlakyResult::Pass,
            "other_test flaky_result defaults to pass"
        );
    }

    /// Test that retries and flaky_result resolve independently through the
    /// override chain. An override that sets only retries should not override
    /// a flaky_result set by a later (lower-priority) override.
    #[test]
    fn overrides_flaky_result_independent_resolution() {
        let config_contents = indoc! {r#"
            # Override 1: sets retries count only.
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 5

            # Override 2: sets retries with flaky-result = "pass".
            [[profile.default.overrides]]
            filter = "all()"
            retries = { backoff = "fixed", count = 2 }
            flaky-result = "pass"

            [profile.ci]
        "#};
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();
        let pcx = ParseContext::new(&graph);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .unwrap();

        let profile = config
            .profile("ci")
            .expect("ci profile is defined")
            .apply_build_platforms(&build_platforms());

        let binary_query = binary_query(
            &graph,
            package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );
        let test_name = TestCaseName::new("my_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let settings = profile.settings_for(NextestRunMode::Test, &query);

        // Retries count comes from override 1 (higher priority).
        assert_eq!(
            settings.retries(),
            RetryPolicy::new_without_delay(5),
            "retries count from first override"
        );
        // Flaky result comes from override 2 (first override didn't set it).
        assert_eq!(
            settings.flaky_result(),
            FlakyResult::Pass,
            "flaky_result from second override"
        );
    }

    /// Test that `flaky-result = "pass"` (without retries) sets only the flaky
    /// result, with the retry policy inherited from a lower-priority override.
    #[test]
    fn overrides_flaky_result_only() {
        let config_contents = indoc! {r#"
            # Override 1: sets only flaky-result, no retry policy.
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            flaky-result = "pass"

            # Override 2: sets retries count for all tests.
            [[profile.default.overrides]]
            filter = "all()"
            retries = 3

            [profile.ci]
        "#};
        let workspace_dir = tempdir().unwrap();

        let graph = temp_workspace(&workspace_dir, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();
        let pcx = ParseContext::new(&graph);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .unwrap();

        let profile = config
            .profile("ci")
            .expect("ci profile is defined")
            .apply_build_platforms(&build_platforms());

        let binary_query = binary_query(
            &graph,
            package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );
        let test_name = TestCaseName::new("my_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let settings = profile.settings_for(NextestRunMode::Test, &query);

        // Retries come from override 2 (override 1 didn't set a policy).
        assert_eq!(
            settings.retries(),
            RetryPolicy::new_without_delay(3),
            "retries from second override"
        );
        // Flaky result comes from override 1.
        assert_eq!(
            settings.flaky_result(),
            FlakyResult::Pass,
            "flaky_result from first override"
        );

        // For a test that doesn't match override 1, flaky_result defaults to
        // pass.
        let test_name = TestCaseName::new("other_test");
        let query = TestQuery {
            binary_query: binary_query.to_query(),
            test_name: &test_name,
        };
        let settings = profile.settings_for(NextestRunMode::Test, &query);
        assert_eq!(
            settings.retries(),
            RetryPolicy::new_without_delay(3),
            "other_test retries from second override"
        );
        assert_eq!(
            settings.flaky_result(),
            FlakyResult::Pass,
            "other_test flaky_result defaults to pass"
        );
    }
}
