// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::{Utf8Component, Utf8PathBuf};
use serde::{
    Deserialize,
    de::{Error, Unexpected},
};

/// An error that occurred while validating an identifier.
#[derive(Clone, Copy, Debug)]
pub(in crate::config) enum InvalidIdentifierKind {
    /// The identifier is empty.
    Empty,
    /// The identifier has invalid XID characters.
    InvalidXid,
}

/// Validates that an identifier follows Unicode XID rules with hyphens allowed.
///
/// Identifiers must be of the form `(XID_Start)(XID_Continue | -)*`.
pub(in crate::config) fn is_valid_identifier_unicode(s: &str) -> Result<(), InvalidIdentifierKind> {
    if s.is_empty() {
        return Err(InvalidIdentifierKind::Empty);
    }

    let mut first = true;
    for ch in s.chars() {
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

/// Deserializes a well-formed relative path.
///
/// Returns an error on absolute paths, and on other kinds of relative paths.
pub(in crate::config) fn deserialize_relative_path<'de, D>(
    deserializer: D,
) -> Result<Utf8PathBuf, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = Utf8PathBuf::deserialize(deserializer)?;
    for component in s.components() {
        match component {
            Utf8Component::Normal(_) | Utf8Component::CurDir => {}
            Utf8Component::RootDir | Utf8Component::Prefix(_) | Utf8Component::ParentDir => {
                return Err(D::Error::invalid_value(
                    Unexpected::Str(s.as_str()),
                    &"a relative path with no parent components",
                ));
            }
        }
    }

    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use color_eyre::eyre::{Context, Result, bail};
    use serde::de::IntoDeserializer;

    #[test]
    fn test_deserialize_relative_path() -> Result<()> {
        let valid = &["foo", "foo/bar", "foo/./bar", "./foo/bar", "."];

        let invalid = &[
            "/foo/bar",
            "foo/../bar",
            "../foo/bar",
            #[cfg(windows)]
            "C:\\foo\\bar",
            #[cfg(windows)]
            "C:foo",
            #[cfg(windows)]
            "\\\\?\\C:\\foo\\bar",
        ];

        for &input in valid {
            let path = de_relative_path(input.into_deserializer())
                .wrap_err_with(|| format!("error deserializing valid path {input:?}: error"))?;
            assert_eq!(path, Utf8PathBuf::from(input), "path matches: {path:?}");
        }

        for &input in invalid {
            let error = match de_relative_path(input.into_deserializer()) {
                Ok(path) => bail!("successfully deserialized an invalid path: {:?}", path),
                Err(error) => error,
            };
            assert_eq!(
                error.to_string(),
                format!(
                    "invalid value: string {input:?}, expected a relative path with no parent components"
                )
            );
        }

        Ok(())
    }

    // Required for type inference.
    fn de_relative_path<'de, D>(deserializer: D) -> Result<Utf8PathBuf, D::Error>
    where
        D: serde::Deserializer<'de, Error = serde::de::value::Error>,
    {
        deserialize_relative_path(deserializer)
    }
}
