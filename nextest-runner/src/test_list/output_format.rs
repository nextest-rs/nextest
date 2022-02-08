// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// clippy complains about the Arbitrary impl for OutputFormat
#![allow(clippy::unit_arg)]

use crate::errors::OutputFormatParseError;
use serde::Serialize;
use std::{fmt, io, str::FromStr};

/// Output formats for nextest.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum OutputFormat {
    /// A plain, user-readable output format.
    Plain,

    /// Machine-readable output format.
    Serializable(SerializableFormat),
}

impl OutputFormat {
    /// Returns string representations of all currently-known, valid variants.
    pub fn variants() -> &'static [&'static str] {
        &["plain", "json", "json-pretty"]
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Plain => write!(f, "plain"),
            OutputFormat::Serializable(SerializableFormat::Json) => write!(f, "json"),
            OutputFormat::Serializable(SerializableFormat::JsonPretty) => write!(f, "json-pretty"),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = OutputFormatParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "plain" => OutputFormat::Plain,
            "json" => OutputFormat::Serializable(SerializableFormat::Json),
            "json-pretty" => OutputFormat::Serializable(SerializableFormat::JsonPretty),
            other => return Err(OutputFormatParseError::new(other)),
        };
        Ok(val)
    }
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Plain
    }
}

/// A serialized, machine-readable output format.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum SerializableFormat {
    /// JSON with no whitespace.
    Json,
    /// JSON, prettified.
    JsonPretty,
}

impl SerializableFormat {
    /// Write this data in the given format to the writer.
    pub fn to_writer(
        self,
        value: &impl Serialize,
        writer: impl io::Write,
    ) -> serde_json::Result<()> {
        match self {
            SerializableFormat::Json => serde_json::to_writer(writer, value),
            SerializableFormat::JsonPretty => serde_json::to_writer_pretty(writer, value),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn output_format_variants() {
        for &variant in OutputFormat::variants() {
            variant.parse::<OutputFormat>().expect("variant is valid");
        }
    }
}
