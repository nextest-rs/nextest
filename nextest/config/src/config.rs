// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::errors::{
    ConfigReadError, ProfileNotFound, StatusLevelParseError, TestOutputDisplayParseError,
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{Config, Environment, File, FileFormat};
use serde::Deserialize;
use std::{collections::HashMap, fmt, marker::PhantomData, str::FromStr, time::Duration};

/// Configuration for nextest.
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
    /// The default rules included with this copy of nextest-runner are:
    ///
    /// ```toml
    #[doc = include_str ! ("../default-config.toml")]
    /// ```
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
    ) -> Result<Self, ConfigReadError> {
        let workspace_root = workspace_root.into();
        let config = Self::read_from_sources(&workspace_root, config_file)?;
        let inner = config.try_into().map_err(ConfigReadError::new)?;
        Ok(Self {
            workspace_root,
            inner,
        })
    }

    /// Returns the default nextest config.
    pub fn default_config(workspace_root: impl Into<Utf8PathBuf>) -> Self {
        let config = Self::make_default_config();
        let inner = config.try_into().expect("default config is always valid");
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
    ) -> Result<Config, ConfigReadError> {
        // First, get the default config.
        let mut config = Self::make_default_config();

        // Next, merge in the config from the given file.
        match file {
            Some(file) => {
                config
                    .merge(File::new(file.as_str(), FileFormat::Toml))
                    .map_err(ConfigReadError::new)?;
            }
            None => {
                let config_path = workspace_root.join(Self::CONFIG_PATH);
                config
                    .merge(File::new(config_path.as_str(), FileFormat::Toml).required(false))
                    .map_err(ConfigReadError::new)?;
            }
        }

        // Finally, read in the environment variables.
        config
            .merge(Environment::with_prefix(Self::ENVIRONMENT_PREFIX).separator("_"))
            .map_err(ConfigReadError::new)?;

        Ok(config)
    }

    fn make_default_config() -> Config {
        Config::new()
            .with_merged(File::from_str(Self::DEFAULT_CONFIG, FileFormat::Toml))
            .expect("default config is valid")
    }

    fn make_profile(&self, name: &str) -> Result<NextestProfile<'_>, ProfileNotFound> {
        let custom_profile = self.inner.profiles.get(name)?;

        // The profile was found: construct the NextestProfile.
        let mut metadata_dir = self.workspace_root.join(&self.inner.metadata.dir);
        metadata_dir.push(name);

        let metadata_name = format!("{}{}", self.inner.metadata.prefix.as_str(), name);

        Ok(NextestProfile {
            metadata_dir,
            metadata_name,
            default_profile: &self.inner.profiles.default,
            custom_profile,
        })
    }
}

/// A nextest profile. Contains configuration for a specific nextest run.
#[derive(Clone, Debug)]
pub struct NextestProfile<'cfg> {
    metadata_dir: Utf8PathBuf,
    metadata_name: String,
    default_profile: &'cfg DefaultProfileImpl,
    custom_profile: Option<&'cfg CustomProfileImpl>,
}

impl<'cfg> NextestProfile<'cfg> {
    /// Returns the absolute profile-specific metadata directory.
    pub fn metadata_dir(&self) -> &Utf8Path {
        &self.metadata_dir
    }

    /// Returns the test run name used in the metadata.
    pub fn metadata_name(&self) -> &str {
        &self.metadata_name
    }

    /// Returns the retry count for this profile.
    pub fn retries(&self) -> usize {
        self.custom_profile
            .map(|profile| profile.retries)
            .flatten()
            .unwrap_or(self.default_profile.retries)
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self) -> Duration {
        self.custom_profile
            .map(|profile| profile.slow_timeout)
            .flatten()
            .unwrap_or(self.default_profile.slow_timeout)
    }

    /// Returns the test status level.
    pub fn status_level(&self) -> StatusLevel {
        self.custom_profile
            .map(|profile| profile.status_level)
            .flatten()
            .unwrap_or(self.default_profile.status_level)
    }

    /// Returns the failure output config for this profile.
    pub fn failure_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .map(|profile| profile.failure_output)
            .flatten()
            .unwrap_or(self.default_profile.failure_output)
    }

    /// Returns the failure output config for this profile.
    pub fn success_output(&self) -> TestOutputDisplay {
        self.custom_profile
            .map(|profile| profile.success_output)
            .flatten()
            .unwrap_or(self.default_profile.success_output)
    }

    /// Returns the fail-fast config for this profile.
    pub fn fail_fast(&self) -> bool {
        self.custom_profile
            .map(|profile| profile.fail_fast)
            .flatten()
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
            let path = self.metadata_dir.join(path);
            NextestJunitConfig {
                path,
                phantom: PhantomData,
            }
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestOutputDisplay {
    Immediate,
    ImmediateFinal,
    Final,
    Never,
}

impl TestOutputDisplay {
    pub fn variants() -> &'static [&'static str] {
        &["immediate", "immediate-final", "final", "never"]
    }

    pub fn is_immediate(self) -> bool {
        match self {
            TestOutputDisplay::Immediate | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Final | TestOutputDisplay::Never => false,
        }
    }

    pub fn is_final(self) -> bool {
        match self {
            TestOutputDisplay::Final | TestOutputDisplay::ImmediateFinal => true,
            TestOutputDisplay::Immediate | TestOutputDisplay::Never => false,
        }
    }
}

impl FromStr for TestOutputDisplay {
    type Err = TestOutputDisplayParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "immediate" => TestOutputDisplay::Immediate,
            "immediate-final" => TestOutputDisplay::ImmediateFinal,
            "final" => TestOutputDisplay::Final,
            "never" => TestOutputDisplay::Never,
            other => return Err(TestOutputDisplayParseError::new(other)),
        };
        Ok(val)
    }
}

impl fmt::Display for TestOutputDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestOutputDisplay::Immediate => write!(f, "immediate"),
            TestOutputDisplay::ImmediateFinal => write!(f, "immediate-final"),
            TestOutputDisplay::Final => write!(f, "final"),
            TestOutputDisplay::Never => write!(f, "never"),
        }
    }
}

/// Status level to show in the output.
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum StatusLevel {
    None,
    Fail,
    Retry,
    Slow,
    Pass,
    Skip,
    All,
}

impl StatusLevel {
    pub fn variants() -> &'static [&'static str] {
        &["none", "fail", "retry", "slow", "pass", "skip", "all"]
    }
}

impl FromStr for StatusLevel {
    type Err = StatusLevelParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let val = match s {
            "none" => StatusLevel::None,
            "fail" => StatusLevel::Fail,
            "retry" => StatusLevel::Retry,
            "slow" => StatusLevel::Slow,
            "pass" => StatusLevel::Pass,
            "skip" => StatusLevel::Skip,
            "all" => StatusLevel::All,
            other => return Err(StatusLevelParseError::new(other)),
        };
        Ok(val)
    }
}

impl fmt::Display for StatusLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusLevel::None => write!(f, "none"),
            StatusLevel::Fail => write!(f, "fail"),
            StatusLevel::Retry => write!(f, "retry"),
            StatusLevel::Slow => write!(f, "slow"),
            StatusLevel::Pass => write!(f, "pass"),
            StatusLevel::Skip => write!(f, "skip"),
            StatusLevel::All => write!(f, "all"),
        }
    }
}

/// JUnit configuration for nextest.
#[derive(Clone, Debug)]
pub struct NextestJunitConfig<'cfg> {
    path: Utf8PathBuf,
    // Possibly will refer to other config fields in the future.
    phantom: PhantomData<&'cfg ()>,
}

impl<'cfg> NextestJunitConfig<'cfg> {
    /// Returns the absolute path to the metadata.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct NextestConfigImpl {
    metadata: MetadataConfigImpl,
    #[serde(rename = "profile")]
    profiles: NextestProfilesImpl,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MetadataConfigImpl {
    dir: Utf8PathBuf,
    prefix: String,
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
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    fail_fast: bool,
    #[serde(with = "humantime_serde")]
    slow_timeout: Duration,
    junit: JunitImpl,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CustomProfileImpl {
    #[serde(default)]
    retries: Option<usize>,
    #[serde(default)]
    status_level: Option<StatusLevel>,
    #[serde(default)]
    failure_output: Option<TestOutputDisplay>,
    #[serde(default)]
    success_output: Option<TestOutputDisplay>,
    #[serde(default)]
    fail_fast: Option<bool>,
    #[serde(with = "humantime_serde")]
    #[serde(default)]
    slow_timeout: Option<Duration>,
    #[serde(default)]
    junit: JunitImpl,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct JunitImpl {
    #[serde(default)]
    path: Option<Utf8PathBuf>,
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
