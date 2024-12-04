use crate::errors::MaxFailParseError;
use std::{fmt, str::FromStr};

/// Type for the max-fail flag
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxFail {
    /// Allow a specific number of tests to fail before exiting.
    Count(usize),

    /// Run all tests. Equivalent to --no-fast-fail.
    All,
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
