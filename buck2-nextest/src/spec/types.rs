// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Buck2 external runner spec, as JSON.
//!
//! Buck2's orchestrator hands a test executor one spec per configured test
//! target, derived from the target's `ExternalRunnerTestInfo` provider. This is
//! the same payload Meta's `tpx` consumes. The types here mirror that shape so
//! `buck2-nextest` can read it from a file or stdin.

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{collections::BTreeMap, fmt};

/// The top-level payload: the set of test targets to run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Buck2TestSpec {
    /// One entry per configured test target.
    pub targets: Vec<ExternalRunnerSpec>,
}

/// The spec for a single configured test target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct ExternalRunnerSpec {
    /// The target this spec describes.
    pub target: ConfiguredTarget,

    /// The kind of test, from `ExternalRunnerTestInfo`'s `type` field.
    ///
    /// `buck2-nextest` requires `"rust"`, since the runner speaks the libtest
    /// protocol.
    pub test_type: String,

    /// The command to run the test binary.
    ///
    /// The first entry is the program; the rest are arguments placed before
    /// nextest's own libtest arguments.
    pub command: Vec<SpecValue>,

    /// Environment variables to set for the test process.
    #[serde(default)]
    pub env: BTreeMap<String, SpecValue>,

    /// Labels attached to the target.
    #[serde(default)]
    pub labels: Vec<String>,

    /// Contacts for the target.
    #[serde(default)]
    pub contacts: Vec<String>,

    /// The oncall rotation for the target, if any.
    #[serde(default)]
    pub oncall: Option<String>,

    /// The cell the test's working directory is relative to.
    #[serde(default)]
    pub working_dir_cell: Option<String>,
}

/// A configured target label.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct ConfiguredTarget {
    /// The cell the target lives in, e.g. `fbcode`.
    pub cell: String,

    /// The package path within the cell, e.g. `buck2/app`.
    pub package: String,

    /// The target name within the package, e.g. `my_test`.
    pub target: String,

    /// The configuration the target was built under.
    #[serde(default)]
    pub configuration: Option<String>,
}

impl ConfiguredTarget {
    /// Returns the target's label in Buck2's `cell//package:target` form.
    pub fn label(&self) -> String {
        format!("{}//{}:{}", self.cell, self.package, self.target)
    }
}

impl fmt::Display for ConfiguredTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// A value in a spec's command or environment.
///
/// Buck2 sends most values verbatim, but may instead send a handle that the
/// executor resolves by calling back into the orchestrator over gRPC. A
/// file-based payload has no such channel, so handles are rejected during
/// validation rather than at deserialization time -- that way the error can name
/// the target they came from.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SpecValue {
    /// A literal string.
    Verbatim(String),

    /// A handle to an argument the orchestrator holds.
    ArgHandle(u64),

    /// A handle to an environment value the orchestrator holds.
    EnvHandle(String),
}

impl SpecValue {
    /// Returns the literal string, if this value is verbatim.
    pub fn as_verbatim(&self) -> Option<&str> {
        match self {
            Self::Verbatim(value) => Some(value),
            Self::ArgHandle(_) | Self::EnvHandle(_) => None,
        }
    }

    /// Returns a short description of the value's kind, for error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Verbatim(_) => "verbatim",
            Self::ArgHandle(_) => "arg_handle",
            Self::EnvHandle(_) => "env_handle",
        }
    }
}

impl fmt::Display for SpecValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verbatim(value) => write!(f, "{value}"),
            Self::ArgHandle(handle) => write!(f, "<arg_handle {handle}>"),
            Self::EnvHandle(handle) => write!(f, "<env_handle {handle}>"),
        }
    }
}

// A hand-written visitor rather than `#[serde(untagged)]`, which produces poor
// error messages. A bare string is accepted as shorthand for `verbatim`, since
// that is overwhelmingly the common case.
impl<'de> Deserialize<'de> for SpecValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SpecValueVisitor)
    }
}

struct SpecValueVisitor;

impl<'de> Visitor<'de> for SpecValueVisitor {
    type Value = SpecValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a string, or a map with exactly one of \
             `verbatim`, `arg_handle`, or `env_handle`",
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SpecValue::Verbatim(value.to_owned()))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let Some(key) = map.next_key::<String>()? else {
            return Err(de::Error::invalid_length(0, &self));
        };

        let value = match key.as_str() {
            "verbatim" => SpecValue::Verbatim(map.next_value()?),
            "arg_handle" => SpecValue::ArgHandle(map.next_value()?),
            "env_handle" => SpecValue::EnvHandle(map.next_value()?),
            other => {
                return Err(de::Error::unknown_variant(
                    other,
                    &["verbatim", "arg_handle", "env_handle"],
                ));
            }
        };

        if let Some(extra) = map.next_key::<String>()? {
            return Err(de::Error::custom(format!(
                "expected exactly one of `verbatim`, `arg_handle`, or `env_handle`, \
                 found `{key}` and `{extra}`"
            )));
        }

        Ok(value)
    }
}
