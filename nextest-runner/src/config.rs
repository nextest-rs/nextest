// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration support for nextest.

use crate::{
    errors::{
        ConfigParseError, ConfigParseErrorKind, ConfigParseOverrideError, ProfileNotFound,
        TestThreadsParseError, ToolConfigFileParseError,
    },
    reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
};
use camino::{Utf8Path, Utf8PathBuf};
use config::{builder::DefaultState, Config, ConfigBuilder, File, FileFormat, FileSourceFile};
use guppy::graph::PackageGraph;
use nextest_filtering::{FilteringExpr, TestQuery};
use serde::{de::IntoDeserializer, Deserialize};
use std::{collections::HashMap, fmt, num::NonZeroUsize, str::FromStr, time::Duration};

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
    overrides: NextestOverridesImpl,
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

    /// The name of the default profile used for miri.
    pub const DEFAULT_MIRI_PROFILE: &'static str = "default-miri";

    /// Reads the nextest config from the given file, or if not specified from `.config/nextest.toml`
    /// in the workspace root.
    ///
    /// `tool_config_files` are lower priority than `config_file` but higher priority than the
    /// default config. Files in `tool_config_files` that come earlier are higher priority than those
    /// that come later.
    ///
    /// If no config files are specified and this file doesn't have `.config/nextest.toml`, uses the
    /// default config options.
    pub fn from_sources<'a, I>(
        workspace_root: impl Into<Utf8PathBuf>,
        graph: &PackageGraph,
        config_file: Option<&Utf8Path>,
        tool_config_files: impl IntoIterator<IntoIter = I>,
    ) -> Result<Self, ConfigParseError>
    where
        I: Iterator<Item = &'a ToolConfigFile> + DoubleEndedIterator,
    {
        let workspace_root = workspace_root.into();
        let tool_config_files_rev = tool_config_files.into_iter().rev();
        let (inner, overrides) =
            Self::read_from_sources(graph, &workspace_root, config_file, tool_config_files_rev)?;
        Ok(Self {
            workspace_root,
            inner,
            overrides,
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
            // The default config does not (cannot) have overrides.
            overrides: NextestOverridesImpl::default(),
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

    fn read_from_sources<'a>(
        graph: &PackageGraph,
        workspace_root: &Utf8Path,
        file: Option<&Utf8Path>,
        tool_config_files_rev: impl Iterator<Item = &'a ToolConfigFile>,
    ) -> Result<(NextestConfigImpl, NextestOverridesImpl), ConfigParseError> {
        // First, get the default config.
        let mut composite_builder = Self::make_default_config();

        // Overrides are handled additively.
        // Note that they're stored in reverse order here, and are flipped over at the end.
        let mut overrides_impl = NextestOverridesImpl::default();

        // Next, merge in tool configs.
        for ToolConfigFile { config_file, tool } in tool_config_files_rev {
            let source = File::new(config_file.as_str(), FileFormat::Toml);
            Self::deserialize_individual_config(
                graph,
                config_file,
                Some(tool),
                source.clone(),
                &mut overrides_impl,
            )?;

            // This is the final, composite builder used at the end.
            composite_builder = composite_builder.add_source(source);
        }

        // Next, merge in the config from the given file.
        let (config_file, source) = match file {
            Some(file) => (file.to_owned(), File::new(file.as_str(), FileFormat::Toml)),
            None => {
                let config_file = workspace_root.join(Self::CONFIG_PATH);
                let source = File::new(config_file.as_str(), FileFormat::Toml).required(false);
                (config_file, source)
            }
        };

        Self::deserialize_individual_config(
            graph,
            &config_file,
            None,
            source.clone(),
            &mut overrides_impl,
        )?;

        composite_builder = composite_builder.add_source(source);

        let config = Self::build_and_deserialize_config(&composite_builder)
            .map_err(|kind| ConfigParseError::new(config_file, None, kind))?;

        // Reverse all the overrides at the end.
        overrides_impl.default.reverse();
        for override_ in overrides_impl.other.values_mut() {
            override_.reverse();
        }

        Ok((config, overrides_impl))
    }

    fn deserialize_individual_config(
        graph: &PackageGraph,
        config_file: &Utf8Path,
        tool: Option<&str>,
        source: File<FileSourceFile, FileFormat>,
        overrides_impl: &mut NextestOverridesImpl,
    ) -> Result<(), ConfigParseError> {
        // Try building default builder + this file to get good error attribution and handle
        // overrides additively.
        let default_builder = Self::make_default_config();
        let this_builder = default_builder.add_source(source);
        let this_config = Self::build_and_deserialize_config(&this_builder)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;
        // Compile the overrides for this file.
        let this_overrides = NextestOverridesImpl::new(graph, &this_config)
            .map_err(|kind| ConfigParseError::new(config_file, tool, kind))?;

        // Grab the overrides for this config. Add them in reversed order (we'll flip it around at the end).
        overrides_impl
            .default
            .extend(this_overrides.default.into_iter().rev());
        for (name, overrides) in this_overrides.other {
            overrides_impl
                .other
                .entry(name)
                .or_default()
                .extend(overrides.into_iter().rev());
        }

        Ok(())
    }

    fn make_default_config() -> ConfigBuilder<DefaultState> {
        Config::builder().add_source(File::from_str(Self::DEFAULT_CONFIG, FileFormat::Toml))
    }

    fn make_profile(&self, name: &str) -> Result<NextestProfile<'_>, ProfileNotFound> {
        let custom_profile = self.inner.profiles.get(name)?;

        // The profile was found: construct the NextestProfile.
        let mut store_dir = self.workspace_root.join(&self.inner.store.dir);
        store_dir.push(name);

        // Grab the overrides as well.
        let overrides = self
            .overrides
            .other
            .get(name)
            .into_iter()
            .flatten()
            .chain(self.overrides.default.iter())
            .collect();

        Ok(NextestProfile {
            store_dir,
            default_profile: &self.inner.profiles.default,
            custom_profile,
            overrides,
        })
    }

    fn build_and_deserialize_config(
        builder: &ConfigBuilder<DefaultState>,
    ) -> Result<NextestConfigImpl, ConfigParseErrorKind> {
        let config = builder
            .build_cloned()
            .map_err(ConfigParseErrorKind::BuildError)?;

        serde_path_to_error::deserialize(config).map_err(ConfigParseErrorKind::DeserializeError)
    }
}

/// A tool-specific config file.
///
/// Tool-specific config files are lower priority than repository configs, but higher priority than
/// the default config shipped with nextest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolConfigFile {
    /// The name of the tool.
    pub tool: String,

    /// The path to the config file.
    pub config_file: Utf8PathBuf,
}

impl FromStr for ToolConfigFile {
    type Err = ToolConfigFileParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.split_once(':') {
            Some((tool, config_file)) => {
                if tool.is_empty() {
                    Err(ToolConfigFileParseError::EmptyToolName {
                        input: input.to_owned(),
                    })
                } else if config_file.is_empty() {
                    Err(ToolConfigFileParseError::EmptyConfigFile {
                        input: input.to_owned(),
                    })
                } else {
                    let config_file = Utf8Path::new(config_file);
                    if config_file.is_absolute() {
                        Ok(Self {
                            tool: tool.to_owned(),
                            config_file: Utf8PathBuf::from(config_file),
                        })
                    } else {
                        Err(ToolConfigFileParseError::ConfigFileNotAbsolute {
                            config_file: config_file.to_owned(),
                        })
                    }
                }
            }
            None => Err(ToolConfigFileParseError::InvalidFormat {
                input: input.to_owned(),
            }),
        }
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
    overrides: Vec<&'cfg ProfileOverrideImpl>,
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

    /// Returns the number of threads to run against for this profile.
    pub fn test_threads(&self) -> TestThreads {
        self.custom_profile
            .and_then(|profile| profile.test_threads)
            .unwrap_or(self.default_profile.test_threads)
    }

    /// Returns the time after which tests are treated as slow for this profile.
    pub fn slow_timeout(&self) -> SlowTimeout {
        self.custom_profile
            .and_then(|profile| profile.slow_timeout)
            .unwrap_or(self.default_profile.slow_timeout)
    }

    /// Returns the time after which a child process that hasn't closed its handles is marked as
    /// leaky.
    pub fn leak_timeout(&self) -> Duration {
        self.custom_profile
            .and_then(|profile| profile.leak_timeout)
            .unwrap_or(self.default_profile.leak_timeout)
    }

    /// Returns the test status level.
    pub fn status_level(&self) -> StatusLevel {
        self.custom_profile
            .and_then(|profile| profile.status_level)
            .unwrap_or(self.default_profile.status_level)
    }

    /// Returns the test status level at the end of the run.
    pub fn final_status_level(&self) -> FinalStatusLevel {
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

    /// Returns override settings for individual tests.
    pub fn overrides_for(&self, query: &TestQuery<'_>) -> ProfileOverrides {
        let mut retries = None;
        let mut slow_timeout = None;
        let mut leak_timeout = None;

        for &override_ in &self.overrides {
            if !override_.expr.matches_test(query) {
                continue;
            }
            if retries.is_none() && override_.data.retries.is_some() {
                retries = override_.data.retries;
            }
            if slow_timeout.is_none() && override_.data.slow_timeout.is_some() {
                slow_timeout = override_.data.slow_timeout;
            }
            if leak_timeout.is_none() && override_.data.leak_timeout.is_some() {
                leak_timeout = override_.data.leak_timeout;
            }
        }

        ProfileOverrides {
            retries,
            slow_timeout,
            leak_timeout,
        }
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

/// Override settings for individual tests.
///
/// Returned by
#[derive(Clone, Debug)]
pub struct ProfileOverrides {
    retries: Option<usize>,
    slow_timeout: Option<SlowTimeout>,
    leak_timeout: Option<Duration>,
}

impl ProfileOverrides {
    /// Returns the number of retries for this test.
    pub fn retries(&self) -> Option<usize> {
        self.retries
    }

    /// Returns the slow timeout for this test.
    pub fn slow_timeout(&self) -> Option<SlowTimeout> {
        self.slow_timeout
    }

    /// Returns the leak timeout for this test.
    pub fn leak_timeout(&self) -> Option<Duration> {
        self.leak_timeout
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
    test_threads: TestThreads,
    retries: usize,
    status_level: StatusLevel,
    final_status_level: FinalStatusLevel,
    failure_output: TestOutputDisplay,
    success_output: TestOutputDisplay,
    fail_fast: bool,
    #[serde(deserialize_with = "require_deserialize_slow_timeout")]
    slow_timeout: SlowTimeout,
    #[serde(with = "humantime_serde")]
    leak_timeout: Duration,
    #[serde(default)]
    overrides: Vec<ProfileOverrideSource>,
    junit: DefaultJunitImpl,
}

/// Type for the test-threads config key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestThreads {
    /// Run tests with a specified number of threads.
    Count(usize),

    /// Run tests with a number of threads equal to the logical CPU count.
    NumCpus,
}

impl TestThreads {
    /// Gets the actual number of test threads computed at runtime.
    pub fn compute(self) -> usize {
        match self {
            Self::Count(threads) => threads,
            Self::NumCpus => num_cpus::get(),
        }
    }
}

impl FromStr for TestThreads {
    type Err = TestThreadsParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "num-cpus" {
            Ok(Self::NumCpus)
        } else if let Ok(threads) = s.parse::<usize>() {
            Ok(Self::Count(threads))
        } else {
            Err(TestThreadsParseError::new(s))
        }
    }
}

impl<'de> Deserialize<'de> for TestThreads {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;

        impl<'de2> serde::de::Visitor<'de2> for V {
            type Value = TestThreads;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "an integer or the string \"num-cpus\"")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v == "num-cpus" {
                    Ok(TestThreads::NumCpus)
                } else {
                    Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Str(v),
                        &self,
                    ))
                }
            }

            // Note that TOML uses i64, not u64.
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(TestThreads::Count(v as usize))
            }
        }

        deserializer.deserialize_any(V)
    }
}

/// Type for the slow-timeout config key.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SlowTimeout {
    #[serde(with = "humantime_serde")]
    pub(crate) period: Duration,
    #[serde(default)]
    pub(crate) terminate_after: Option<NonZeroUsize>,
}

fn require_deserialize_slow_timeout<'de, D>(deserializer: D) -> Result<SlowTimeout, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match deserialize_slow_timeout(deserializer) {
        Ok(None) => Err(serde::de::Error::missing_field("field missing or null")),
        Err(e) => Err(e),
        Ok(Some(st)) => Ok(st),
    }
}

fn deserialize_slow_timeout<'de, D>(deserializer: D) -> Result<Option<SlowTimeout>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<SlowTimeout>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ period = \"60s\", terminate-after = 2 }}) or a string (\"60s\")"
            )
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if v.is_empty() {
                Ok(None)
            } else {
                let period = humantime_serde::deserialize(v.into_deserializer())?;
                Ok(Some(SlowTimeout {
                    period,
                    terminate_after: None,
                }))
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            SlowTimeout::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
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
    test_threads: Option<TestThreads>,
    #[serde(default)]
    status_level: Option<StatusLevel>,
    #[serde(default)]
    final_status_level: Option<FinalStatusLevel>,
    #[serde(default)]
    failure_output: Option<TestOutputDisplay>,
    #[serde(default)]
    success_output: Option<TestOutputDisplay>,
    #[serde(default)]
    fail_fast: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default, with = "humantime_serde::option")]
    leak_timeout: Option<Duration>,
    #[serde(default)]
    overrides: Vec<ProfileOverrideSource>,
    #[serde(default)]
    junit: JunitImpl,
}

/// Pre-compiled form of profile overrides.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ProfileOverrideSource {
    /// The filter expression to match against.
    filter: String,
    /// Overrides.
    #[serde(flatten)]
    data: ProfileOverrideData,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ProfileOverrideData {
    #[serde(default)]
    retries: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_slow_timeout")]
    slow_timeout: Option<SlowTimeout>,
    #[serde(default)]
    leak_timeout: Option<Duration>,
}

#[derive(Clone, Debug, Default)]
struct NextestOverridesImpl {
    default: Vec<ProfileOverrideImpl>,
    other: HashMap<String, Vec<ProfileOverrideImpl>>,
}

impl NextestOverridesImpl {
    fn new(graph: &PackageGraph, config: &NextestConfigImpl) -> Result<Self, ConfigParseErrorKind> {
        let mut errors = vec![];
        let default = Self::compile_overrides(
            graph,
            "default",
            &config.profiles.default.overrides,
            &mut errors,
        );
        let other: HashMap<_, _> = config
            .profiles
            .other
            .iter()
            .map(|(profile_name, profile)| {
                (
                    profile_name.clone(),
                    Self::compile_overrides(graph, profile_name, &profile.overrides, &mut errors),
                )
            })
            .collect();

        if errors.is_empty() {
            Ok(Self { default, other })
        } else {
            Err(ConfigParseErrorKind::OverrideError(errors))
        }
    }

    fn compile_overrides(
        graph: &PackageGraph,
        profile_name: &str,
        overrides: &[ProfileOverrideSource],
        errors: &mut Vec<ConfigParseOverrideError>,
    ) -> Vec<ProfileOverrideImpl> {
        overrides
            .iter()
            .filter_map(|source| ProfileOverrideImpl::new(graph, profile_name, source, errors))
            .collect()
    }
}

#[derive(Clone, Debug)]
struct ProfileOverrideImpl {
    expr: FilteringExpr,
    data: ProfileOverrideData,
}

impl ProfileOverrideImpl {
    fn new(
        graph: &PackageGraph,
        profile_name: &str,
        source: &ProfileOverrideSource,
        errors: &mut Vec<ConfigParseOverrideError>,
    ) -> Option<Self> {
        match FilteringExpr::parse(&source.filter, graph) {
            Ok(expr) => Some(Self {
                expr,
                data: source.data.clone(),
            }),
            Err(parse_errors) => {
                errors.push(ConfigParseOverrideError {
                    profile_name: profile_name.to_owned(),
                    parse_errors,
                });
                None
            }
        }
    }
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
    use guppy::{graph::cargo::BuildPlatform, MetadataCommand};
    use indoc::indoc;
    use nextest_filtering::BinaryQuery;
    use std::{io::Write, path::PathBuf, process::Command};
    use tempfile::tempdir;
    use test_case::test_case;

    #[test]
    fn default_config_is_valid() {
        let default_config = NextestConfig::default_config("foo");
        default_config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile should exist");
    }

    #[test_case(
        "",
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: None }),
        None

        ; "empty config is expected to use the hardcoded values"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(30), terminate_after: None }),
        None

        ; "overrides the default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "30s"

            [profile.ci]
            slow-timeout = { period = "60s", terminate-after = 3 }
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(30), terminate_after: None }),
        Some(SlowTimeout { period: Duration::from_secs(60), terminate_after: Some(NonZeroUsize::new(3).unwrap()), })

        ; "adds a custom profile 'ci'"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 3 }

            [profile.ci]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: Some(NonZeroUsize::new(3).unwrap()) }),
        Some(SlowTimeout { period: Duration::from_secs(30), terminate_after: None, })

        ; "ci profile uses string notation"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s" }
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: None }),
        None

        ; "partial table"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 0 }
        "#},
        Err("original: invalid value: integer `0`, expected a nonzero usize"),
        None

        ; "zero terminate-after should fail"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "60s"

            [profile.ci]
            slow-timeout = { terminate-after = 3 }
        "#},
        Err("original: missing field `period`"),
        None

        ; "partial slow-timeout table should error"
    )]
    fn slowtimeout_adheres_to_hierarchy(
        config_contents: &str,
        expected_default: Result<SlowTimeout, &str>,
        maybe_expected_ci: Option<SlowTimeout>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let nextest_config_result =
            NextestConfig::from_sources(graph.workspace().root(), &graph, None, []);

        match expected_default {
            Ok(expected_default) => {
                let nextest_config = nextest_config_result.expect("config file should parse");

                assert_eq!(
                    nextest_config
                        .profile("default")
                        .expect("default profile should exist")
                        .slow_timeout(),
                    expected_default,
                );

                if let Some(expected_ci) = maybe_expected_ci {
                    assert_eq!(
                        nextest_config
                            .profile("ci")
                            .expect("ci profile should exist")
                            .slow_timeout(),
                        expected_ci,
                    );
                }
            }

            Err(expected_err_str) => {
                let err_str = format!("{:?}", nextest_config_result.unwrap_err());

                assert!(
                    err_str.contains(expected_err_str),
                    "expected error string not found: {}",
                    err_str,
                )
            }
        }
    }

    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        Some(2)

        ; "my_test matches exactly"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "!test(=my_test)"
            retries = 2

            [profile.ci]
        "#},
        None

        ; "not match"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(=my_test)"

            [profile.ci]
        "#},
        None

        ; "no retries specified"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [[profile.default.overrides]]
            filter = "test(=my_test)"
            retries = 3

            [profile.ci]
        "#},
        Some(2)

        ; "earlier configs override later ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "test(=my_test)"
            retries = 3
        "#},
        Some(3)

        ; "profile-specific configs override default ones"
    )]
    #[test_case(
        indoc! {r#"
            [[profile.default.overrides]]
            filter = "(!package(test-package)) and test(test)"
            retries = 2

            [profile.ci]

            [[profile.ci.overrides]]
            filter = "!test(=my_test_2)"
            retries = 3
        "#},
        Some(3)

        ; "no overrides match my_test exactly"
    )]

    fn overrides_retries(config_contents: &str, retries: Option<usize>) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id();

        let config =
            NextestConfig::from_sources(graph.workspace().root(), &graph, None, []).unwrap();
        let query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "my_test",
        };
        let overrides_for = config
            .profile("ci")
            .expect("ci profile is defined")
            .overrides_for(&query);
        assert_eq!(
            overrides_for.retries(),
            retries,
            "actual retries don't match expected retries"
        );
    }

    #[test]
    fn parse_tool_config_file() {
        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                let valid = ["tool:C:\\foo\\bar", "tool:\\\\?\\C:\\foo\\bar"];
                let invalid = ["C:\\foo\\bar", "tool:\\foo\\bar", "tool:", ":/foo/bar"];
            } else {
                let valid = ["tool:/foo/bar"];
                let invalid = ["/foo/bar", "tool:", ":/foo/bar", "tool:foo/bar"];
            }
        }

        for valid_input in valid {
            valid_input.parse::<ToolConfigFile>().unwrap_or_else(|err| {
                panic!("valid input {valid_input} should parse correctly: {err}")
            });
        }

        for invalid_input in invalid {
            invalid_input
                .parse::<ToolConfigFile>()
                .expect_err(&format!("invalid input {invalid_input} should error out"));
        }
    }

    #[test]
    fn lowpri_config() {
        let config_contents = r#"
        [profile.default]
        retries = 3

        [[profile.default.overrides]]
        filter = 'test(test_foo)'
        retries = 20
        "#;

        let lowpri1_config_contents = r#"
        [profile.default]
        retries = 4

        [[profile.default.overrides]]
        filter = 'test(test_bar)'
        retries = 21

        [profile.lowpri]
        retries = 12

        [[profile.lowpri.overrides]]
        filter = 'test(test_baz)'
        retries = 22
        "#;

        let lowpri2_config_contents = r#"
        [profile.default]
        retries = 5

        [[profile.default.overrides]]
        filter = 'test(test_)'
        retries = 23

        [profile.lowpri]
        retries = 16

        [[profile.lowpri.overrides]]
        filter = 'test(test_ba)'
        retries = 24

        [[profile.lowpri.overrides]]
        filter = 'test(test_)'
        retries = 25

        [profile.lowpri2]
        retries = 18

        [[profile.lowpri2.overrides]]
        filter = 'all()'
        retries = 26
        "#;

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);
        let workspace_root = graph.workspace().root();
        let lowpri1_path = workspace_root.join(".config/lowpri1.toml");
        let lowpri2_path = workspace_root.join(".config/lowpri2.toml");
        std::fs::write(&lowpri1_path, lowpri1_config_contents).unwrap();
        std::fs::write(&lowpri2_path, lowpri2_config_contents).unwrap();

        let config = NextestConfig::from_sources(
            workspace_root,
            &graph,
            None,
            &[
                ToolConfigFile {
                    tool: "lowpri1".to_owned(),
                    config_file: lowpri1_path,
                },
                ToolConfigFile {
                    tool: "lowpri2".to_owned(),
                    config_file: lowpri2_path,
                },
            ],
        )
        .expect("parsing config failed");

        let default_profile = config
            .profile(NextestConfig::DEFAULT_PROFILE)
            .expect("default profile is present");
        // This is present in .config/nextest.toml and is the highest priority
        assert_eq!(default_profile.retries(), 3);

        let package_id = graph.workspace().iter().next().unwrap().id();

        let test_foo_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_foo",
        };
        let test_bar_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_bar",
        };
        let test_baz_query = TestQuery {
            binary_query: BinaryQuery {
                package_id,
                kind: "lib",
                binary_name: "my-binary",
                platform: BuildPlatform::Target,
            },
            test_name: "test_baz",
        };

        assert_eq!(
            default_profile.overrides_for(&test_foo_query).retries(),
            Some(20),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_bar_query).retries(),
            Some(21),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_baz_query).retries(),
            Some(23),
            "retries for test_baz/default profile"
        );

        let lowpri_profile = config.profile("lowpri").expect("lowpri profile is present");
        assert_eq!(lowpri_profile.retries(), 12);
        assert_eq!(
            lowpri_profile.overrides_for(&test_foo_query).retries(),
            Some(25),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            lowpri_profile.overrides_for(&test_bar_query).retries(),
            Some(24),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            lowpri_profile.overrides_for(&test_baz_query).retries(),
            Some(22),
            "retries for test_baz/default profile"
        );

        let lowpri2_profile = config
            .profile("lowpri2")
            .expect("lowpri2 profile is present");
        assert_eq!(lowpri2_profile.retries(), 18);
        assert_eq!(
            lowpri2_profile.overrides_for(&test_foo_query).retries(),
            Some(26),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            lowpri2_profile.overrides_for(&test_bar_query).retries(),
            Some(26),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            lowpri2_profile.overrides_for(&test_baz_query).retries(),
            Some(26),
            "retries for test_baz/default profile"
        );
    }

    fn temp_workspace(temp_dir: &Utf8Path, config_contents: &str) -> PackageGraph {
        Command::new(cargo_path())
            .args(["init", "--lib", "--name=test-package"])
            .current_dir(temp_dir)
            .status()
            .expect("error initializing cargo project");

        let config_dir = temp_dir.join(".config");
        std::fs::create_dir(&config_dir).expect("error creating config dir");

        let config_path = config_dir.join("nextest.toml");
        let mut config_file = std::fs::File::create(&config_path).unwrap();
        config_file.write_all(config_contents.as_bytes()).unwrap();

        PackageGraph::from_command(MetadataCommand::new().current_dir(temp_dir))
            .expect("error creating package graph")
    }

    fn cargo_path() -> Utf8PathBuf {
        match std::env::var_os("CARGO") {
            Some(cargo_path) => PathBuf::from(cargo_path)
                .try_into()
                .expect("CARGO env var is not valid UTF-8"),
            None => Utf8PathBuf::from("cargo"),
        }
    }
}
