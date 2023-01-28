// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::InvalidIdentifier;
use smol_str::SmolStr;
use std::fmt;
use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

/// An identifier used in configuration.
///
/// The identifier goes through some basic validation:
/// * conversion to NFC
/// * ensuring that it is of the form (XID_Start)(XID_Continue | -)*
///
/// Identifiers can also be tool identifiers, which are of the form "@tool:tool-name:identifier".
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigIdentifier(SmolStr);

impl ConfigIdentifier {
    /// Validates and creates a new identifier.
    pub fn new(identifier: SmolStr) -> Result<Self, InvalidIdentifier> {
        let identifier = if is_nfc_quick(identifier.chars()) == IsNormalized::Yes {
            identifier
        } else {
            identifier.nfc().collect::<SmolStr>()
        };

        if identifier.is_empty() {
            return Err(InvalidIdentifier::Empty);
        }

        // Tool identifiers are of the form "@tool:identifier:identifier".

        if let Some(suffix) = identifier.strip_prefix("@tool:") {
            // TODO: finish this
            let mut parts = suffix.splitn(2, ':');
            let tool_name = parts
                .next()
                .expect("at least one identifier should be returned.");
            let tool_identifier = match parts.next() {
                Some(tool_identifier) => tool_identifier,
                None => return Err(InvalidIdentifier::ToolIdentifierInvalidFormat(identifier)),
            };

            for x in [tool_name, tool_identifier] {
                Self::is_valid_unicode(x).map_err(|error| match error {
                    InvalidIdentifierKind::Empty => {
                        InvalidIdentifier::ToolComponentEmpty(identifier.clone())
                    }
                    InvalidIdentifierKind::InvalidXid => {
                        InvalidIdentifier::ToolIdentifierInvalidXid(identifier.clone())
                    }
                })?;
            }
        } else {
            // This should be a regular identifier.
            Self::is_valid_unicode(&identifier).map_err(|error| match error {
                InvalidIdentifierKind::Empty => InvalidIdentifier::Empty,
                InvalidIdentifierKind::InvalidXid => {
                    InvalidIdentifier::InvalidXid(identifier.clone())
                }
            })?;
        }

        Ok(Self(identifier))
    }

    /// Returns true if this is a tool identifier.
    pub fn is_tool_identifier(&self) -> bool {
        self.0.starts_with("@tool:")
    }

    /// Returns the tool name and identifier, if this is a tool identifier.
    pub fn tool_components(&self) -> Option<(&str, &str)> {
        self.0.strip_prefix("@tool:").map(|suffix| {
            let mut parts = suffix.splitn(2, ':');
            let tool_name = parts
                .next()
                .expect("identifier was checked to have 2 components above");
            let tool_identifier = parts
                .next()
                .expect("identifier was checked to have 2 components above");
            (tool_name, tool_identifier)
        })
    }

    /// Returns the identifier as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid_unicode(identifier: &str) -> Result<(), InvalidIdentifierKind> {
        if identifier.is_empty() {
            return Err(InvalidIdentifierKind::Empty);
        }

        let mut first = true;
        for ch in identifier.chars() {
            if first {
                if !unicode_ident::is_xid_start(ch) {
                    return Err(InvalidIdentifierKind::InvalidXid);
                }
                first = false;
            } else if !(ch == '-' || unicode_ident::is_xid_continue(ch)) {
                return Err(InvalidIdentifierKind::InvalidXid);
            }
        }
        Ok(())
    }
}

impl fmt::Display for ConfigIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ConfigIdentifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let identifier = SmolStr::deserialize(deserializer)?;
        ConfigIdentifier::new(identifier).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug)]
enum InvalidIdentifierKind {
    Empty,
    InvalidXid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq, Eq)]
    struct TestDeserialize {
        identifier: ConfigIdentifier,
    }

    fn make_json(identifier: &str) -> String {
        format!(r#"{{ "identifier": "{identifier}" }}"#)
    }

    #[test]
    fn test_valid() {
        let valid_inputs = ["foo", "foo-bar", "Δabc"];

        for &input in &valid_inputs {
            let identifier = ConfigIdentifier::new(input.into()).unwrap();
            assert_eq!(identifier.as_str(), input);
            assert!(!identifier.is_tool_identifier());

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap();
        }

        let valid_tool_inputs = ["@tool:foo:bar", "@tool:Δabc_def-ghi:foo-bar"];

        for &input in &valid_tool_inputs {
            let identifier = ConfigIdentifier::new(input.into()).unwrap();
            assert_eq!(identifier.as_str(), input);
            assert!(identifier.is_tool_identifier());

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap();
        }
    }

    #[test]
    fn test_invalid() {
        let identifier = ConfigIdentifier::new("".into());
        assert_eq!(identifier.unwrap_err(), InvalidIdentifier::Empty);

        let invalid_xid = ["foo bar", "_", "-foo", "_foo", "@foo", "@tool"];

        for &input in &invalid_xid {
            let identifier = ConfigIdentifier::new(input.into());
            assert_eq!(
                identifier.unwrap_err(),
                InvalidIdentifier::InvalidXid(input.into())
            );

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap_err();
        }

        let tool_component_empty = ["@tool::", "@tool:foo:", "@tool::foo"];

        for &input in &tool_component_empty {
            let identifier = ConfigIdentifier::new(input.into());
            assert_eq!(
                identifier.unwrap_err(),
                InvalidIdentifier::ToolComponentEmpty(input.into())
            );

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap_err();
        }

        let tool_identifier_invalid_format = ["@tool:", "@tool:foo"];

        for &input in &tool_identifier_invalid_format {
            let identifier = ConfigIdentifier::new(input.into());
            assert_eq!(
                identifier.unwrap_err(),
                InvalidIdentifier::ToolIdentifierInvalidFormat(input.into())
            );

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap_err();
        }

        let tool_identifier_invalid_xid = ["@tool:_foo:bar", "@tool:foo:#bar", "@tool:foo:bar:baz"];

        for &input in &tool_identifier_invalid_xid {
            let identifier = ConfigIdentifier::new(input.into());
            assert_eq!(
                identifier.unwrap_err(),
                InvalidIdentifier::ToolIdentifierInvalidXid(input.into())
            );

            serde_json::from_str::<TestDeserialize>(&make_json(input)).unwrap_err();
        }
    }
}
