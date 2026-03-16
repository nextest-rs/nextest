// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::runner::script_helpers::validate_env_var_key;
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
                formatter.write_str("a map")
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
}
