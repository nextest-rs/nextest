// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// clippy complains about the Arbitrary impl for OutputFormat
#![allow(clippy::unit_arg)]

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{fmt, io, str::FromStr};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum OutputFormat {
    Plain,
    Serializable(SerializableFormat),
}

impl OutputFormat {
    pub fn variants() -> [&'static str; 5] {
        ["plain", "json", "json-pretty", "toml", "toml-pretty"]
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Plain => write!(f, "plain"),
            OutputFormat::Serializable(SerializableFormat::Json) => write!(f, "json"),
            OutputFormat::Serializable(SerializableFormat::JsonPretty) => write!(f, "json-pretty"),
            OutputFormat::Serializable(SerializableFormat::Toml) => write!(f, "toml"),
            OutputFormat::Serializable(SerializableFormat::TomlPretty) => write!(f, "toml-pretty"),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "plain" => OutputFormat::Plain,
            "json" => OutputFormat::Serializable(SerializableFormat::Json),
            "json-pretty" => OutputFormat::Serializable(SerializableFormat::JsonPretty),
            "toml" => OutputFormat::Serializable(SerializableFormat::Toml),
            "toml-pretty" => OutputFormat::Serializable(SerializableFormat::TomlPretty),
            other => bail!("unrecognized format: {}", other),
        };
        Ok(val)
    }
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Plain
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub enum SerializableFormat {
    Json,
    JsonPretty,
    Toml,
    TomlPretty,
}

impl SerializableFormat {
    /// Write this data in the given format to the writer.
    pub fn to_writer(self, value: &impl Serialize, mut writer: impl io::Write) -> Result<()> {
        match self {
            SerializableFormat::Json => {
                serde_json::to_writer(writer, value).context("error serializing to JSON")
            }
            SerializableFormat::JsonPretty => {
                serde_json::to_writer_pretty(writer, value).context("error serializing to JSON")
            }
            SerializableFormat::Toml => {
                let s = toml::to_string(value).context("error serializing to TOML")?;
                write!(writer, "{}", s).context("error writing output")
            }
            SerializableFormat::TomlPretty => {
                let s = toml::to_string_pretty(value).context("error serializing to TOML")?;
                write!(writer, "{}", s).context("error writing output")
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn output_format_variants() {
        for &variant in &OutputFormat::variants() {
            variant.parse::<OutputFormat>().expect("variant is valid");
        }
    }

    proptest! {
        #[test]
        fn output_format_from_str_display_roundtrip(format in any::<OutputFormat>()) {
            let displayed = format!("{}", format);
            let format2 = displayed.parse::<OutputFormat>().expect("Display output is valid");
            prop_assert_eq!(format, format2, "Display -> FromStr roundtrips");
        }
    }
}
