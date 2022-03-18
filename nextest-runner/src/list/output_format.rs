// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// clippy complains about the Arbitrary impl for OutputFormat
#![allow(clippy::unit_arg)]

use owo_colors::Style;
use serde::Serialize;
use std::io;

/// Output formats for nextest.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
#[non_exhaustive]
pub enum OutputFormat {
    /// A human-readable output format.
    Human {
        /// Whether to produce verbose output.
        verbose: bool,
    },

    /// Machine-readable output format.
    Serializable(SerializableFormat),
}

/// A serialized, machine-readable output format.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
#[non_exhaustive]
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

#[derive(Clone, Debug, Default)]
pub(crate) struct Styles {
    pub(crate) binary_id: Style,
    pub(crate) test_name: Style,
    pub(crate) module_path: Style,
    pub(crate) field: Style,
}

impl Styles {
    pub(crate) fn colorize(&mut self) {
        self.binary_id = Style::new().magenta().bold();
        self.test_name = Style::new().blue().bold();
        self.field = Style::new().yellow().bold();
        self.module_path = Style::new().cyan();
    }
}
