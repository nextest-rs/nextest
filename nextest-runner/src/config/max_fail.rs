use crate::{config::fail_fast::FailFast, errors::MaxFailParseError};
use serde::Deserialize;
use std::{cmp::Ordering, fmt, str::FromStr};

/// Type for the max-fail flag
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxFail {
    /// Allow a specific number of tests to fail before exiting.
    Count(usize),

    /// Run all tests. Equivalent to --no-fast-fail.
    All,
}

impl MaxFail {
    /// Returns the max-fail corresponding to the fail-fast.
    pub fn from_fail_fast(fail_fast: FailFast) -> Self {
        match fail_fast {
            FailFast::Boolean(true) => Self::Count(1),
            _ => Self::All,
        }
    }

    /// Returns true if the max-fail has been exceeded.
    pub fn is_exceeded(&self, failed: usize) -> bool {
        match self {
            Self::Count(n) => failed >= *n,
            Self::All => false,
        }
    }
}

impl FromStr for MaxFail {
    type Err = MaxFailParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.to_lowercase() == "all" {
            return Ok(Self::All);
        }

        match s.parse::<isize>() {
            Err(e) => Err(MaxFailParseError::new(format!("Error: {e} parsing {s}"))),
            Ok(j) if j <= 0 => Err(MaxFailParseError::new("max-fail may not be <= 0")),
            Ok(j) => Ok(MaxFail::Count(j as usize)),
        }
    }
}

impl fmt::Display for MaxFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => write!(f, "all"),
            Self::Count(n) => write!(f, "{n}"),
        }
    }
}

impl<'de> Deserialize<'de> for MaxFail {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;

        impl serde::de::Visitor<'_> for V {
            type Value = MaxFail;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "an integer or the string \"all\"")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                MaxFail::from_str(v).map_err(serde::de::Error::custom)
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v.cmp(&0) {
                    Ordering::Equal | Ordering::Less => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Unsigned(v as u64),
                        &"a positive integer",
                    )),
                    Ordering::Greater => Ok(MaxFail::Count(v as usize)),
                }
            }
        }

        deserializer.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maxfail_builder_from_str() {
        let successes = vec![
            ("all", MaxFail::All),
            ("ALL", MaxFail::All),
            ("1", MaxFail::Count(1)),
        ];

        let failures = vec!["-1", "0", "foo"];

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
}
