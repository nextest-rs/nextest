// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::EnvVarError;
use serde::{Deserialize, de::Error};
use std::{collections::BTreeMap, fmt, process::Command};

/// A map of environment variables associated with a [`super::ScriptCommand`].
///
/// Map keys are validated at construction time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptCommandEnvMap(BTreeMap<String, String>);

impl ScriptCommandEnvMap {
    /// Returns the value for the given key, if present.
    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Applies all environment variables to the command.
    ///
    /// All keys are validated at construction time (via [`Deserialize`] or
    /// [`ScriptCommandEnvMap::new`]), so this method is infallible.
    pub(crate) fn apply_env(&self, cmd: &mut Command) {
        for (key, value) in &self.0 {
            cmd.env(key, value);
        }
    }

    /// Creates a new `ScriptCommandEnvMap` from a `BTreeMap`, validating that
    /// all keys are valid environment variable names.
    #[cfg(test)]
    pub(crate) fn new(
        value: BTreeMap<String, String>,
    ) -> Result<Self, crate::errors::ErrorList<crate::errors::EnvVarError>> {
        use crate::errors::ErrorList;

        let mut errors = Vec::new();
        for key in value.keys() {
            if let Err(err) = validate_env_var_key(key) {
                errors.push(err);
            }
        }
        if let Some(err) = ErrorList::new("unsupported environment variables", errors) {
            return Err(err);
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for ScriptCommandEnvMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EnvMapVisitor;

        impl<'de> serde::de::Visitor<'de> for EnvMapVisitor {
            type Value = ScriptCommandEnvMap;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of environment variable names to values")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut env = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if let Err(err) = validate_env_var_key(&key) {
                        return Err(A::Error::invalid_value(
                            serde::de::Unexpected::Str(&key),
                            &err,
                        ));
                    }
                    env.insert(key, value);
                }
                Ok(ScriptCommandEnvMap(env))
            }
        }

        deserializer.deserialize_map(EnvMapVisitor)
    }
}

// Validates against the most conservative definition of a valid environment
// variable key. The definition of "Name" is taken from POSIX.1-2024 [1]:
//
// > In the shell command language, a word consisting solely of underscores,
// > digits, and alphabetics from the portable character set. The first
// > character of a name is not a digit.
//
// This is more conservative than strictly necessary: chapter 8 of
// POSIX.1-2024 [2] and Microsoft's documentation [3] only prohibit '=' in
// keys, and POSIX notes that implementations may permit other characters.
// However, restricting to the portable character set is not wrong and avoids
// cross-platform surprises (e.g. shells that reject non-POSIX names, or NUL
// bytes that are not portable).
//
// [1]: https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/V1_chap03.html#tag_03_216
// [2]: https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap08.html
// [3]: https://learn.microsoft.com/en-us/windows/win32/procthread/environment-variables

/// Validates a str to see if it is suitable to be a key of an environment
/// variable.
pub(crate) fn validate_env_var_key(key: &str) -> Result<(), EnvVarError> {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => {
            return Err(EnvVarError::InvalidKeyStartChar {
                key: key.to_owned(),
            });
        }
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(EnvVarError::InvalidKey {
            key: key.to_owned(),
        });
    }
    if key.starts_with("NEXTEST") {
        return Err(EnvVarError::ReservedKey {
            key: key.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn apply_env() {
        let mut cmd = std::process::Command::new("demo");
        cmd.env_clear();
        let env = ScriptCommandEnvMap::new(BTreeMap::from([
            (String::from("KEY_A"), String::from("value_a")),
            (String::from("KEY_B"), String::from("value_b")),
        ]))
        .expect("valid env var keys");

        env.apply_env(&mut cmd);
        let applied: BTreeMap<_, _> = cmd.get_envs().collect();
        assert_eq!(applied.len(), 2, "all env vars applied");
    }

    #[test]
    fn new_rejects_invalid_key() {
        let err =
            ScriptCommandEnvMap::new(BTreeMap::from([(String::from("INVALID "), String::new())]))
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "key `INVALID ` does not consist solely of letters, digits, and underscores",
        );
    }

    #[test]
    fn validate_env_var_key_valid() {
        validate_env_var_key("MY_ENV_VAR").unwrap();
        validate_env_var_key("MY_ENV_VAR_1").unwrap();
        validate_env_var_key("__NEXTEST_TEST").unwrap();
    }

    #[test]
    fn validate_env_var_key_invalid() {
        let cases = [
            ("", "key `` does not start with a letter or underscore"),
            (" ", "key ` ` does not start with a letter or underscore"),
            ("=", "key `=` does not start with a letter or underscore"),
            ("0", "key `0` does not start with a letter or underscore"),
            (
                "0TEST ",
                "key `0TEST ` does not start with a letter or underscore",
            ),
            (
                "=TEST=",
                "key `=TEST=` does not start with a letter or underscore",
            ),
            (
                "TEST TEST",
                "key `TEST TEST` does not consist solely of letters, digits, and underscores",
            ),
            (
                "TESTTEST\n",
                "key `TESTTEST\n` does not consist solely of letters, digits, and underscores",
            ),
            (
                "TEST=TEST",
                "key `TEST=TEST` does not consist solely of letters, digits, and underscores",
            ),
            (
                "TEST=",
                "key `TEST=` does not consist solely of letters, digits, and underscores",
            ),
            (
                "NEXTEST",
                "key `NEXTEST` begins with `NEXTEST`, which is reserved for internal use",
            ),
            (
                "NEXTEST_NEXTEST",
                "key `NEXTEST_NEXTEST` begins with `NEXTEST`, which is reserved for internal use",
            ),
        ];

        for (key, message) in cases {
            let err = validate_env_var_key(key).unwrap_err();
            let actual_message = err.to_string();

            assert_eq!(
                actual_message, *message,
                "key validation error message equals expected"
            );
        }
    }
}
