// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{CargoConfigSource, CargoConfigs, DiscoveredConfig};
use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    ffi::OsString,
    process::Command,
};

/// Environment variables to set when running tests.
#[derive(Clone, Debug)]
pub struct EnvironmentMap {
    map: BTreeMap<imp::EnvKey, CargoEnvironmentVariable>,
}

impl EnvironmentMap {
    /// Creates a new `EnvironmentMap` from the given Cargo configs.
    pub fn new(configs: &CargoConfigs) -> Self {
        let env_configs = configs
            .discovered_configs()
            .filter_map(|config| match config {
                DiscoveredConfig::CliOption { config, source }
                | DiscoveredConfig::File { config, source } => Some((config, source)),
                DiscoveredConfig::Env => None,
            })
            .flat_map(|(config, source)| {
                let source = match source {
                    CargoConfigSource::CliOption => None,
                    CargoConfigSource::File(path) => Some(path.clone()),
                };
                config
                    .env
                    .clone()
                    .into_iter()
                    .map(move |(name, value)| (source.clone(), name, value))
            });

        let mut map = BTreeMap::<imp::EnvKey, CargoEnvironmentVariable>::new();

        for (source, name, value) in env_configs {
            match map.entry(imp::EnvKey::from(name.clone())) {
                Entry::Occupied(mut entry) => {
                    // Ignore the value lower in precedence, but do look at force and relative if
                    // they haven't been set already.
                    let var = entry.get_mut();
                    if var.force.is_none() && value.force().is_some() {
                        var.force = value.force();
                    }
                    if var.relative.is_none() && value.relative().is_some() {
                        var.relative = value.relative();
                    }
                }
                Entry::Vacant(entry) => {
                    let force = value.force();
                    let relative = value.relative();
                    let value = value.into_value();
                    entry.insert(CargoEnvironmentVariable {
                        source,
                        name,
                        value,
                        force,
                        relative,
                    });
                }
            }
        }

        Self { map }
    }

    /// Creates an empty `EnvironmentMap`.
    ///
    /// Used for replay and testing where actual environment variables
    /// are not needed.
    pub fn empty() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub(crate) fn apply_env(&self, command: &mut Command) {
        #[cfg_attr(not(windows), expect(clippy::useless_conversion))]
        let existing_keys: BTreeSet<imp::EnvKey> =
            std::env::vars_os().map(|(k, _v)| k.into()).collect();

        for (name, var) in &self.map {
            let should_set_value = if existing_keys.contains(name) {
                var.force.unwrap_or_default()
            } else {
                true
            };
            if !should_set_value {
                continue;
            }

            let value = if var.relative.unwrap_or_default() {
                let base_path = match &var.source {
                    Some(source_path) => source_path,
                    None => unreachable!(
                        "Cannot use a relative path for environment variable {name:?} \
                        whose source is not a config file (this should already have been checked)"
                    ),
                };
                relative_dir_for(base_path).map_or_else(
                    || var.value.clone(),
                    |rel_dir| rel_dir.join(&var.value).into_string(),
                )
            } else {
                var.value.clone()
            };

            command.env(name, value);
        }
    }
}

/// An environment variable set in `config.toml`. See
/// <https://doc.rust-lang.org/cargo/reference/config.html#env>.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoEnvironmentVariable {
    /// The source `config.toml` file. See
    /// <https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure> for the
    /// lookup order.
    pub source: Option<Utf8PathBuf>,

    /// The name of the environment variable to set.
    pub name: String,

    /// The value of the environment variable to set.
    pub value: String,

    /// If the environment variable is already set in the environment, it is not reassigned unless
    /// `force` is set to `true`.
    ///
    /// Note: None means false.
    pub force: Option<bool>,

    /// Interpret the environment variable as a path relative to the directory containing the source
    /// `config.toml` file.
    ///
    /// Note: None means false.
    pub relative: Option<bool>,
}

/// Returns the directory against which relative paths are computed for the given config path.
pub fn relative_dir_for(config_path: &Utf8Path) -> Option<&Utf8Path> {
    // Need to call parent() twice here, since in Cargo land relative means relative to the *parent*
    // of the directory the config is in. First parent() gets the directory the config is in, and
    // the second one gets the parent of that.
    let relative_dir = config_path.parent()?.parent()?;

    // On Windows, remove the UNC prefix since Cargo does so as well.
    Some(imp::strip_unc_prefix(relative_dir))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{borrow::Borrow, cmp, ffi::OsStr, os::windows::prelude::OsStrExt};
    use windows_sys::Win32::Globalization::{
        CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal,
    };

    pub(super) fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
        dunce::simplified(path.as_std_path())
            .try_into()
            .expect("stripping verbatim components from a UTF-8 path should result in a UTF-8 path")
    }

    // The definition of EnvKey is borrowed from
    // https://github.com/rust-lang/rust/blob/a24a020e6d926dffe6b472fc647978f92269504e/library/std/src/sys/windows/process.rs.

    #[derive(Clone, Debug, Eq)]
    #[doc(hidden)]
    pub(super) struct EnvKey {
        os_string: OsString,
        // This stores a UTF-16 encoded string to workaround the mismatch between
        // Rust's OsString (WTF-8) and the Windows API string type (UTF-16).
        // Normally converting on every API call is acceptable but here
        // `c::CompareStringOrdinal` will be called for every use of `==`.
        utf16: Vec<u16>,
    }

    // Comparing Windows environment variable keys[1] are behaviourally the
    // composition of two operations[2]:
    //
    // 1. Case-fold both strings. This is done using a language-independent
    // uppercase mapping that's unique to Windows (albeit based on data from an
    // older Unicode spec). It only operates on individual UTF-16 code units so
    // surrogates are left unchanged. This uppercase mapping can potentially change
    // between Windows versions.
    //
    // 2. Perform an ordinal comparison of the strings. A comparison using ordinal
    // is just a comparison based on the numerical value of each UTF-16 code unit[3].
    //
    // Because the case-folding mapping is unique to Windows and not guaranteed to
    // be stable, we ask the OS to compare the strings for us. This is done by
    // calling `CompareStringOrdinal`[4] with `bIgnoreCase` set to `TRUE`.
    //
    // [1] https://docs.microsoft.com/en-us/dotnet/standard/base-types/best-practices-strings#choosing-a-stringcomparison-member-for-your-method-call
    // [2] https://docs.microsoft.com/en-us/dotnet/standard/base-types/best-practices-strings#stringtoupper-and-stringtolower
    // [3] https://docs.microsoft.com/en-us/dotnet/api/system.stringcomparison?view=net-5.0#System_StringComparison_Ordinal
    // [4] https://docs.microsoft.com/en-us/windows/win32/api/stringapiset/nf-stringapiset-comparestringordinal
    impl Ord for EnvKey {
        fn cmp(&self, other: &Self) -> cmp::Ordering {
            unsafe {
                let result = CompareStringOrdinal(
                    self.utf16.as_ptr(),
                    self.utf16.len() as _,
                    other.utf16.as_ptr(),
                    other.utf16.len() as _,
                    1, /* ignore case */
                );
                match result {
                    CSTR_LESS_THAN => cmp::Ordering::Less,
                    CSTR_EQUAL => cmp::Ordering::Equal,
                    CSTR_GREATER_THAN => cmp::Ordering::Greater,
                    // `CompareStringOrdinal` should never fail so long as the parameters are correct.
                    _ => panic!(
                        "comparing environment keys failed: {}",
                        std::io::Error::last_os_error()
                    ),
                }
            }
        }
    }
    impl PartialOrd for EnvKey {
        fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl PartialEq for EnvKey {
        fn eq(&self, other: &Self) -> bool {
            if self.utf16.len() != other.utf16.len() {
                false
            } else {
                self.cmp(other) == cmp::Ordering::Equal
            }
        }
    }

    // Environment variable keys should preserve their original case even though
    // they are compared using a caseless string mapping.
    impl From<OsString> for EnvKey {
        fn from(k: OsString) -> Self {
            EnvKey {
                utf16: k.encode_wide().collect(),
                os_string: k,
            }
        }
    }

    impl From<String> for EnvKey {
        fn from(k: String) -> Self {
            OsString::from(k).into()
        }
    }

    impl From<EnvKey> for OsString {
        fn from(k: EnvKey) -> Self {
            k.os_string
        }
    }

    impl Borrow<OsStr> for EnvKey {
        fn borrow(&self) -> &OsStr {
            &self.os_string
        }
    }

    impl AsRef<OsStr> for EnvKey {
        fn as_ref(&self) -> &OsStr {
            &self.os_string
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub(super) fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
        path
    }

    pub(super) type EnvKey = OsString;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_config::test_helpers::setup_temp_dir;
    use std::ffi::OsStr;

    #[test]
    fn test_env_var_precedence() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        let configs = CargoConfigs::new_with_isolation(
            &[] as &[&str],
            &dir_foo_bar_path,
            &dir_path,
            Vec::new(),
        )
        .unwrap();
        let env = EnvironmentMap::new(&configs);
        let var = env
            .map
            .get(OsStr::new("SOME_VAR"))
            .expect("SOME_VAR is specified in test config");
        assert_eq!(var.value, "foo-bar-config");

        let configs = CargoConfigs::new_with_isolation(
            ["env.SOME_VAR=\"cli-config\""],
            &dir_foo_bar_path,
            &dir_path,
            Vec::new(),
        )
        .unwrap();
        let env = EnvironmentMap::new(&configs);
        let var = env
            .map
            .get(OsStr::new("SOME_VAR"))
            .expect("SOME_VAR is specified in test config");
        assert_eq!(var.value, "cli-config");
    }

    #[test]
    fn test_cli_env_var_relative() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        CargoConfigs::new_with_isolation(
            ["env.SOME_VAR={value = \"path\", relative = true }"],
            &dir_foo_bar_path,
            &dir_path,
            Vec::new(),
        )
        .expect_err("CLI configs can't be relative");

        CargoConfigs::new_with_isolation(
            ["env.SOME_VAR.value=\"path\"", "env.SOME_VAR.relative=true"],
            &dir_foo_bar_path,
            &dir_path,
            Vec::new(),
        )
        .expect_err("CLI configs can't be relative");
    }

    #[test]
    #[cfg(unix)]
    fn test_relative_dir_for_unix() {
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config.toml".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/config".as_ref()),
            Some("/foo".as_ref())
        );
        assert_eq!(relative_dir_for("/foo/config".as_ref()), Some("/".as_ref()));
        assert_eq!(relative_dir_for("/config.toml".as_ref()), None);
    }

    #[test]
    #[cfg(windows)]
    fn test_relative_dir_for_windows() {
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config.toml".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\config".as_ref()),
            Some("C:\\foo".as_ref())
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\config".as_ref()),
            Some("C:\\".as_ref())
        );
        assert_eq!(relative_dir_for("C:\\config.toml".as_ref()), None);
    }
}
