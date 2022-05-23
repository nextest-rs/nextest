// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration support for nextest.

use crate::{
    errors::{ConfigParseError, ProfileNotFound},
    reporter::{StatusLevel, TestOutputDisplay},
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{builder::DefaultState, Config, ConfigBuilder, File, FileFormat};
use serde::{de::IntoDeserializer, Deserialize};
use std::{collections::HashMap, fmt, time::Duration};

/// Overall configuration for nextest.
///
/// This is the root data structure for nextest configuration. Most runner-specific configuration is managed
/// through [profiles](NextestProfile), obtained through the [`profile`](Self::profile) method.
///
/// For more about configuration, see
/// [Configuration](https://nexte.st/book/configuration.html) in the nextest
/// book.
#[derive(Clone, Debug)]
pub struct NextestConfig {
    workspace_root: Utf8PathBuf,
    inner: NextestConfigImpl,
}

impl NextestConfig {
    /// The default location of the config within the path: `.config/nextest.toml`, used to read the
    /// config from the given directory.
    pub const CONFIG_PATH: &'static str = ".config/nextest.toml";

    /// Contains the default config as a TOML file.
    ///
    /// Repository-specific configuration is layered on top of the default config.
    pub const DEFAULT_CONFIG: &'static str = include_str!("../default-config.toml");

    /// Environment configuration uses this prefix, plus a _.
    pub const ENVIRONMENT_PREFIX: &'static str = "NEXTEST";

    /// The name of the default profile.
    pub const DEFAULT_PROFILE: &'static str = "default";

    /// Reads the nextest config from the given file, or if not specified from `.config/nextest.toml`
    /// in the given directory.
    ///
    /// If the file isn't specified and the directory doesn't have `.config/nextest.toml`, uses the
    /// default config options.
    pub fn from_sources(
        workspace_root: impl Into<Utf8PathBuf>,
        config_file: Option<&Utf8Path>,
    ) -> Result<Self, ConfigParseError> {
        let workspace_root = workspace_root.into();
        let (config_file, config) = Self::read_from_sources(&workspace_root, config_file)?;
        let inner = config
            .try_deserialize()
            .map_err(|err| ConfigParseError::new(config_file, err))?;
        Ok(Self {
            workspace_root,
            inner,
        })
    }

    /// Returns the default nextest config.
    pub fn default_config(workspace_root: impl Into<Utf8PathBuf>) -> Self {
        let config = Self::make_default_config()
            .build()
            .expect("default config is always valid");

        let inner = config
            .try_deserialize()
            .expect("default config is always valid");
        Self {
            workspace_root: workspace_root.into(),
            inner,
        }
    }

    /// Returns the profile with the given name, or an error if a profile was specified but not
    /// found.
    pub fn profile(&self, name: impl AsRef<str>) -> Result<NextestProfile<'_>, ProfileNotFound> {
        self.make_profile(name.as_ref())
    }

    // ---
    // Helper methods
    // ---

    fn read_from_sources(
        workspace_root: &Utf8Path,
        file: Option<&Utf8Path>,
    ) -> Result<(Utf8PathBuf, Config), ConfigParseError> {
        // First, get the default config.
        let builder = Self::make_default_config();

        // Next, merge in the config from the given file.
        let (builder, config_path) = match file {
            Some(file) => (
                builder.add_source(File::new(file.as_str(), FileFormat::Toml)),
                file.to_owned(),
            ),
            None => {
                let config_path = workspace_root.join(Self::CONFIG_PATH);
                (
                    builder.add_source(
                        File::new(config_path.as_str(), FileFormat::Toml).required(false),
                    ),
                    config_path,
                )
            }
        };

        let config = builder
            .build()
            .map_err(|err| ConfigParseError::new(&config_path, err))?;
        Ok((config_path, config))
    }

    fn make_default_config() -> ConfigBuilder<DefaultState> {
        Config::builder().add_source(File::from_str(Self::DEFAULT_CONFIG, FileFormat::Toml))
    }

    fn make_profile(&self, name: &str) -> Result<NextestProfile<'_>, ProfileNotFound> {
        let custom_profile = self.inner.profiles.get(name)?;

        // The profile was found: construct the NextestProfile.
        let mut store_dir = self.workspace_root.join(&self.inner.store.dir);
        store_dir.push(name);

        Ok(NextestProfile {
            store_dir,
            default_profile: &self.inner.profiles.default,
            custom_profile,
        })
    }
}

/// A configuration profile for nextest. Contains most configuration used by the nextest runner.
///
/// Returned by [`NextestConfig::profile`].
#[derive(Clone, Debug)]
pub struct NextestProfile<'cfg> {
    store_dir: Utf8PathBuf,
    default_profile: &'cfg DefaultProfileImpl,
    custom_profile: Option<&'cfg CustomProfileImpl>,
}

impl<'cfg> NextestProfile<'cfg> {
    /// Returns the absolute profile-specific store directory.
    pub fn store_dir(&self) -> &Utf8Path {
        &self.store_dir
    }

    /// Returns the retry count for this profile.
    pub fn retries(&self) -> usize {
        self.custom_profile
            .and_then(|profile| profile.retries)
            .unwrap_or(self.default_profile.retries)
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self) -> Duration {
        self.custom_profile
            .and_then(|profile| profile.slow_timeout.display_after)
            .unwrap_or(self.default_profile.slow_timeout.display_after)
    }

    /// Returns the test status level.
    pub fn status_level(&self) -> StatusLevel {
        self.custom_profile
            .and_then(|profile| profile.status_level)
            .unwrap_or(self.default_profile.status_level)
    }

    /// Returns the test status level at the end of the run.
    pub fn final_status_level(&self) -> StatusLevel {
        self.custom_profile
            .and_then(|profile| profile.final_status_level)
            .unwrap_or(self.default_profile.final_status_level)
    }

    /// Returns the failure output config for this profile.
    pub fn failure_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .and_then(|profile| profile.failure_output)
            .unwrap_or(self.default_profile.failure_output)
    }

    /// Returns the failure output config for this profile.
    pub fn success_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .and_then(|profile| profile.success_output)
            .unwrap_or(self.default_profile.success_output)
    }

    /// Returns the fail-fast config for this profile.
    pub fn fail_fast(&self) -> bool {
        self.custom_profile
            .and_then(|profile| profile.fail_fast)
            .unwrap_or(self.default_profile.fail_fast)
    }

    /// Returns the JUnit configuration for this profile.
    pub fn junit(&self) -> Option<NextestJunitConfig<'cfg>> {
        let path = self
            .custom_profile
            .map(|profile| &profile.junit.path)
            .unwrap_or(&self.default_profile.junit.path)
            .as_deref();

        path.map(|path| {
            let path = self.store_dir.join(path);
            let report_name = self
                .custom_profile
                .and_then(|profile| profile.junit.report_name.as_deref())
                .unwrap_or(&self.default_profile.junit.report_name);
            NextestJunitConfig { path, report_name }
        })
    }
}

/// JUnit configuration for nextest, returned by a [`NextestProfile`].
#[derive(Clone, Debug)]
pub struct NextestJunitConfig<'cfg> {
    path: Utf8PathBuf,
    report_name: &'cfg str,
}

impl<'cfg> NextestJunitConfig<'cfg> {
    /// Returns the absolute path to the JUnit report.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Returns the name of the JUnit report.
    pub fn report_name(&self) -> &'cfg str {
        self.report_name
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestConfigImpl {
    store: StoreConfigImpl,
    #[serde(rename = "profile")]
    profiles: NextestProfilesImpl,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct StoreConfigImpl {
    dir: Utf8PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestProfilesImpl {
    default: DefaultProfileImpl,
    #[serde(flatten)]
    other: HashMap<String, CustomProfileImpl>,
}

impl NextestProfilesImpl {
    fn get(&self, profile: &str) -> Result<Option<&CustomProfileImpl>, ProfileNotFound> {
        let custom_profile = match profile {
            NextestConfig::DEFAULT_PROFILE => None,
            other => Some(
                self.other
                    .get(other)
                    .ok_or_else(|| ProfileNotFound::new(profile, self.all_profiles()))?,
            ),
        };
        Ok(custom_profile)
    }

    fn all_profiles(&self) -> impl Iterator<Item = &str> {
        self.other
            .keys()
            .map(|key| key.as_str())
            .chain(std::iter::once(NextestConfig::DEFAULT_PROFILE))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DefaultProfileImpl {
    retries: usize,
    status_level: StatusLevel,
    final_status_level: StatusLevel,
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    fail_fast: bool,
    #[serde(deserialize_with = "deserialize_default_slow_timeout")]
    slow_timeout: DefaultSlowTimeout,
    junit: DefaultJunitImpl,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DefaultSlowTimeout {
    #[serde(with = "humantime_serde")]
    display_after: Duration,
}

fn deserialize_default_slow_timeout<'de, D>(deserializer: D) -> Result<DefaultSlowTimeout, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = DefaultSlowTimeout;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table {{ display-after = \"60s\" }} or a string (\"60s\")"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let display_after = humantime_serde::deserialize(v.into_deserializer())?;
            Ok(DefaultSlowTimeout { display_after })
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            DefaultSlowTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(V)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DefaultJunitImpl {
    #[serde(default)]
    path: Option<Utf8PathBuf>,
    report_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CustomProfileImpl {
    #[serde(default)]
    retries: Option<usize>,
    #[serde(default)]
    status_level: Option<StatusLevel>,
    #[serde(default)]
    final_status_level: Option<StatusLevel>,
    #[serde(default)]
    failure_output: Option<TestOutputDisplay>,
    #[serde(default)]
    success_output: Option<TestOutputDisplay>,
    #[serde(default)]
    fail_fast: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_custom_slow_timeout")]
    slow_timeout: CustomSlowTimeout,
    #[serde(default)]
    junit: JunitImpl,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CustomSlowTimeout {
    #[serde(with = "humantime_serde::option")]
    display_after: Option<Duration>,
}

fn deserialize_custom_slow_timeout<'de, D>(deserializer: D) -> Result<CustomSlowTimeout, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = CustomSlowTimeout;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table {{ display-after = \"60s\" }} or a string (\"60s\")"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let display_after = Some(humantime_serde::deserialize(v.into_deserializer())?);
            Ok(CustomSlowTimeout { display_after })
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            CustomSlowTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(V)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JunitImpl {
    #[serde(default)]
    path: Option<Utf8PathBuf>,
    report_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let default_config = NextestConfig::default_config("foo");
        default_config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile should exist");
    }
}
