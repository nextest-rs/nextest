// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::PackageSpecError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};

/// A package specifier provided after `-p` or `--exclude` on the command line.
#[derive(Clone, Debug)]
pub struct PackageSpec {
    raw: String,
}

// Equality and hashing use `raw` only.
impl PartialEq for PackageSpec {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Eq for PackageSpec {}

impl Hash for PackageSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl PackageSpec {
    /// Creates a new `PackageSpec` from the given string.
    pub fn new(spec: &str) -> Result<Self, PackageSpecError> {
        Ok(Self {
            raw: spec.to_owned(),
        })
    }

    /// Returns the string representation of this package spec.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for PackageSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FromStr for PackageSpec {
    type Err = PackageSpecError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        Self::new(spec)
    }
}

impl Serialize for PackageSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for PackageSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(PackageSpecVisitor)
    }
}

struct PackageSpecVisitor;

impl de::Visitor<'_> for PackageSpecVisitor {
    type Value = PackageSpec;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a Cargo package spec (a package name, name@version, URL, or glob pattern)")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        PackageSpec::new(value).map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let inputs = [
            "foo",
            "foo@1.2",
            "https://github.com/rust-lang/cargo#0.52.0",
            "nextest-*",
        ];

        for input in inputs {
            let spec = PackageSpec::new(input).expect("spec is valid");
            assert_eq!(spec.as_str(), input, "as_str matches {input}");
            assert_eq!(spec.to_string(), input, "Display matches {input}");
            assert_eq!(
                input.parse::<PackageSpec>().expect("spec parses"),
                spec,
                "FromStr matches new for {input}"
            );
            let json = serde_json::to_string(&spec).expect("serialized spec");
            assert_eq!(
                json,
                serde_json::to_string(input).expect("serialized input string"),
                "spec serializes as a plain string for {input}"
            );
            let deserialized: PackageSpec = serde_json::from_str(&json).expect("deserialized spec");
            assert_eq!(deserialized, spec, "round-trip matches for {input}");
        }
    }

    #[test]
    fn deserialize_non_string() {
        let error =
            serde_json::from_str::<PackageSpec>("42").expect_err("non-string fails to deserialize");
        assert_eq!(
            error.to_string(),
            "invalid type: integer `42`, expected a Cargo package spec \
             (a package name, name@version, URL, or glob pattern) at line 1 column 2",
        );
    }
}
