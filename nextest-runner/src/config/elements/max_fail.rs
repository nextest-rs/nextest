// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::MaxFailParseError;
use serde::Deserialize;
use std::{fmt, str::FromStr};

/// Type for the max-fail flag and fail-fast configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxFail {
    /// Allow a specific number of tests to fail before exiting.
    Count {
        /// The maximum number of tests that can fail before exiting.
        max_fail: usize,
        /// Whether to terminate running tests immediately or wait for them to complete.
        terminate: TerminateMode,
    },

    /// Run all tests. Equivalent to --no-fast-fail.
    All,
}

impl MaxFail {
    /// Returns the max-fail corresponding to the fail-fast.
    pub fn from_fail_fast(fail_fast: bool) -> Self {
        if fail_fast {
            Self::Count {
                max_fail: 1,
                terminate: TerminateMode::Wait,
            }
        } else {
            Self::All
        }
    }

    /// Returns the terminate mode if the max-fail has been exceeded, or None otherwise.
    pub fn is_exceeded(&self, failed: usize) -> Option<TerminateMode> {
        match self {
            Self::Count {
                max_fail,
                terminate,
            } => (failed >= *max_fail).then_some(*terminate),
            Self::All => None,
        }
    }
}

impl FromStr for MaxFail {
    type Err = MaxFailParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.to_lowercase() == "all" {
            return Ok(Self::All);
        }

        // Check for N:mode syntax
        let (count_str, terminate) = if let Some((count, mode_str)) = s.split_once(':') {
            (count, mode_str.parse()?)
        } else {
            (s, TerminateMode::default())
        };

        // Parse and validate count
        let max_fail = count_str
            .parse::<isize>()
            .map_err(|e| MaxFailParseError::new(format!("{e} parsing '{count_str}'")))?;

        if max_fail <= 0 {
            return Err(MaxFailParseError::new("max-fail may not be <= 0"));
        }

        Ok(MaxFail::Count {
            max_fail: max_fail as usize,
            terminate,
        })
    }
}

impl fmt::Display for MaxFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => write!(f, "all"),
            Self::Count {
                max_fail,
                terminate,
            } => {
                if *terminate == TerminateMode::default() {
                    write!(f, "{max_fail}")
                } else {
                    write!(f, "{max_fail}:{terminate}")
                }
            }
        }
    }
}

/// Mode for terminating running tests when max-fail is exceeded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminateMode {
    /// Wait for running tests to complete (default)
    #[default]
    Wait,
    /// Terminate running tests immediately
    Immediate,
}

impl fmt::Display for TerminateMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wait => write!(f, "wait"),
            Self::Immediate => write!(f, "immediate"),
        }
    }
}

impl FromStr for TerminateMode {
    type Err = MaxFailParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "wait" => Ok(Self::Wait),
            "immediate" => Ok(Self::Immediate),
            _ => Err(MaxFailParseError::new(format!(
                "invalid terminate mode '{}', expected 'wait' or 'immediate'",
                s
            ))),
        }
    }
}

/// Deserializes a fail-fast configuration.
pub(in crate::config) fn deserialize_fail_fast<'de, D>(
    deserializer: D,
) -> Result<Option<MaxFail>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<MaxFail>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a boolean or {{ max-fail = ... }}")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(MaxFail::from_fail_fast(v)))
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            let de = serde::de::value::MapAccessDeserializer::new(map);
            FailFastMap::deserialize(de).map(|helper| match helper.max_fail_count {
                MaxFailCount::Count(n) => Some(MaxFail::Count {
                    max_fail: n,
                    terminate: helper.terminate,
                }),
                MaxFailCount::All => Some(MaxFail::All),
            })
        }
    }

    deserializer.deserialize_any(V)
}

/// A deserializer for `{ max-fail = xyz, terminate = "..." }`.
#[derive(Deserialize)]
struct FailFastMap {
    #[serde(rename = "max-fail")]
    max_fail_count: MaxFailCount,
    #[serde(default)]
    terminate: TerminateMode,
}

/// Represents the max-fail count or "all".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MaxFailCount {
    Count(usize),
    All,
}

impl<'de> Deserialize<'de> for MaxFailCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;

        impl serde::de::Visitor<'_> for V {
            type Value = MaxFailCount;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a positive integer or the string \"all\"")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v == "all" {
                    return Ok(MaxFailCount::All);
                }

                // If v is a string that represents a number, suggest using the
                // integer form.
                if let Ok(val) = v.parse::<i64>() {
                    if val > 0 {
                        return Err(serde::de::Error::invalid_value(
                            serde::de::Unexpected::Str(v),
                            &"the string \"all\" (numbers must be specified without quotes)",
                        ));
                    } else {
                        return Err(serde::de::Error::invalid_value(
                            serde::de::Unexpected::Str(v),
                            &"the string \"all\" (numbers must be positive and without quotes)",
                        ));
                    }
                }

                Err(serde::de::Error::invalid_value(
                    serde::de::Unexpected::Str(v),
                    &"the string \"all\" or a positive integer",
                ))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v > 0 {
                    Ok(MaxFailCount::Count(v as usize))
                } else {
                    Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Signed(v),
                        &"a positive integer or the string \"all\"",
                    ))
                }
            }
        }

        deserializer.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{core::NextestConfig, utils::test_helpers::*},
        errors::ConfigParseErrorKind,
    };
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use test_case::test_case;

    #[test]
    fn maxfail_builder_from_str() {
        let successes = vec![
            ("all", MaxFail::All),
            ("ALL", MaxFail::All),
            (
                "1",
                MaxFail::Count {
                    max_fail: 1,
                    terminate: TerminateMode::Wait,
                },
            ),
            (
                "1:wait",
                MaxFail::Count {
                    max_fail: 1,
                    terminate: TerminateMode::Wait,
                },
            ),
            (
                "1:immediate",
                MaxFail::Count {
                    max_fail: 1,
                    terminate: TerminateMode::Immediate,
                },
            ),
            (
                "5:immediate",
                MaxFail::Count {
                    max_fail: 5,
                    terminate: TerminateMode::Immediate,
                },
            ),
        ];

        let failures = vec!["-1", "0", "foo", "1:invalid", "1:"];

        for (input, output) in successes {
            assert_eq!(
                MaxFail::from_str(input).unwrap_or_else(|err| panic!(
                    "expected input '{input}' to succeed, failed with: {err}"
                )),
                output,
                "success case '{input}' matches",
            );
        }

        for input in failures {
            MaxFail::from_str(input).expect_err(&format!("expected input '{input}' to fail"));
        }
    }

    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = true
        "#},
        MaxFail::Count { max_fail: 1, terminate: TerminateMode::Wait }
        ; "boolean true"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = false
        "#},
        MaxFail::All
        ; "boolean false"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 1 }
        "#},
        MaxFail::Count { max_fail: 1, terminate: TerminateMode::Wait }
        ; "max-fail 1"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 2 }
        "#},
        MaxFail::Count { max_fail: 2, terminate: TerminateMode::Wait }
        ; "max-fail 2"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "all" }
        "#},
        MaxFail::All
        ; "max-fail all"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 1, terminate = "wait" }
        "#},
        MaxFail::Count { max_fail: 1, terminate: TerminateMode::Wait }
        ; "max-fail 1 with explicit wait"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 1, terminate = "immediate" }
        "#},
        MaxFail::Count { max_fail: 1, terminate: TerminateMode::Immediate }
        ; "max-fail 1 with immediate"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 5, terminate = "immediate" }
        "#},
        MaxFail::Count { max_fail: 5, terminate: TerminateMode::Immediate }
        ; "max-fail 5 with immediate"
    )]
    fn parse_fail_fast(config_contents: &str, expected: MaxFail) {
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
        .expect("expected parsing to succeed");

        let profile = config
            .profile("custom")
            .unwrap()
            .apply_build_platforms(&build_platforms());

        assert_eq!(profile.max_fail(), expected);
    }

    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 0 }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: integer `0`, expected a positive integer or the string \"all\""
        ; "invalid zero max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = -1 }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: integer `-1`, expected a positive integer or the string \"all\""
        ; "invalid negative max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "" }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: string \"\", expected the string \"all\" or a positive integer"
        ; "empty string max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "1" }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: string \"1\", expected the string \"all\" (numbers must be specified without quotes)"
        ; "string as positive integer"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "0" }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: string \"0\", expected the string \"all\" (numbers must be positive and without quotes)"
        ; "zero string"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "invalid" }
        "#},
        "profile.custom.fail-fast.max-fail: invalid value: string \"invalid\", expected the string \"all\" or a positive integer"
        ; "invalid string max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = true }
        "#},
        "profile.custom.fail-fast.max-fail: invalid type: boolean `true`, expected a positive integer or the string \"all\""
        ; "invalid max-fail type"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { invalid-key = 1 }
        "#},
        r#"profile.custom.fail-fast: missing configuration field "profile.custom.fail-fast.max-fail""#
        ; "invalid map key"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = "true"
        "#},
        "profile.custom.fail-fast: invalid type: string \"true\", expected a boolean or { max-fail = ... }"
        ; "string boolean not allowed"
    )]
    fn invalid_fail_fast(config_contents: &str, error_str: &str) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            [],
            &Default::default(),
        )
        .expect_err("expected parsing to fail");

        let error = match error.kind() {
            ConfigParseErrorKind::DeserializeError(d) => d,
            _ => panic!("expected deserialize error, found {error:?}"),
        };

        assert_eq!(
            error.to_string(),
            error_str,
            "actual error matches expected"
        );
    }
}
