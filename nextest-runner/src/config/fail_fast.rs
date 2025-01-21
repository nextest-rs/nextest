use crate::config::max_fail::MaxFail;
use serde::Deserialize;
use std::{fmt, str::FromStr};

/// Type for the fail-fast flag
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailFast {
    /// Stop on the first test failure
    Boolean(bool),
    /// Stop after a maximum number of test failures
    MaxFail,
}

impl FailFast {
    /// Returns the fail-fast corresponding to the max-fail value.
    pub fn from_max_fail(max_fail: MaxFail) -> Self {
        match max_fail {
            MaxFail::Count(1) => Self::Boolean(true),
            MaxFail::All => Self::Boolean(false),
            _ => Self::MaxFail,
        }
    }
}

impl FromStr for FailFast {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "true" => Ok(Self::Boolean(true)),
            "false" => Ok(Self::Boolean(false)),
            "maxfail" => Ok(Self::MaxFail),
            _ => Err(format!(
                "Invalid fail-fast value: {s}. Expected 'true', 'false' or 'maxfail'"
            )),
        }
    }
}

impl fmt::Display for FailFast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boolean(true) => write!(f, "true"),
            Self::Boolean(false) => write!(f, "false"),
            Self::MaxFail => write!(f, "maxfail"),
        }
    }
}

#[derive(Deserialize)]
struct FailFastHelper {
    #[serde(rename = "max-fail")]
    max_fail: MaxFail,
}

impl<'de> Deserialize<'de> for FailFast {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;

        impl<'de> serde::de::Visitor<'de> for V {
            type Value = FailFast;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a boolean or a max-fail configuration")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FailFast::Boolean(v))
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let helper =
                    FailFastHelper::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;

                Ok(FailFast::from_max_fail(helper.max_fail))
            }
        }

        deserializer.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::FailFast;
    use crate::config::{
        test_helpers::{build_platforms, temp_workspace},
        NextestConfig,
    };
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use test_case::test_case;

    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = true
        "#},
        Some(FailFast::Boolean(true))
        ; "boolean true"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = false
        "#},
        Some(FailFast::Boolean(false))
        ; "boolean false"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 1 }
        "#},
        Some(FailFast::Boolean(true))
        ; "max-fail 1 converts to boolean"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 2 }
        "#},
        Some(FailFast::MaxFail)
        ; "max-fail 2 remains max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "all" }
        "#},
        Some(FailFast::Boolean(false))
        ; "max-fail all"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "ALL" }
        "#},
        Some(FailFast::Boolean(false))
        ; "max-fail ALL uppercase"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = 0 }
        "#},
        None
        ; "invalid zero max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = -1 }
        "#},
        None
        ; "invalid negative max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "" }
        "#},
        None
        ; "empty string max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { max-fail = "invalid" }
        "#},
        None
        ; "invalid string max-fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = { invalid-key = 1 }
        "#},
        None
        ; "invalid map key"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            fail-fast = "true"
        "#},
        None
        ; "string boolean not allowed"
    )]
    fn parse_fail_fast(config_contents: &str, fail_fast: Option<FailFast>) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(workspace_dir.path(), config_contents);

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &graph,
            None,
            [],
            &Default::default(),
        );

        match fail_fast {
            None => assert!(config.is_err()),
            Some(expected) => {
                let config = config.unwrap();
                let profile = config
                    .profile("custom")
                    .unwrap()
                    .apply_build_platforms(&build_platforms());

                assert_eq!(profile.fail_fast(), expected);
            }
        }
    }
}
