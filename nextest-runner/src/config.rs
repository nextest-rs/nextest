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
use std::{
    cmp::Ordering, collections::HashMap, fmt, num::NonZeroUsize, str::FromStr, time::Duration,
};

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
    pub fn retries(&self) -> RetryPolicy {
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
    retries: Option<RetryPolicy>,
    slow_timeout: Option<SlowTimeout>,
    leak_timeout: Option<Duration>,
}

impl ProfileOverrides {
    /// Returns the number of retries for this test.
    pub fn retries(&self) -> Option<RetryPolicy> {
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
    #[serde(default, deserialize_with = "deserialize_retry_policy_or_default")]
    retries: RetryPolicy,
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
            return Ok(Self::NumCpus);
        }

        match s.parse::<isize>() {
            Err(e) => Err(TestThreadsParseError::new(format!(
                "Error: {e} parsing {s}"
            ))),
            Ok(0) => Err(TestThreadsParseError::new("jobs may not be 0")),
            Ok(j) if j < 0 => Ok(TestThreads::Count(
                (num_cpus::get() as isize + j).max(1) as usize
            )),
            Ok(j) => Ok(TestThreads::Count(j as usize)),
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
                match v.cmp(&0) {
                    Ordering::Greater => Ok(TestThreads::Count(v as usize)),
                    Ordering::Less => Ok(TestThreads::Count(
                        (num_cpus::get() as i64 + v).max(1) as usize
                    )),
                    Ordering::Equal => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Signed(v),
                        &self,
                    )),
                }
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
    #[serde(with = "humantime_serde", default = "default_grace_period")]
    pub(crate) grace_period: Duration,
}

fn default_grace_period() -> Duration {
    Duration::from_secs(10)
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
                    grace_period: default_grace_period(),
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

/// Type for the retry config key.
#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "backoff", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RetryPolicy {
    /// Fixed backoff.
    #[serde(rename_all = "kebab-case")]
    Fixed {
        /// Maximum retry count.
        count: usize,

        /// Delay between retries.
        #[serde(default, with = "humantime_serde")]
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        #[serde(default)]
        jitter: bool,
    },

    /// Exponential backoff.
    #[serde(rename_all = "kebab-case")]
    Exponential {
        /// Maximum retry count.
        count: usize,

        /// Delay between retries. Not optional for exponential backoff.
        #[serde(with = "humantime_serde")]
        delay: Duration,

        /// If set to true, randomness will be added to the delay on each retry attempt.
        #[serde(default)]
        jitter: bool,

        /// If set, limits the delay between retries.
        #[serde(default, with = "humantime_serde")]
        max_delay: Option<Duration>,
    },
}

impl Default for RetryPolicy {
    #[inline]
    fn default() -> Self {
        Self::new_without_delay(0)
    }
}

impl RetryPolicy {
    /// Create new policy with no delay between retries.
    pub fn new_without_delay(count: usize) -> Self {
        Self::Fixed {
            count,
            delay: Duration::ZERO,
            jitter: false,
        }
    }

    /// Returns the number of retries.
    pub fn count(&self) -> usize {
        match self {
            Self::Fixed { count, .. } | Self::Exponential { count, .. } => *count,
        }
    }
}

fn deserialize_retry_policy_or_default<'de, D>(deserializer: D) -> Result<RetryPolicy, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match deserialize_retry_policy(deserializer) {
        Ok(None) => Ok(Default::default()),
        Err(e) => Err(e),
        Ok(Some(st)) => Ok(st),
    }
}

fn deserialize_retry_policy<'de, D>(deserializer: D) -> Result<Option<RetryPolicy>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;

    impl<'de2> serde::de::Visitor<'de2> for V {
        type Value = Option<RetryPolicy>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "a table ({{ count = 5, backoff = \"exponential\", delay = \"1s\", max-delay = \"10s\", jitter = true }}) or a number (5)"
            )
        }

        // Note that TOML uses i64, not u64.
        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match v.cmp(&0) {
                Ordering::Greater => Ok(Some(RetryPolicy::new_without_delay(v as usize))),
                Ordering::Less => Err(serde::de::Error::invalid_value(
                    serde::de::Unexpected::Signed(v),
                    &self,
                )),
                Ordering::Equal => Ok(None),
            }
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de2>,
        {
            RetryPolicy::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
        }
    }

    // Post-deserialize validation of retry policy.
    let retry_policy = deserializer.deserialize_any(V)?;
    match &retry_policy {
        Some(RetryPolicy::Fixed {
            count: _,
            delay,
            jitter,
        }) => {
            // Jitter can't be specified if delay is 0.
            if delay.is_zero() && *jitter {
                return Err(serde::de::Error::custom(
                    "`jitter` cannot be true if `delay` isn't specified or is zero",
                ));
            }
        }
        Some(RetryPolicy::Exponential {
            count,
            delay,
            jitter: _,
            max_delay,
        }) => {
            // Count can't be zero.
            if *count == 0 {
                return Err(serde::de::Error::custom(
                    "`count` cannot be zero with exponential backoff",
                ));
            }
            // Delay can't be zero.
            if delay.is_zero() {
                return Err(serde::de::Error::custom(
                    "`delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay, if specified, can't be zero.
            if max_delay.map_or(false, |f| f.is_zero()) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be zero with exponential backoff",
                ));
            }
            // Max delay can't be less than delay.
            if max_delay.map_or(false, |max_delay| max_delay < *delay) {
                return Err(serde::de::Error::custom(
                    "`max-delay` cannot be less than delay with exponential backoff",
                ));
            }
        }
        None => {}
    }

    Ok(retry_policy)
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
    #[serde(default, deserialize_with = "deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
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
    #[serde(default, deserialize_with = "deserialize_retry_policy")]
    retries: Option<RetryPolicy>,
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
    use config::ConfigError;
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
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: None, grace_period: Duration::from_secs(10) }),
        None

        ; "empty config is expected to use the hardcoded values"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(30), terminate_after: None, grace_period: Duration::from_secs(10) }),
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
        Ok(SlowTimeout { period: Duration::from_secs(30), terminate_after: None, grace_period: Duration::from_secs(10) }),
        Some(SlowTimeout { period: Duration::from_secs(60), terminate_after: Some(NonZeroUsize::new(3).unwrap()), grace_period: Duration::from_secs(10) })

        ; "adds a custom profile 'ci'"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 3 }

            [profile.ci]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: Some(NonZeroUsize::new(3).unwrap()), grace_period: Duration::from_secs(10) }),
        Some(SlowTimeout { period: Duration::from_secs(30), terminate_after: None, grace_period: Duration::from_secs(10) })

        ; "ci profile uses string notation"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s", terminate-after = 3, grace-period = "1s" }

            [profile.ci]
            slow-timeout = "30s"
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: Some(NonZeroUsize::new(3).unwrap()), grace_period: Duration::from_secs(1) }),
        Some(SlowTimeout { period: Duration::from_secs(30), terminate_after: None, grace_period: Duration::from_secs(10) })

        ; "timeout grace period"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            slow-timeout = { period = "60s" }
        "#},
        Ok(SlowTimeout { period: Duration::from_secs(60), terminate_after: None, grace_period: Duration::from_secs(10) }),
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
        Some(RetryPolicy::new_without_delay(2))

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
        Some(RetryPolicy::new_without_delay(2))

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
        Some(RetryPolicy::new_without_delay(3))

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
        Some(RetryPolicy::new_without_delay(3))

        ; "no overrides match my_test exactly"
    )]

    fn overrides_retries(config_contents: &str, retries: Option<RetryPolicy>) {
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
    fn parse_retries_valid() {
        let config_contents = indoc! {r#"
            [profile.default]
            retries = { backoff = "fixed", count = 3 }

            [profile.fixed-with-delay]
            retries = { backoff = "fixed", count = 3, delay = "1s" }

            [profile.exp]
            retries = { backoff = "exponential", count = 4, delay = "2s" }

            [profile.exp-with-max-delay]
            retries = { backoff = "exponential", count = 5, delay = "3s", max-delay = "10s" }

            [profile.exp-with-max-delay-and-jitter]
            retries = { backoff = "exponential", count = 6, delay = "4s", max-delay = "1m", jitter = true }
        "#};

        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config = NextestConfig::from_sources(graph.workspace().root(), &graph, None, [])
            .expect("config is valid");
        assert_eq!(
            config
                .profile("default")
                .expect("default profile exists")
                .retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::ZERO,
                jitter: false,
            },
            "default retries matches"
        );

        assert_eq!(
            config
                .profile("fixed-with-delay")
                .expect("profile exists")
                .retries(),
            RetryPolicy::Fixed {
                count: 3,
                delay: Duration::from_secs(1),
                jitter: false,
            },
            "fixed-with-delay retries matches"
        );

        assert_eq!(
            config.profile("exp").expect("profile exists").retries(),
            RetryPolicy::Exponential {
                count: 4,
                delay: Duration::from_secs(2),
                jitter: false,
                max_delay: None,
            },
            "exp retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay")
                .expect("profile exists")
                .retries(),
            RetryPolicy::Exponential {
                count: 5,
                delay: Duration::from_secs(3),
                jitter: false,
                max_delay: Some(Duration::from_secs(10)),
            },
            "exp-with-max-delay retries matches"
        );

        assert_eq!(
            config
                .profile("exp-with-max-delay-and-jitter")
                .expect("profile exists")
                .retries(),
            RetryPolicy::Exponential {
                count: 6,
                delay: Duration::from_secs(4),
                jitter: true,
                max_delay: Some(Duration::from_secs(60)),
            },
            "exp-with-max-delay-and-jitter retries matches"
        );
    }

    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "foo" }
        "#},
        "unknown variant `foo`, expected `fixed` or `exponential`"
        ; "invalid value for backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed" }
        "#},
        "missing field `count`"
        ; "fixed specified without count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, delay = "foobar" }
        "#},
        "invalid value: string \"foobar\", expected a duration"
        ; "delay is not a valid duration")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, jitter = true }
        "#},
        "`jitter` cannot be true if `delay` isn't specified or is zero"
        ; "jitter specified without delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "fixed", count = 1, max-delay = "10s" }
        "#},
        "unknown field `max-delay`, expected one of `count`, `delay`, `jitter`"
        ; "max-delay is incompatible with fixed backoff")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1 }
        "#},
        "missing field `delay`"
        ; "exponential backoff must specify delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", delay = "1s" }
        "#},
        "missing field `count`"
        ; "exponential backoff must specify count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 0, delay = "1s" }
        "#},
        "`count` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero count")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "0s" }
        "#},
        "`delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "1s", max-delay = "0s" }
        "#},
        "`max-delay` cannot be zero with exponential backoff"
        ; "exponential backoff must have a non-zero max delay")]
    #[test_case(
        indoc!{r#"
            [profile.default]
            retries = { backoff = "exponential", count = 1, delay = "4s", max-delay = "2s", jitter = true }
        "#},
        "`max-delay` cannot be less than delay"
        ; "max-delay greater than delay")]
    fn parse_retries_invalid(config_contents: &str, expected_message: &str) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config_err = NextestConfig::from_sources(graph.workspace().root(), &graph, None, [])
            .expect_err("config expected to be invalid");

        let message = match config_err.kind() {
            ConfigParseErrorKind::DeserializeError(path_error) => match path_error.inner() {
                ConfigError::Message(message) => message,
                other => {
                    panic!("for config error {config_err:?}, expected ConfigError::Message for inner error {other:?}");
                }
            },
            other => {
                panic!(
                    "for config error {other:?}, expected ConfigParseErrorKind::DeserializeError"
                );
            }
        };

        assert!(
            message.contains(expected_message),
            "expected message \"{message}\" to contain \"{expected_message}\""
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
        assert_eq!(default_profile.retries(), RetryPolicy::new_without_delay(3));

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
            Some(RetryPolicy::new_without_delay(20)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(21)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            default_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(23)),
            "retries for test_baz/default profile"
        );

        let lowpri_profile = config.profile("lowpri").expect("lowpri profile is present");
        assert_eq!(lowpri_profile.retries(), RetryPolicy::new_without_delay(12));
        assert_eq!(
            lowpri_profile.overrides_for(&test_foo_query).retries(),
            Some(RetryPolicy::new_without_delay(25)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            lowpri_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(24)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            lowpri_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(22)),
            "retries for test_baz/default profile"
        );

        let lowpri2_profile = config
            .profile("lowpri2")
            .expect("lowpri2 profile is present");
        assert_eq!(
            lowpri2_profile.retries(),
            RetryPolicy::new_without_delay(18)
        );
        assert_eq!(
            lowpri2_profile.overrides_for(&test_foo_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_foo/default profile"
        );
        assert_eq!(
            lowpri2_profile.overrides_for(&test_bar_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_bar/default profile"
        );
        assert_eq!(
            lowpri2_profile.overrides_for(&test_baz_query).retries(),
            Some(RetryPolicy::new_without_delay(26)),
            "retries for test_baz/default profile"
        );
    }

    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = -1
        "#},
        None

        ; "negative_j"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = 2
        "#},
        Some(2)

        ; "positive_j"
    )]
    #[test_case(
        indoc! {r#"
            [profile.custom]
            test-threads = 0
        "#},
        Some(0)

        ; "zero_j"
    )]
    fn parse_test_threads_negative_jobs(config_contents: &str, n_threads: Option<usize>) {
        let workspace_dir = tempdir().unwrap();
        let workspace_path: &Utf8Path = workspace_dir.path().try_into().unwrap();

        let graph = temp_workspace(workspace_path, config_contents);

        let config = NextestConfig::from_sources(graph.workspace().root(), &graph, None, []);
        match n_threads {
            Some(0) => assert!(config.is_err()),
            Some(j) => assert_eq!(
                config
                    .unwrap()
                    .profile("custom")
                    .unwrap()
                    .custom_profile
                    .unwrap()
                    .test_threads
                    .unwrap()
                    .compute(),
                j
            ),
            None => assert_eq!(
                config
                    .unwrap()
                    .profile("custom")
                    .unwrap()
                    .custom_profile
                    .unwrap()
                    .test_threads
                    .unwrap()
                    .compute(),
                num_cpus::get() - 1
            ),
        }
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
